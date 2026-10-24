//! 人工release＋実エンジン＋固定経路の結果が、openapi.yamlの応答例と一致すること
//! （behavior.md 11章 項目14、#8 ステップ13）。
//!
//! 比較規則は C3（#7〜#9 共通前提4章）と同じ：
//! - `dataset_id` は64桁の小文字hexで `engine.dataset()` と等しいことを確かめてから0埋めへ置換
//! - `aggregation_ms` は有限の非負数であることを確かめてから値の比較から除く
//! - 座標（経路・事故点・高リスク区間の補間座標）は絶対誤差1e-10度、距離は1e-6m
//! - それ以外（丸め済みの値・件数・順序・ID・選定・文言）は完全一致
//!
//! 例の名前は固定せず、fixtureの全例を対象にする。

use std::sync::Arc;

use accident_route_backend::contract::{CancelToken, LonLat, RouteEngine, RouteOutcome};
use accident_route_backend::data;
use accident_route_backend::testing::fixtures::{
    check_outcome_invariants, expected_route_outcome, route_example, route_response_names,
};
use accident_route_backend::testing::{TestRelease, example_router};

const COORDINATE_TOLERANCE_DEG: f64 = 1e-10;
const DISTANCE_TOLERANCE_M: f64 = 1e-6;

fn snap(actual: &mut f64, expected: f64, tolerance: f64) {
    if (*actual - expected).abs() <= tolerance {
        *actual = expected;
    }
}

fn snap_point(actual: &mut LonLat, expected: LonLat) {
    snap(&mut actual.lon, expected.lon, COORDINATE_TOLERANCE_DEG);
    snap(&mut actual.lat, expected.lat, COORDINATE_TOLERANCE_DEG);
}

fn snap_points(actual: &mut [LonLat], expected: &[LonLat]) {
    for (a, e) in actual.iter_mut().zip(expected) {
        snap_point(a, *e);
    }
}

/// 許容差の範囲の差を期待値に寄せ、識別子と計測値を置き換えたもの。
/// 残りの差は `assert_eq!` で全体として表示する。
fn normalize(
    mut actual: RouteOutcome,
    expected: &RouteOutcome,
    engine: &dyn RouteEngine,
) -> RouteOutcome {
    let id = &actual.dataset_id;
    assert_eq!(id.len(), 64, "dataset_idは64桁");
    assert!(
        id.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "dataset_idは小文字hex: {id}"
    );
    assert_eq!(id, &engine.dataset().dataset_id);
    actual.dataset_id = expected.dataset_id.clone();

    assert!(
        actual.aggregation_ms.is_finite() && actual.aggregation_ms >= 0.0,
        "aggregation_msは有限の非負数: {}",
        actual.aggregation_ms
    );
    actual.aggregation_ms = expected.aggregation_ms;

    for (a, e) in actual.candidates.iter_mut().zip(&expected.candidates) {
        snap_points(&mut a.path.coordinates, &e.path.coordinates);
        snap(
            &mut a.path.distance_m,
            e.path.distance_m,
            DISTANCE_TOLERANCE_M,
        );
        for (p, q) in a.accident_points.iter_mut().zip(&e.accident_points) {
            snap_point(&mut p.position, q.position);
        }
        for (s, t) in a.high_risk_sections.iter_mut().zip(&e.high_risk_sections) {
            snap_point(&mut s.start, t.start);
            snap_point(&mut s.end, t.end);
        }
        if let (Some(n), Some(m)) = (a.navigation.as_mut(), e.navigation.as_ref()) {
            snap(
                &mut n.projection_lat_deg,
                m.projection_lat_deg,
                COORDINATE_TOLERANCE_DEG,
            );
            snap(&mut n.length_m, m.length_m, DISTANCE_TOLERANCE_M);
            for (d, f) in n.vertex_distances_m.iter_mut().zip(&m.vertex_distances_m) {
                snap(d, *f, DISTANCE_TOLERANCE_M);
            }
            for (s, t) in n.sections.iter_mut().zip(&m.sections) {
                snap(&mut s.start_m, t.start_m, DISTANCE_TOLERANCE_M);
                snap(&mut s.end_m, t.end_m, DISTANCE_TOLERANCE_M);
            }
        }
    }
    actual
}

async fn run_example(name: &str) {
    let example = route_example(name);
    let release = TestRelease::from_example(&example);
    let calendar = data::load_calendar(release.path()).expect("人工releaseの祝日表");
    drop(calendar);
    let engine: Arc<dyn RouteEngine> = data::load_engine(release.path()).expect("人工release");
    assert_eq!(engine.dataset().dataset_id, release.ids.dataset_id);
    assert_eq!(engine.dataset().release_id, release.ids.release_id);

    let router = example_router(&example);
    let cancel = CancelToken::unbounded();
    let candidates = engine
        .collect_candidates(&router, &example.query, &cancel)
        .await
        .expect("候補取得");
    // 上流呼出しは最大3回（ナビの有無で増えない）
    let calls = router.calls();
    assert!(calls.len() <= 3, "{name}: 上流呼出し{}回", calls.len());
    assert_eq!(router.remaining(), 0, "{name}: 用意した応答を使い切る");
    let outcome = engine
        .evaluate(&example.query, candidates, &cancel)
        .expect("評価");

    check_outcome_invariants(&outcome).unwrap_or_else(|e| panic!("{name}: {e}"));
    let expected = expected_route_outcome(name);
    let normalized = normalize(outcome, &expected, engine.as_ref());
    assert_eq!(normalized, expected, "{name}: 期待値と一致しない");
}

#[tokio::test]
async fn every_fixture_example_matches_expected_outcome() {
    let names = route_response_names();
    assert!(!names.is_empty());
    for name in names {
        run_example(&name).await;
    }
}

#[tokio::test]
async fn same_input_gives_identical_outcome_twice() {
    for name in route_response_names() {
        let example = route_example(&name);
        let release = TestRelease::from_example(&example);
        let engine = data::load_engine(release.path()).unwrap();
        let cancel = CancelToken::unbounded();
        let mut outcomes = Vec::new();
        for _ in 0..2 {
            let router = example_router(&example);
            let candidates = engine
                .collect_candidates(&router, &example.query, &cancel)
                .await
                .unwrap();
            let mut outcome = engine
                .evaluate(&example.query, candidates, &cancel)
                .unwrap();
            outcome.aggregation_ms = 0.0;
            outcomes.push(outcome);
        }
        assert_eq!(outcomes[0], outcomes[1], "{name}");
    }
}
