//! 最短時間・事故考慮経路の選定と文言（behavior.md 8章、11章 SpatialAndSelectionTests、#8 ステップ13）。

mod common;

use accident_route_backend::contract::{
    CancelToken, CandidateSet, ConditionFilter, Mode, RouteEngine, RouteOutcome, RoutedPath,
    Weather,
};
use accident_route_backend::domain::engine::{
    NO_ACCIDENT_REASON, NO_ACCIDENT_WARNING, SAME_ROUTE_REASON, ZERO_BASELINE_REASON, risk_note,
};
use accident_route_backend::testing::fixtures::check_outcome_invariants;

use common::{Acc, engine, path, query};

const KX35: f64 =
    accident_route_backend::contract::geo::METERS_PER_DEGREE * 0.819_152_044_288_991_8;

/// 緯度 `35+lane*0.01` の東西1000mの経路（車線ごとに約1.1km離れる）。
fn lane(lane: u32, duration_s: f64, distance_m: f64) -> RoutedPath {
    let lat = 35.0 + 0.01 * f64::from(lane);
    path(
        duration_s,
        distance_m,
        &[(139.0, lat), (139.0 + 1000.0 / KX35, lat)],
    )
}

/// 車線上（距離0、重み1）に深刻度 `severity`（負傷者数）の事故を置く。
fn on_lane(lane: u32, severity: u16) -> Acc {
    Acc::at(139.0 + 500.0 / KX35, 35.0 + 0.01 * f64::from(lane)).casualties(0, severity)
}

fn run(
    routes: Vec<RoutedPath>,
    accidents: &[Acc],
    max_detour_min: f64,
    weather: Option<Weather>,
    warnings: Vec<String>,
) -> RouteOutcome {
    let e = engine(accidents);
    let conditions = ConditionFilter {
        weather,
        ..Default::default()
    };
    let outcome = e
        .evaluate(
            &query(Mode::Walk, max_detour_min, conditions),
            CandidateSet { routes, warnings },
            &CancelToken::unbounded(),
        )
        .unwrap();
    check_outcome_invariants(&outcome).unwrap();
    outcome
}

#[test]
fn test_safest_obeys_detour_and_uses_weighted_total() {
    // 600/900/1300秒、許容5分。1300秒は許容外なのでリスク最小でも選ばない
    let routes = vec![
        lane(0, 600.0, 1000.0),
        lane(1, 900.0, 1200.0),
        lane(2, 1300.0, 1500.0),
    ];
    let accidents = [on_lane(0, 10), on_lane(1, 4), on_lane(2, 1)];
    let o = run(routes.clone(), &accidents, 5.0, None, vec![]);
    assert_eq!(o.fastest, 0);
    assert_eq!(o.safest, Some(1));
    assert_eq!(
        o.candidates
            .iter()
            .map(|c| c.within_detour)
            .collect::<Vec<_>>(),
        [true, true, false]
    );
    assert_eq!(
        o.candidates
            .iter()
            .map(|c| c.extra_minutes)
            .collect::<Vec<_>>(),
        [0.0, 5.0, 11.667]
    );
    assert_eq!(o.extra_minutes, Some(5.0));
    assert_eq!(o.risk_reduction_percent, Some(60.0));
    assert_eq!(
        o.reason,
        "取得した3候補のうち、最短時間経路から+5分以内で、距離で重み付けしたリスクが最小の経路を選びました。追加5.0分、リスク60.0%減です。"
    );
    assert!(o.candidates[0].navigation.is_some() && o.candidates[1].navigation.is_some());
    assert!(o.candidates[2].navigation.is_none());

    // 許容0分ならfastest
    let o = run(routes, &accidents, 0.0, None, vec![]);
    assert_eq!(o.safest, Some(0));
    assert_eq!(o.reason, SAME_ROUTE_REASON);
    assert_eq!(
        o.risk_reduction_percent,
        Some(0.0),
        "同一経路で基準リスク正なら0%"
    );
}

#[test]
fn detour_limit_includes_epsilon() {
    // 許容は丸め前の duration <= fastest + max_detour*60 + 1e-6 秒
    let routes = vec![
        lane(0, 600.0, 1000.0),
        lane(1, 900.000_000_5, 1000.0),
        lane(2, 900.000_002, 1000.0),
    ];
    let accidents = [on_lane(0, 10), on_lane(1, 5), on_lane(2, 1)];
    let o = run(routes, &accidents, 5.0, None, vec![]);
    assert_eq!(
        o.candidates
            .iter()
            .map(|c| c.within_detour)
            .collect::<Vec<_>>(),
        [true, true, false]
    );
    assert_eq!(o.safest, Some(1));
}

#[test]
fn ties_use_rounded_risk_then_duration_then_distance_then_order() {
    // 丸めたリスクが同じなら所要時間、同じなら距離、完全同値は候補順先頭
    let accidents = [on_lane(0, 9), on_lane(1, 3), on_lane(2, 3), on_lane(3, 3)];
    let o = run(
        vec![
            lane(0, 600.0, 1000.0),
            lane(1, 700.0, 1100.0),
            lane(2, 650.0, 1200.0),
            lane(3, 650.0, 1150.0),
        ],
        &accidents,
        10.0,
        None,
        vec![],
    );
    assert_eq!(o.safest, Some(3));
    let o = run(
        vec![
            lane(0, 600.0, 1000.0),
            lane(1, 650.0, 1100.0),
            lane(2, 650.0, 1100.0),
        ],
        &accidents[..3],
        10.0,
        None,
        vec![],
    );
    assert_eq!(o.safest, Some(1));
    // fastestは (duration, distance) の辞書順。同値は候補順先頭
    let o = run(
        vec![
            lane(0, 600.0, 1200.0),
            lane(1, 600.0, 1100.0),
            lane(2, 600.0, 1100.0),
        ],
        &accidents[..3],
        10.0,
        None,
        vec![],
    );
    assert_eq!(o.fastest, 1);
}

#[test]
fn rounded_risk_tie_is_broken_by_duration_not_raw_score() {
    // 生のスコアは 3.0（車線1）> 2.996（車線2）だが、2桁に丸めるとどちらも3.00。
    // 丸めた値で比べるので、所要時間の短い車線1を選ぶ（生のスコアなら車線2になる）
    let sigma = 24.0f64;
    let north = (-2.0 * sigma * sigma * (2.996f64 / 3.0).ln()).sqrt(); // 3×exp(−d²/2σ²)=2.996
    let mut slightly_off = on_lane(2, 3);
    slightly_off.lat += north / accident_route_backend::contract::geo::METERS_PER_DEGREE;
    let o = run(
        vec![
            lane(0, 600.0, 1000.0),
            lane(1, 650.0, 1000.0),
            lane(2, 700.0, 1000.0),
        ],
        &[on_lane(0, 10), on_lane(1, 3), slightly_off],
        10.0,
        None,
        vec![],
    );
    assert_eq!(o.candidates[1].weighted_risk, 3.0);
    assert_eq!(o.candidates[2].weighted_risk, 3.0);
    assert_eq!(
        o.candidates[2].totals.accident_count, 1,
        "2.996でBに入っている"
    );
    assert_eq!(o.safest, Some(1));
    // 削減率も丸めた値から計算する：100×(10−3)/10
    assert_eq!(o.risk_reduction_percent, Some(70.0));
}

#[test]
fn test_time_fallback_and_all_zero_have_no_false_percentage() {
    // 全候補で集合Bが空：safest・extra_minutes・削減率はnull、警告を加える
    let o = run(
        vec![lane(0, 600.0, 1000.0), lane(1, 700.0, 1000.0)],
        &[],
        10.0,
        Some(Weather::Rain),
        vec!["追加の警告".to_owned()],
    );
    assert_eq!(o.safest, None);
    assert_eq!(o.extra_minutes, None);
    assert_eq!(o.risk_reduction_percent, None);
    assert_eq!(o.reason, NO_ACCIDENT_REASON);
    assert_eq!(
        o.warnings,
        ["追加の警告", NO_ACCIDENT_WARNING],
        "候補取得の警告の後に続ける"
    );
    assert!(o.candidates[0].navigation.is_some());
    assert!(o.candidates[1].navigation.is_none());
    // 天候一致は全項目0で、条件を解除しない
    assert_eq!(o.candidates[0].weather_match.unwrap().accident_count, 0);
    assert_eq!(o.conditions.weather, Some(Weather::Rain));
}

#[test]
fn test_no_relaxation_if_another_candidate_has_exact_match() {
    // 最短時間経路のスコアが0で他候補にBがある：safest=fastest、削減率null
    let o = run(
        vec![lane(0, 600.0, 1000.0), lane(1, 700.0, 1000.0)],
        &[on_lane(1, 4)],
        10.0,
        None,
        vec![],
    );
    assert_eq!(o.safest, Some(0));
    assert_eq!(o.risk_reduction_percent, None);
    assert_eq!(o.extra_minutes, Some(0.0));
    assert_eq!(
        o.reason,
        format!("{SAME_ROUTE_REASON}{ZERO_BASELINE_REASON}")
    );
    assert!(o.warnings.is_empty());
}

#[test]
fn reason_formats_detour_minutes_and_rounding() {
    let o = run(
        vec![lane(0, 600.0, 1000.0), lane(1, 700.0, 1000.0)],
        &[on_lane(0, 3), on_lane(1, 1)],
        7.5,
        None,
        vec![],
    );
    // 追加100秒=1.667分→「1.7」、削減率66.7%
    assert_eq!(o.extra_minutes, Some(1.667));
    assert_eq!(o.risk_reduction_percent, Some(66.7));
    assert_eq!(
        o.reason,
        "取得した2候補のうち、最短時間経路から+7.5分以内で、距離で重み付けしたリスクが最小の経路を選びました。追加1.7分、リスク66.7%減です。"
    );
}

#[test]
fn risk_note_mentions_weather_only_for_rain_fog_snow() {
    for (weather, has) in [
        (None, false),
        (Some(Weather::Clear), false),
        (Some(Weather::Cloudy), false),
        (Some(Weather::Rain), true),
        (Some(Weather::Fog), true),
        (Some(Weather::Snow), true),
    ] {
        let note = risk_note(weather);
        assert!(note.starts_with("過去の事故件数に基づく参考指標です。"));
        assert!(note.ends_with("交通量や通行回数で補正した事故確率ではありません。"));
        assert_eq!(
            note.contains("雨・霧・雪を指定した場合"),
            has,
            "{weather:?}"
        );
    }
}

#[test]
fn outcome_uses_the_loaded_snapshot() {
    let e = engine(&[on_lane(0, 3)]);
    let o = e
        .evaluate(
            &query(Mode::Walk, 10.0, ConditionFilter::default()),
            CandidateSet {
                routes: vec![lane(0, 600.0, 1000.0)],
                warnings: vec![],
            },
            &CancelToken::unbounded(),
        )
        .unwrap();
    assert_eq!(o.dataset_id, e.dataset().dataset_id);
    assert_eq!(o.data_years, e.dataset().data_years);
    assert!(o.aggregation_ms.is_finite() && o.aggregation_ms >= 0.0);
    // 小数2桁
    assert_eq!(
        o.aggregation_ms,
        accident_route_backend::domain::py_round(o.aggregation_ms, 2)
    );
}
