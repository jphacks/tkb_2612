//! M4.5：実データのreleaseに対する、新方式（schema 3）エンジンの契約の検査。
//!
//! `ROUTING_REAL_RELEASE_DIR` にreleaseのディレクトリを指定し、`--ignored` で実行する。
//!
//! ```sh
//! ROUTING_REAL_RELEASE_DIR=backend/data/releases/<release_id> \
//!     cargo test --release --manifest-path backend/Cargo.toml \
//!         --test routing_real_release -- --ignored --nocapture
//! ```
//!
//! 問い合わせは `tests/fixtures/routing-kanto4-queries.json` の固定の300件。
//! M3bのOD一覧 `kanto4.jsonl` から、4地域（tokyo23・yokohama・saitama・chiba）と
//! 3手段（walk・bike・car）の各組の先頭25件を取った。通し番号i=0..299について、
//! 時間帯は `[null, 早朝, 昼, 夕, 夜, 深夜]` の `i % 6`、曜日区分は
//! `[null, 平日, 休日]` の `(i / 6) % 3`、天候は `[null, 晴, 曇, 雨, 霧, 雪]` の
//! `(i / 18) % 6`、遠回りの上限（分）は `[0, 5, 7.5, 10, 20, 30]` の `(i / 2) % 6` を
//! 割り当てた（除算は整数）。`STRAIGHT_DISTANCE_RANGE_M`（20m〜30km）の範囲外の組は
//! 先頭25件に無かったため、除外・補充はしていない。
//!
//! 各問い合わせは期限35秒の [`CancelToken`] で `collect_candidates`（空の
//! `CandidateSet`）→ `evaluate` を呼び、`check_outcome_invariants`・`check_detour`・
//! `render_route_response` がすべて通ることを確かめる。`NoRoute` と `RouteTooLong` は
//! 契約どおりの応答として数える。処理は4スレッドで並行に行う。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use accident_route_backend::api::route_response::{check_detour, render_route_response};
use accident_route_backend::contract::{
    CancelToken, ConditionFilter, DayType, EngineError, LatLon, Mode, Period, RouteEngine,
    RouteQuery, Router, Weather,
};
use accident_route_backend::data;
use accident_route_backend::testing::fixtures::check_outcome_invariants;
use accident_route_backend::testing::{RouterResponse, ScriptedRouter};
use serde::Deserialize;

const QUERIES_JSON: &str = include_str!("fixtures/routing-kanto4-queries.json");
const EXPECTED_QUERIES: usize = 300;
const WORKERS: usize = 4;
const QUERY_DEADLINE: Duration = Duration::from_secs(35);

#[derive(Deserialize)]
struct Fixture {
    description: String,
    queries: Vec<QueryFixture>,
}

#[derive(Deserialize)]
struct QueryFixture {
    id: String,
    region: String,
    mode: String,
    origin: PointFixture,
    dest: PointFixture,
    period: Option<String>,
    day_type: Option<String>,
    weather: Option<String>,
    max_detour_min: f64,
}

#[derive(Deserialize)]
struct PointFixture {
    lat: f64,
    lon: f64,
}

/// 検証済みの1件の問い合わせ。
struct ParsedQuery {
    id: String,
    mode: Mode,
    query: RouteQuery,
}

impl QueryFixture {
    fn parse(&self) -> ParsedQuery {
        let mode = Mode::parse(&self.mode)
            .unwrap_or_else(|| panic!("{}: 知らないmode {}", self.id, self.mode));
        let conditions = ConditionFilter {
            period: self.period.as_deref().map(|label| {
                Period::from_label(label)
                    .unwrap_or_else(|| panic!("{}: 知らない時間帯 {label}", self.id))
            }),
            day_type: self.day_type.as_deref().map(|label| {
                DayType::from_label(label)
                    .unwrap_or_else(|| panic!("{}: 知らない曜日区分 {label}", self.id))
            }),
            weather: self.weather.as_deref().map(|label| {
                Weather::from_label(label)
                    .unwrap_or_else(|| panic!("{}: 知らない天候 {label}", self.id))
            }),
        };
        let query = RouteQuery::new(
            LatLon::new(self.origin.lat, self.origin.lon),
            LatLon::new(self.dest.lat, self.dest.lon),
            mode,
            self.max_detour_min,
            conditions,
        )
        .unwrap_or_else(|e| panic!("{}: 問い合わせが不正: {e:?}", self.id));
        ParsedQuery {
            id: self.id.clone(),
            mode,
            query,
        }
    }
}

/// 成功した問い合わせの集計に使う値。結果そのものは検査後すぐに捨てる。
struct OutcomeStats {
    candidates: usize,
    safest_is_none: bool,
    high_risk_total: u64,
    render_bytes: usize,
}

enum RunResult {
    /// 契約の検査がすべて通った。
    Ok(OutcomeStats),
    /// 契約どおりのエラー（NoRoute・RouteTooLong）。
    Allowed(EngineError),
    /// 契約違反・中断・その他のエラー。
    Failed(String),
}

struct QueryRun {
    id: String,
    mode: Mode,
    elapsed: Duration,
    result: RunResult,
}

fn load_queries() -> Vec<ParsedQuery> {
    let fixture: Fixture =
        serde_json::from_str(QUERIES_JSON).expect("問い合わせのfixtureがJSONとして読める");
    eprintln!("ROUTING REAL: fixture={}", fixture.description);
    assert_eq!(
        fixture.queries.len(),
        EXPECTED_QUERIES,
        "問い合わせの件数が300でない"
    );

    // 4地域×3手段の各組が25件ずつであること（fixtureの取り違えを早期に気づく）。
    let mut per_block: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for query in &fixture.queries {
        *per_block
            .entry((query.region.as_str(), query.mode.as_str()))
            .or_insert(0) += 1;
    }
    assert_eq!(per_block.len(), 12, "地域×手段の組が12でない");
    assert!(
        per_block.values().all(|&count| count == 25),
        "地域×手段の件数が25でない: {per_block:?}"
    );

    fixture.queries.iter().map(QueryFixture::parse).collect()
}

fn is_allowed_error(error: &EngineError) -> bool {
    matches!(error, EngineError::NoRoute | EngineError::RouteTooLong)
}

/// 1件を期限付きで実行し、契約の検査まで行う。
fn run_query(
    engine: &dyn RouteEngine,
    runtime: &tokio::runtime::Runtime,
    router: &dyn Router,
    query: &ParsedQuery,
) -> QueryRun {
    let started = Instant::now();
    let cancel = CancelToken::with_timeout(QUERY_DEADLINE);
    let candidates = runtime.block_on(engine.collect_candidates(router, &query.query, &cancel));
    let evaluated = match candidates {
        Ok(candidates) => engine.evaluate(&query.query, candidates, &cancel),
        Err(error) => Err(error),
    };
    // 期限後に完了した結果は、APIと同じく中断として扱う。
    let evaluated = cancel.settle(evaluated);
    let elapsed = started.elapsed();

    let result = match evaluated {
        Ok(outcome) => {
            let mut reasons = Vec::new();
            if let Err(reason) = check_outcome_invariants(&outcome) {
                reasons.push(format!("check_outcome_invariants: {reason}"));
            }
            if let Err(reason) = check_detour(&outcome, query.query.max_detour_min()) {
                reasons.push(format!("check_detour: {}", reason.0));
            }
            let render_bytes = match render_route_response(&outcome) {
                Ok(bytes) => Some(bytes.len()),
                Err(reason) => {
                    reasons.push(format!("render_route_response: {}", reason.0));
                    None
                }
            };
            if reasons.is_empty() {
                RunResult::Ok(OutcomeStats {
                    candidates: outcome.candidates.len(),
                    safest_is_none: outcome.safest.is_none(),
                    high_risk_total: outcome
                        .candidates
                        .iter()
                        .map(|candidate| candidate.high_risk_total)
                        .sum(),
                    render_bytes: render_bytes.expect("renderに成功した本文の長さ"),
                })
            } else {
                RunResult::Failed(reasons.join(" / "))
            }
        }
        Err(error) if is_allowed_error(&error) => RunResult::Allowed(error),
        Err(error) => RunResult::Failed(format!("EngineError: {error:?}")),
    };

    QueryRun {
        id: query.id.clone(),
        mode: query.mode,
        elapsed,
        result,
    }
}

fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let middle = values.len() / 2;
    if values.len() & 1 == 0 {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

fn print_mode_summary(mode: Mode, runs: &[&QueryRun]) {
    let mut success = 0usize;
    let mut no_route = 0usize;
    let mut route_too_long = 0usize;
    let mut failed = 0usize;
    let mut times = Vec::with_capacity(runs.len());
    let mut candidates: BTreeMap<usize, usize> = BTreeMap::new();
    let mut safest_is_none = 0usize;
    let mut high_risk_total = 0u64;
    let mut max_render_bytes = 0usize;
    for run in runs {
        times.push(run.elapsed.as_secs_f64());
        match &run.result {
            RunResult::Ok(stats) => {
                success += 1;
                *candidates.entry(stats.candidates).or_insert(0) += 1;
                if stats.safest_is_none {
                    safest_is_none += 1;
                }
                high_risk_total += stats.high_risk_total;
                max_render_bytes = max_render_bytes.max(stats.render_bytes);
            }
            RunResult::Allowed(EngineError::NoRoute) => no_route += 1,
            RunResult::Allowed(EngineError::RouteTooLong) => route_too_long += 1,
            RunResult::Allowed(_) => failed += 1,
            RunResult::Failed(_) => failed += 1,
        }
    }
    let time_median = median(&mut times);
    let time_max = times.last().copied().unwrap_or(0.0);
    eprintln!(
        "ROUTING REAL: mode={} total={} success={} no_route={} route_too_long={} failed={}",
        mode.as_str(),
        runs.len(),
        success,
        no_route,
        route_too_long,
        failed
    );
    eprintln!(
        "ROUTING REAL: mode={} time_median={time_median:.3}s time_max={time_max:.3}s \
         candidates={candidates:?} safest_null={safest_is_none} \
         high_risk_total={high_risk_total} max_render_bytes={max_render_bytes}",
        mode.as_str()
    );
}

#[test]
#[ignore = "実releaseが必要（ROUTING_REAL_RELEASE_DIR を指定して --ignored で実行）"]
fn routing_real_release_satisfies_the_contract_for_300_queries() {
    let dir = std::env::var("ROUTING_REAL_RELEASE_DIR")
        .expect("ROUTING_REAL_RELEASE_DIR に実releaseのディレクトリを指定してください");
    let dir = std::fs::canonicalize(dir).expect("releaseディレクトリ");
    let queries = load_queries();

    let started = Instant::now();
    let engine = data::load_engine(&dir).expect("schema 3のreleaseを読み込める");
    let load_elapsed = started.elapsed();
    let info = engine.dataset();
    eprintln!(
        "ROUTING REAL: release_id={} dataset_id={} accidents={} data_years={:?} grid_cells={:?}",
        info.release_id, info.dataset_id, info.accidents, info.data_years, info.grid_cells
    );
    eprintln!(
        "ROUTING REAL: load_engine {:.3}s (load_seconds={:.3})",
        load_elapsed.as_secs_f64(),
        info.load_seconds
    );

    // 新方式はルーターを呼ばない。呼んだら ScriptedRouter がpanicする。
    let router = ScriptedRouter::new(std::iter::empty::<RouterResponse>());
    let chunk_size = queries.len().div_ceil(WORKERS);
    let runs: Vec<QueryRun> = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for chunk in queries.chunks(chunk_size) {
            let engine = Arc::clone(&engine);
            let router = &router;
            handles.push(scope.spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("スレッド用のruntime");
                let runs: Vec<QueryRun> = chunk
                    .iter()
                    .map(|query| run_query(engine.as_ref(), &runtime, router, query))
                    .collect();
                eprintln!("ROUTING REAL: shard done {}件", runs.len());
                runs
            }));
        }
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("ワーカースレッドが失敗した"))
            .collect()
    });

    // NoRoute・RouteTooLong は契約どおりの応答。件数とidを表示する。
    for (name, selected) in [
        ("NoRoute", EngineError::NoRoute),
        ("RouteTooLong", EngineError::RouteTooLong),
    ] {
        let ids: Vec<&str> = runs
            .iter()
            .filter(|run| match &run.result {
                RunResult::Allowed(error) => {
                    std::mem::discriminant(error) == std::mem::discriminant(&selected)
                }
                _ => false,
            })
            .map(|run| run.id.as_str())
            .collect();
        eprintln!(
            "ROUTING REAL: {name} count={} ids={}",
            ids.len(),
            ids.join(",")
        );
    }

    let failures: Vec<(&QueryRun, &str)> = runs
        .iter()
        .filter_map(|run| match &run.result {
            RunResult::Failed(reason) => Some((run, reason.as_str())),
            _ => None,
        })
        .collect();
    for (run, reason) in &failures {
        eprintln!(
            "ROUTING REAL FAIL: id={} mode={} reason={reason}",
            run.id,
            run.mode.as_str()
        );
    }
    for mode in Mode::ALL {
        let selected: Vec<&QueryRun> = runs.iter().filter(|run| run.mode == mode).collect();
        print_mode_summary(mode, &selected);
    }
    let total_success = runs
        .iter()
        .filter(|run| matches!(run.result, RunResult::Ok(_)))
        .count();
    eprintln!(
        "ROUTING REAL: total={} success={} no_route={} route_too_long={}",
        runs.len(),
        total_success,
        runs.iter()
            .filter(|run| matches!(run.result, RunResult::Allowed(EngineError::NoRoute)))
            .count(),
        runs.iter()
            .filter(|run| matches!(run.result, RunResult::Allowed(EngineError::RouteTooLong)))
            .count()
    );
    assert!(
        failures.is_empty(),
        "契約の検査に失敗した問い合わせが{}件ある",
        failures.len()
    );
}
