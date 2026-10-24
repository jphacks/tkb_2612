//! 集合B・件数・スコア・事故点・高リスク区間（behavior.md 7章、11章 追加試験9、#8 ステップ11）。

mod common;

use accident_route_backend::contract::{
    AccidentTotals, CancelToken, CandidateSet, ConditionFilter, DayType, Mode, Period, RouteEngine,
    RouteOutcome, Weather,
};
use accident_route_backend::domain::py_round;
use accident_route_backend::domain::scoring::{distance_weight, sample, weather_multiplier};

use common::{Acc, engine, lat_north_of_35, query, straight_east};

const KX35: f64 =
    accident_route_backend::contract::geo::METERS_PER_DEGREE * 0.819_152_044_288_991_8;

/// 東西1000m（distance 2000m）の1候補で評価する。
fn evaluate(accidents: &[Acc], mode: Mode, conditions: ConditionFilter) -> RouteOutcome {
    let e = engine(accidents);
    e.evaluate(
        &query(mode, 10.0, conditions),
        CandidateSet {
            routes: vec![straight_east(600.0, 2000.0, 1000.0)],
            warnings: Vec::new(),
        },
        &CancelToken::unbounded(),
    )
    .unwrap()
}

fn at_m(x: f64, north: f64) -> Acc {
    Acc::at(139.0 + x / KX35, lat_north_of_35(north))
}

#[test]
fn test_filters_casualties_normalization_and_hotspots() {
    let accidents = [
        at_m(100.0, 0.0).casualties(1, 2), // 0 一致・死亡
        at_m(100.0, 24.0).casualties(0, 3).weather(Weather::Rain), // 1 一致・雨
        at_m(500.0, 10.0).period(Period::Night), // 2 時間帯不一致
        at_m(500.0, 10.0).day_type(DayType::Holiday), // 3 曜日区分不一致
        at_m(500.0, 10.0).modes(Mode::Bike.mask()), // 4 手段不一致
        at_m(500.0, 80.0),                 // 5 72m超
        at_m(900.0, 0.0).casualties(0, 1).weather(Weather::Snow), // 6 一致・雪
    ];
    let conditions = ConditionFilter {
        period: Some(Period::Daytime),
        day_type: Some(DayType::Weekday),
        weather: Some(Weather::Rain),
    };
    let outcome = evaluate(&accidents, Mode::Walk, conditions);
    let c = &outcome.candidates[0];
    // 天候ではBを減らさない（雪の事故6もBに入る）
    assert_eq!(
        c.totals,
        AccidentTotals {
            accident_count: 3,
            fatal_accidents: 1,
            injury_accidents: 2,
            deaths: 1,
            injuries: 6
        }
    );
    assert_eq!(
        c.weather_match,
        Some(AccidentTotals {
            accident_count: 1,
            fatal_accidents: 0,
            injury_accidents: 1,
            deaths: 0,
            injuries: 3
        })
    );
    // S = 1×1×12 + exp(−24²/(2·24²))×2×3 + 1×1×1
    let s = 12.0 + (-0.5f64).exp() * 2.0 * 3.0 + 1.0;
    assert_eq!(c.weighted_risk, py_round(s, 2));
    assert_eq!(
        c.risk_per_km,
        py_round(py_round(s, 2) / 2.0, 3),
        "distance 2000mで割る"
    );
    // 小線分の深刻度（距離減衰・天候倍率なし）：sid 1 = 12+3、sid 12 = 1
    assert_eq!(c.high_risk_total, 1);
    assert_eq!(c.high_risk_sections.len(), 1);
    assert_eq!(c.high_risk_sections[0].segment_index, 1);
    assert_eq!(c.high_risk_sections[0].severity_weight, 15);
    assert_eq!(outcome.conditions, conditions, "条件を緩和しない");
}

#[test]
fn weather_multipliers() {
    for (requested, accident, m) in [
        (Some(Weather::Rain), Weather::Rain, 2.0),
        (Some(Weather::Fog), Weather::Fog, 4.0),
        (Some(Weather::Snow), Weather::Snow, 4.0),
        (Some(Weather::Clear), Weather::Clear, 1.0),
        (Some(Weather::Cloudy), Weather::Cloudy, 1.0),
        (Some(Weather::Rain), Weather::Snow, 1.0),
        (Some(Weather::Snow), Weather::Fog, 1.0),
        (None, Weather::Rain, 1.0),
    ] {
        assert_eq!(
            weather_multiplier(requested, accident.code()),
            m,
            "{requested:?} {accident:?}"
        );
    }
    assert_eq!(distance_weight(0.0), 1.0);
    assert!((distance_weight(72.0 * 72.0) - (-4.5f64).exp()).abs() < 1e-15);

    // 天候all：weather_matchはnull、倍率1
    let accidents = [at_m(100.0, 0.0).weather(Weather::Snow).casualties(0, 5)];
    let all = evaluate(&accidents, Mode::Walk, ConditionFilter::default());
    assert_eq!(all.candidates[0].weather_match, None);
    assert_eq!(all.candidates[0].weighted_risk, 5.0);
    // 雪指定：一致事故を4倍に数える
    let snow = evaluate(
        &accidents,
        Mode::Walk,
        ConditionFilter {
            weather: Some(Weather::Snow),
            ..Default::default()
        },
    );
    assert_eq!(snow.candidates[0].weighted_risk, 20.0);
    // 晴指定：不一致の事故も1倍で残り、weather_matchは全項目0
    let clear = evaluate(
        &accidents,
        Mode::Walk,
        ConditionFilter {
            weather: Some(Weather::Clear),
            ..Default::default()
        },
    );
    assert_eq!(clear.candidates[0].weighted_risk, 5.0);
    assert_eq!(
        clear.candidates[0].weather_match,
        Some(AccidentTotals::default())
    );
    assert_eq!(clear.candidates[0].totals.accident_count, 1);
}

#[test]
fn test_point_sampling_prioritizes_fatalities_without_affecting_totals() {
    // 400事故（死亡10件）→250点、死亡10件を全て含む
    let accidents: Vec<Acc> = (0..400)
        .map(|i| {
            let a = at_m(10.0 + 2.0 * i as f64, (i % 7) as f64);
            if i % 40 == 0 { a.casualties(1, 0) } else { a }
        })
        .collect();
    let outcome = evaluate(&accidents, Mode::Walk, ConditionFilter::default());
    let c = &outcome.candidates[0];
    assert_eq!(c.totals.accident_count, 400);
    assert_eq!(c.totals.fatal_accidents, 10);
    assert_eq!(c.accident_points.len(), 250);
    let fatal: Vec<u64> = c
        .accident_points
        .iter()
        .filter(|p| p.deaths > 0)
        .map(|p| p.id)
        .collect();
    assert_eq!(fatal, (0..10).map(|i| i * 40).collect::<Vec<_>>());
    // 先に死亡事故、続いて残り枠をそれ以外から等間隔に（事故ID昇順の元順）
    assert!(c.accident_points[..10].iter().all(|p| p.deaths > 0));
    let others: Vec<u64> = (0..400).filter(|i| i % 40 != 0).collect();
    let expected: Vec<u64> = sample(&others, 240);
    assert_eq!(
        c.accident_points[10..]
            .iter()
            .map(|p| p.id)
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn more_than_250_fatal_accidents_are_sampled() {
    let accidents: Vec<Acc> = (0..300)
        .map(|i| at_m(10.0 + 3.0 * i as f64, 0.0).casualties(1, 0))
        .collect();
    let outcome = evaluate(&accidents, Mode::Walk, ConditionFilter::default());
    let c = &outcome.candidates[0];
    assert_eq!(c.totals.fatal_accidents, 300);
    assert_eq!(c.accident_points.len(), 250);
    let ids: Vec<u64> = c.accident_points.iter().map(|p| p.id).collect();
    assert_eq!(ids, sample(&(0..300).collect::<Vec<u64>>(), 250));
}

#[test]
fn sample_rule() {
    assert!(sample(&[1, 2, 3], 0).is_empty());
    assert_eq!(sample(&[1, 2, 3], 3), [1, 2, 3]);
    assert_eq!(sample(&[0, 1, 2, 3, 4], 2), [0, 2]); // floor(i*5/2)
    assert_eq!(sample(&(0..10).collect::<Vec<_>>(), 4), [0, 2, 5, 7]);
}

#[test]
fn hotspots_over_100_and_weight_threshold() {
    // 30kmの直線（400本）に、sidごとの深刻度が2か3以上になるよう事故を置く
    let kx = KX35;
    let e = engine(
        &(0..400)
            .map(|sid| {
                let x = 75.0 * sid as f64 + 30.0;
                Acc::at(139.0 + x / kx, 35.0)
                    .casualties(0, if sid % 3 == 0 { 2 } else { 3 + sid as u16 % 4 })
            })
            .collect::<Vec<_>>(),
    );
    let outcome = e
        .evaluate(
            &query(Mode::Walk, 10.0, ConditionFilter::default()),
            CandidateSet {
                routes: vec![straight_east(600.0, 30_000.0, 30_000.0)],
                warnings: Vec::new(),
            },
            &CancelToken::unbounded(),
        )
        .unwrap();
    let c = &outcome.candidates[0];
    let targets = (0..400).filter(|sid| sid % 3 != 0).count() as u64;
    assert_eq!(c.high_risk_total, targets);
    assert!(c.high_risk_total > 100);
    assert_eq!(c.high_risk_sections.len(), 100);
    assert!(c.high_risk_sections.iter().all(|s| s.severity_weight >= 3));
    // 重み2の小線分は表示もナビも対象外
    assert!(
        c.high_risk_sections
            .iter()
            .all(|s| s.segment_index % 3 != 0)
    );
    let nav = c.navigation.as_ref().unwrap();
    let covered: u64 = nav
        .sections
        .iter()
        .map(|s| u64::from(s.end_segment_index - s.start_segment_index + 1))
        .sum();
    assert_eq!(covered, c.high_risk_total);
    // 表示区間の両端は元の辺の上の補間点
    let s = &c.high_risk_sections[0];
    assert_eq!(s.source_vertex_start_index, 0);
    assert!((s.end.lon - s.start.lon - 75.0 / kx).abs() < 1e-9);
}

#[test]
fn independent_of_accident_registration_order() {
    // 同じ事故集合を逆順に登録しても、IDの振り直しを除いて同じ集計になる
    let base: Vec<Acc> = (0..50)
        .map(|i| {
            at_m(15.0 * i as f64, (i % 9) as f64 * 7.0).casualties(i as u16 % 2, 1 + i as u16 % 3)
        })
        .collect();
    let mut reversed = base.clone();
    reversed.reverse();
    let a = evaluate(&base, Mode::Walk, ConditionFilter::default());
    let b = evaluate(&reversed, Mode::Walk, ConditionFilter::default());
    let (ca, cb) = (&a.candidates[0], &b.candidates[0]);
    assert_eq!(ca.totals, cb.totals);
    assert_eq!(ca.high_risk_sections, cb.high_risk_sections);
    assert_eq!(ca.high_risk_total, cb.high_risk_total);
    // 加算順の違いによる最下位桁の差を除き同じ
    assert!((ca.weighted_risk - cb.weighted_risk).abs() <= 0.01);
}
