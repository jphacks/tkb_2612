//! 人工releaseの生成（②の実エンジン試験、③のC3・C4用）。
//!
//! `route_example(name)` の `accident_ids` の事故だけを、前処理と同じ書き出し
//! （`data::release`）で17列のreleaseにする。IDの計算を二重に持たない。
//! `dataset_id`・`release_id` は実際のハッシュで、fixtureの0埋めとは異なる。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::NaiveDate;
use serde_json::{Map, json};

use crate::contract::{Calendar, Period};
use crate::data::holidays::parse_holiday_csv;
use crate::data::preprocess::rows::AccidentRecord;
use crate::data::release::{AccidentCsvWriter, build_release, write_release_files};

use super::fixtures::{RouteExample, route_example, route_example_accidents};
use super::router::{RouterResponse, ScriptedRouter, valid_routes};

/// 人工releaseに同梱する祝日表（2025年の国民の祝日・休日）。
///
/// 収録範囲は2025-01-01〜2025-12-31。範囲外（2024年以前・2026年以降）は土日と年末年始だけで
/// 判定され、`holiday_table_covered=false` になる。③がC3で `/api/conditions` を確かめるときは、
/// 範囲内（例：2025-11-24は振替休日、2025-05-06は振替休日）と範囲外（例：2026-01-05）の
/// 両方の日付を選べる。
pub const TEST_HOLIDAY_CSV: &str = "\
国民の祝日・休日月日,国民の祝日・休日名称
2025/1/1,元日
2025/1/13,成人の日
2025/2/11,建国記念の日
2025/2/23,天皇誕生日
2025/2/24,休日
2025/3/20,春分の日
2025/4/29,昭和の日
2025/5/3,憲法記念日
2025/5/4,みどりの日
2025/5/5,こどもの日
2025/5/6,休日
2025/7/21,海の日
2025/8/11,山の日
2025/9/15,敬老の日
2025/9/23,秋分の日
2025/10/13,スポーツの日
2025/11/3,文化の日
2025/11/23,勤労感謝の日
2025/11/24,休日
";

/// metadataの `generated_at`（人工releaseでは固定）。IDには影響しない。
const GENERATED_AT: &str = "2025-01-01T00:00:00+09:00";

/// 作ったreleaseの識別子。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseIds {
    pub dataset_id: String,
    pub release_id: String,
}

/// 時間帯の代表の時。
fn representative_hour(period: Period) -> u8 {
    match period {
        Period::EarlyMorning => 6,
        Period::Daytime => 12,
        Period::Evening => 17,
        Period::Night => 21,
        Period::LateNight => 2,
    }
}

/// 例の事故を17列の行にする。事故IDは行番号なので、`accident_ids` が `0..n` でなければ失敗。
pub fn example_records(
    example: &RouteExample,
    calendar: &dyn Calendar,
) -> Result<Vec<AccidentRecord>, String> {
    let ids: Vec<u64> = (0..example.accident_ids.len() as u64).collect();
    if example.accident_ids != ids {
        return Err(format!(
            "{}: accident_idsが0から始まる連番ではありません: {:?}",
            example.name, example.accident_ids
        ));
    }
    let accidents = route_example_accidents();
    example
        .accident_ids
        .iter()
        .map(|&id| {
            let a = accidents
                .iter()
                .find(|a| a.point.id == id)
                .ok_or_else(|| format!("{}: 事故ID{id}がfixtureにありません", example.name))?;
            let p = &a.point;
            let date = NaiveDate::from_ymd_opt(
                (p.date / 10_000) as i32,
                (p.date / 100) % 100,
                p.date % 100,
            )
            .ok_or_else(|| format!("事故ID{id}の日付が不正です"))?;
            let hour = representative_hour(p.period);
            Ok(AccidentRecord {
                lat: p.position.lat,
                lon: p.position.lon,
                modes: a.modes,
                period: p.period,
                weather: p.weather,
                // fixtureの曜日区分をそのまま書く（祝日表で再判定しない）
                day_type: p.day_type,
                deaths: p.deaths,
                injuries: p.injuries,
                source_year: p.source_year,
                date,
                hour,
                minute: 0,
                daynight: if (6..18).contains(&hour) { 12 } else { 22 },
                accident_type: 1,
                injury_a: if p.deaths > 0 { 1 } else { 2 },
                injury_b: 0,
                holiday_table_covered: calendar.classify_date(date).holiday_table_covered,
            })
        })
        .collect()
}

/// 例の人工releaseを `dir` に書き出す（ディレクトリは無ければ作る）。
pub fn write_example_release(example: &RouteExample, dir: &Path) -> std::io::Result<ReleaseIds> {
    let invalid = |m: String| std::io::Error::new(std::io::ErrorKind::InvalidInput, m);
    let holidays = TEST_HOLIDAY_CSV.as_bytes().to_vec();
    let calendar = parse_holiday_csv(&holidays).map_err(|e| invalid(e.to_string()))?;
    let mut writer = AccidentCsvWriter::new();
    for record in example_records(example, &calendar).map_err(invalid)? {
        writer.push(&record);
    }
    let mut extra = Map::new();
    extra.insert("test_example".to_owned(), json!(example.name));
    let content = build_release(writer.finish(), holidays, &calendar, GENERATED_AT, extra);
    write_release_files(dir, &content)?;
    Ok(ReleaseIds {
        dataset_id: content.dataset_id,
        release_id: content.release_id,
    })
}

/// 一時ディレクトリに作った人工release。破棄時にディレクトリごと消える。
#[derive(Debug)]
pub struct TestRelease {
    dir: PathBuf,
    pub ids: ReleaseIds,
}

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

impl TestRelease {
    /// `route_example(name)` の人工release。
    pub fn for_example(name: &str) -> Self {
        Self::from_example(&route_example(name))
    }

    pub fn from_example(example: &RouteExample) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "accident-route-release-{}-{}-{nanos}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed),
        ));
        let ids = write_example_release(example, &dir)
            .unwrap_or_else(|e| panic!("人工releaseを作れません: {e}"));
        Self { dir, ids }
    }

    /// `data::load_calendar`・`data::load_engine` に渡すディレクトリ。
    pub fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for TestRelease {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 例ごとの `ScriptedRouter`。初回は固定経路を返し、初回の先頭6本を重複排除して3本未満の例では
/// 追加の2回にも初回と同じ経路を返す（重複排除で候補は増えず、追加取得エラーの警告も出ない）。
/// 本数はエンジンと同じ基準（5桁fingerprintでの重複排除後）で数える。
///
/// ②のエンジン試験と③のC3が同じものを使う（#7〜#9 共通前提2章「C3の固定経路」）。
pub fn example_router(example: &RouteExample) -> ScriptedRouter {
    let initial = valid_routes(&example.routes);
    let mut responses: Vec<RouterResponse> = vec![initial];
    let distinct: std::collections::BTreeSet<_> = example
        .routes
        .iter()
        .take(crate::contract::routing::MAX_ROUTES_PER_CALL)
        .map(crate::domain::candidates::fingerprint)
        .collect();
    if distinct.len() < crate::domain::candidates::MIN_INITIAL_CANDIDATES {
        responses.push(valid_routes(&example.routes));
        responses.push(valid_routes(&example.routes));
    }
    ScriptedRouter::new(responses)
}
