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
//!
//! 新方式では、`safest` が必ず選ばれることと、帰属不明の事故点が経路から72m以内に
//! あること（距離の計算の差を見込んで1mの余裕を持たせる）も確かめる。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use accident_route_backend::api::route_response::{
    NAV_MAX_GUIDANCE_STEPS, check_detour, render_route_response,
};
use accident_route_backend::contract::{
    Attribution, CancelToken, ConditionFilter, DayType, EngineError, LatLon, LonLat, Maneuver,
    Mode, Period, RouteEngine, RouteOutcome, RouteQuery, Router, Weather,
};
use accident_route_backend::data;
use accident_route_backend::testing::fixtures::check_outcome_invariants;
use accident_route_backend::testing::{RouterResponse, ScriptedRouter};
use serde::Deserialize;

const QUERIES_JSON: &str = include_str!("fixtures/routing-kanto4-queries.json");
const EXPECTED_QUERIES: usize = 300;
const WORKERS: usize = 4;
const QUERY_DEADLINE: Duration = Duration::from_secs(35);
/// 帰属不明の事故を数える範囲（72m）に、距離の計算の差を見込んだ余裕を足した値。
const UNATTRIBUTED_LIMIT_M: f64 = 73.0;

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
    guidance_steps: usize,
    guidance_bytes: usize,
    render_bytes: usize,
    /// 全候補の事故が0件で、安全ルートを選んだ。
    selected_without_accidents: bool,
    /// 全候補の事故が0件で、最短ルートと異なる安全ルートを選んだ。
    selected_without_accidents_differs: bool,
    /// 安全ルートの事故件数が最短ルートより多い。
    count_inverted: bool,
    /// 事故点の結び付けの内訳（edge・intersection・unattributed）。
    attributions: [u64; 3],
    max_unattributed_distance_m: f64,
}

/// 点から折れ線までの距離（m）。経路の付近で平面に近似する。
fn distance_to_path_m(point: LonLat, path: &[LonLat]) -> f64 {
    const METERS_PER_DEGREE: f64 = 111_320.0;
    let cos_lat = point.lat.to_radians().cos();
    let xy = |p: &LonLat| {
        (
            (p.lon - point.lon) * METERS_PER_DEGREE * cos_lat,
            (p.lat - point.lat) * METERS_PER_DEGREE,
        )
    };
    path.windows(2)
        .map(|pair| {
            let (ax, ay) = xy(&pair[0]);
            let (bx, by) = xy(&pair[1]);
            let (dx, dy) = (bx - ax, by - ay);
            let squared = dx * dx + dy * dy;
            let t = if squared > 0.0 {
                (-(ax * dx + ay * dy) / squared).clamp(0.0, 1.0)
            } else {
                0.0
            };
            (ax + t * dx).hypot(ay + t * dy)
        })
        .fold(f64::INFINITY, f64::min)
}

/// 新方式だけの検査と、選定・結び付けの集計。
fn check_new_engine(
    outcome: &RouteOutcome,
    render_bytes: usize,
    reasons: &mut Vec<String>,
) -> OutcomeStats {
    let mut attributions = [0u64; 3];
    let mut max_unattributed_distance_m = 0f64;
    for candidate in &outcome.candidates {
        for point in &candidate.accident_points {
            match point.attribution {
                Attribution::Edge => attributions[0] += 1,
                Attribution::Intersection => attributions[1] += 1,
                Attribution::Unattributed => {
                    attributions[2] += 1;
                    let distance = distance_to_path_m(point.position, &candidate.path.coordinates);
                    max_unattributed_distance_m = max_unattributed_distance_m.max(distance);
                    if distance > UNATTRIBUTED_LIMIT_M {
                        reasons.push(format!(
                            "帰属不明の事故{}が経路から{distance:.1}m離れている",
                            point.id
                        ));
                    }
                }
            }
        }
    }
    let no_accidents = outcome
        .candidates
        .iter()
        .all(|candidate| candidate.totals.accident_count == 0);
    let fastest = &outcome.candidates[outcome.fastest];
    let safest = outcome.safest;
    if safest.is_none() {
        reasons.push("新方式でsafestがnull".to_owned());
    }
    let (guidance_steps, guidance_bytes) = outcome
        .candidates
        .iter()
        .filter_map(|candidate| candidate.navigation.as_ref())
        .fold((0usize, 0usize), |(steps, bytes), nav| {
            (steps + nav.guidance.len(), bytes + guidance_bytes(nav))
        });
    OutcomeStats {
        candidates: outcome.candidates.len(),
        safest_is_none: safest.is_none(),
        high_risk_total: outcome
            .candidates
            .iter()
            .map(|candidate| candidate.high_risk_total)
            .sum(),
        guidance_steps,
        guidance_bytes,
        render_bytes,
        selected_without_accidents: no_accidents && safest.is_some(),
        selected_without_accidents_differs: no_accidents
            && safest.is_some_and(|index| index != outcome.fastest),
        count_inverted: safest.is_some_and(|index| {
            outcome.candidates[index].totals.accident_count > fastest.totals.accident_count
        }),
        attributions,
        max_unattributed_distance_m,
    }
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

#[derive(serde::Serialize)]
struct GuidanceStepBytes<'a> {
    maneuver: &'a str,
    at_m: f64,
}

/// 応答の `guidance` と同じ形にしたときのバイト数。
fn guidance_bytes(nav: &accident_route_backend::contract::NavigationData) -> usize {
    let steps: Vec<GuidanceStepBytes<'_>> = nav
        .guidance
        .iter()
        .map(|step| GuidanceStepBytes {
            maneuver: step.maneuver.as_str(),
            at_m: step.at_m,
        })
        .collect();
    serde_json::to_vec(&steps)
        .expect("案内はJSONにできる")
        .len()
}

/// 選定経路の `guidance` の不変条件（計画書3章・behavior.md 7章）。
fn check_guidance(outcome: &RouteOutcome) -> Result<(), String> {
    for (i, candidate) in outcome.candidates.iter().enumerate() {
        let Some(nav) = &candidate.navigation else {
            continue;
        };
        let id = format!("route-{}", i + 1);
        if nav.guidance.is_empty() {
            return Err(format!("{id}: guidanceが空"));
        }
        if nav.guidance.len() > NAV_MAX_GUIDANCE_STEPS {
            return Err(format!("{id}: 案内地点数が範囲外"));
        }
        let mut previous_at_m = 0.0;
        for (j, step) in nav.guidance.iter().enumerate() {
            if !step.at_m.is_finite() || step.at_m < 0.0 || step.at_m > nav.length_m {
                return Err(format!("{id}: 案内地点の距離が不正"));
            }
            if step.at_m < previous_at_m {
                return Err(format!("{id}: 案内地点が逆行している"));
            }
            if step.maneuver == Maneuver::Arrive && j + 1 != nav.guidance.len() {
                return Err(format!("{id}: arriveが末尾以外にある"));
            }
            previous_at_m = step.at_m;
        }
        let last = nav.guidance.last().expect("空でない");
        if last.maneuver != Maneuver::Arrive || last.at_m != nav.length_m {
            return Err(format!("{id}: 末尾のarriveがlength_mと一致しない"));
        }
    }
    Ok(())
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
            if let Err(reason) = check_guidance(&outcome) {
                reasons.push(format!("check_guidance: {reason}"));
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
            let stats = check_new_engine(&outcome, render_bytes.unwrap_or(0), &mut reasons);
            if reasons.is_empty() {
                RunResult::Ok(stats)
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
    let mut guidance_steps = 0usize;
    let mut guidance_bytes = 0usize;
    let mut max_render_bytes = 0usize;
    let mut selected_without_accidents = 0usize;
    let mut selected_without_accidents_differs = 0usize;
    let mut count_inverted = 0usize;
    let mut attributions = [0u64; 3];
    let mut max_unattributed_distance_m = 0f64;
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
                guidance_steps += stats.guidance_steps;
                guidance_bytes += stats.guidance_bytes;
                max_render_bytes = max_render_bytes.max(stats.render_bytes);
                selected_without_accidents += usize::from(stats.selected_without_accidents);
                selected_without_accidents_differs +=
                    usize::from(stats.selected_without_accidents_differs);
                count_inverted += usize::from(stats.count_inverted);
                for (total, count) in attributions.iter_mut().zip(stats.attributions) {
                    *total += count;
                }
                max_unattributed_distance_m =
                    max_unattributed_distance_m.max(stats.max_unattributed_distance_m);
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
         high_risk_total={high_risk_total} guidance_steps={guidance_steps} \
         guidance_bytes={guidance_bytes} max_render_bytes={max_render_bytes}",
        mode.as_str()
    );
    eprintln!(
        "ROUTING REAL: mode={} all_zero_selected={selected_without_accidents} \
         all_zero_differs={selected_without_accidents_differs} count_inverted={count_inverted} \
         attribution(edge,intersection,unattributed)={attributions:?} \
         max_unattributed_m={max_unattributed_distance_m:.1}",
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
