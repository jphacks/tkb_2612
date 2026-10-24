//! 実データのreleaseの確認（#8 ステップ16）。既定では `ignored` として数え、
//! `ACCIDENT_REAL_RELEASE_DIR` を指定して `--ignored` で実行する。
//!
//! ```sh
//! ACCIDENT_REAL_RELEASE_DIR=backend/data/releases/<release_id> \
//!     cargo test --release --manifest-path backend/Cargo.toml --test real_release -- --ignored --nocapture
//! ```
//!
//! ロード時間と固定検索（東京駅付近の人工の固定経路）の結果を表示する。合否の基準は
//! 起動時の検証（ID・件数・列挙値）と相関制約だけで、件数そのものは原本の版に依存する。

use std::time::Instant;

use accident_route_backend::contract::{
    CancelToken, ConditionFilter, DayType, LatLon, Mode, Period, RouteQuery, RoutedPath,
};
use accident_route_backend::data;
use accident_route_backend::testing::fixtures::check_outcome_invariants;
use accident_route_backend::testing::{ScriptedRouter, valid_routes};

#[tokio::test]
#[ignore = "実releaseが必要（ACCIDENT_REAL_RELEASE_DIR を指定して --ignored で実行）"]
async fn real_release_loads_and_answers_a_fixed_search() {
    let dir = std::env::var("ACCIDENT_REAL_RELEASE_DIR")
        .expect("ACCIDENT_REAL_RELEASE_DIR に実releaseのディレクトリを指定してください");
    let dir = std::fs::canonicalize(dir).expect("releaseディレクトリ");
    let started = Instant::now();
    let calendar = data::load_calendar(&dir).expect("祝日表");
    let calendar_s = started.elapsed().as_secs_f64();
    let engine = data::load_engine(&dir).expect("事故データ");
    let info = engine.dataset();
    eprintln!(
        "REAL: release_id={} dataset_id={} accidents={} data_years={:?} grid_cells={:?}",
        info.release_id, info.dataset_id, info.accidents, info.data_years, info.grid_cells
    );
    eprintln!(
        "REAL: load_calendar {calendar_s:.3}s, load_engine load_seconds={:.3}",
        info.load_seconds
    );
    drop(calendar);

    // 東京駅→日本橋付近の人工の固定経路（実OSRMではない）
    let route = |duration_s: f64, mid_lat: f64| RoutedPath {
        duration_s,
        distance_m: 1500.0,
        coordinates: [
            (139.7671, 35.6812),
            (139.7710, mid_lat),
            (139.7745, 35.6840),
        ]
        .map(|(lon, lat)| accident_route_backend::contract::LonLat::new(lon, lat))
        .to_vec(),
    };
    for (mode, conditions) in [
        (Mode::Walk, ConditionFilter::default()),
        (
            Mode::Car,
            ConditionFilter {
                period: Some(Period::Daytime),
                day_type: Some(DayType::Weekday),
                weather: None,
            },
        ),
    ] {
        let query = RouteQuery::new(
            LatLon::new(35.6812, 139.7671),
            LatLon::new(35.6840, 139.7745),
            mode,
            10.0,
            conditions,
        )
        .unwrap();
        let router = ScriptedRouter::new([valid_routes(&[
            route(1200.0, 35.6830),
            route(1260.0, 35.6845),
            route(1320.0, 35.6815),
        ])]);
        let cancel = CancelToken::unbounded();
        let candidates = engine
            .collect_candidates(&router, &query, &cancel)
            .await
            .unwrap();
        let outcome = engine.evaluate(&query, candidates, &cancel).unwrap();
        check_outcome_invariants(&outcome).unwrap();
        eprintln!(
            "REAL: {mode:?} counts={:?} weighted_risk={:?} fastest={} safest={:?} aggregation_ms={}",
            outcome
                .candidates
                .iter()
                .map(|c| c.totals.accident_count)
                .collect::<Vec<_>>(),
            outcome
                .candidates
                .iter()
                .map(|c| c.weighted_risk)
                .collect::<Vec<_>>(),
            outcome.fastest,
            outcome.safest,
            outcome.aggregation_ms
        );
    }
}
