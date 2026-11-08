//! `RoutingEngine::evaluate`（behavior.md 5〜8章）。
//!
//! 吸着 → 探索 → 候補ごとの形状・事故の集合B・件数・リスク・高リスク区間 → 選定・文面、
//! までを1回で行う。探索の状態は利用者に出さない。

mod accidents;
pub mod geometry;

use std::time::Instant;

use crate::contract::engine::{MAX_ACCIDENT_POINTS, MAX_HIGH_RISK_SECTIONS};
use crate::contract::{
    AccidentPoint, AccidentTotals, CancelToken, EngineError, EvaluatedRoute, HighRiskSegment,
    Interrupt, Maneuver, Mode, NavigationData, RouteEngine, RouteOutcome, RouteQuery, RoutedPath,
    Weather,
};
use crate::domain::candidates::SINGLE_CANDIDATE_WARNING;
use crate::domain::corridor::Subdivision;
use crate::domain::engine::{
    NO_ACCIDENT_REASON, NO_ACCIDENT_WARNING, SAME_ROUTE_REASON, ZERO_BASELINE_REASON,
    format_minutes,
};
use crate::domain::navigation::{
    HIGH_RISK_MIN_WEIGHT, display_sections_checked, navigation_data_checked,
};
use crate::domain::rounding::py_round;
use crate::domain::routing::risk::RiskView;
use crate::domain::routing::risk::arc_cost::{RiskArcCost, path_risk};
use crate::domain::routing::search::SearchContext;
use crate::domain::routing::search::endpoints::path_distance_dm;
use crate::domain::routing::search::route::RouteError;
use crate::domain::routing::search::route::find_routes_ds;
use crate::domain::routing::snap::snap;

use super::engine::RoutingEngine;

/// within_detourの判定の許容（秒）。
const DETOUR_EPSILON_S: f64 = 1e-6;

const RISK_NOTE_HEAD: &str = "過去の事故に基づく推定指標です。事故を道路の区間や交差点に割り当て、道路の種類ごとの平均で偶然のばらつきを補正しています。";
const RISK_NOTE_CAR: &str = "生活道路は交通量が少ないぶん件数が少なく出るため、重く数えています。";
const RISK_NOTE_WEATHER: &str = "雨・霧・雪を指定した場合は、同じ天候の事故をさらに重く数えます。";
const RISK_NOTE_TAIL: &str = "交通量や通行回数で補正した事故確率ではありません。";

/// 新方式の `risk_note`（behavior.md 8章「新方式（schema 3）での扱い」）。
pub fn routing_risk_note(mode: Mode, weather: Option<Weather>) -> String {
    let mut note = RISK_NOTE_HEAD.to_owned();
    if mode == Mode::Car {
        note.push_str(RISK_NOTE_CAR);
    }
    if matches!(weather, Some(Weather::Rain | Weather::Fog | Weather::Snow)) {
        note.push_str(RISK_NOTE_WEATHER);
    }
    note.push_str(RISK_NOTE_TAIL);
    note
}

/// 1候補の評価途中の値。
struct Candidate {
    coordinates: Vec<crate::contract::LonLat>,
    subdivision: Subdivision,
    totals: AccidentTotals,
    weather_match: Option<AccidentTotals>,
    weighted_risk: f64,
    risk_per_km: f64,
    accident_points: Vec<AccidentPoint>,
    weights: Vec<u64>,
    high_risk_sections: Vec<HighRiskSegment>,
    high_risk_total: u64,
    distance_m: f64,
    duration_s: f64,
}

/// 評価の中間値のうち、応答に出さない計測用の値（`routing_eval engine-batch`）。
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateReport {
    /// 間引く前の形状の頂点数。
    pub full_vertex_count: usize,
    /// 表示用の形状を間引いたか。
    pub simplified: bool,
    /// 表示事故点の数（`min(accident_count, 250)`）。
    pub accident_point_count: usize,
    /// 件数3件以上の高リスクのsidの数（応答の `high_risk_total` と同じ）。
    pub t3_sids: u64,
    /// 件数3件以上で作った表示用の区間の数。
    pub t3_sections: u64,
    /// 「2件以上かつ深刻度の合計3以上」の高リスクのsidの数。
    pub t2_sids: u64,
    /// 「2件以上かつ深刻度の合計3以上」で作った表示用の区間の数。
    pub t2_sections: u64,
}

/// 1回の評価の計測用の値（応答には出さない）。
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluationReport {
    /// `RouteOutcome::candidates` と同じ順の候補ごとの値。
    pub candidates: Vec<CandidateReport>,
    /// 探索の候補の整理で80,000mを超えるために外した候補の数。
    pub dropped_too_long: u64,
}

fn search_error(error: RouteError, cancel: &CancelToken) -> EngineError {
    match error {
        RouteError::NoRoute => EngineError::NoRoute,
        RouteError::RouteTooLong => EngineError::RouteTooLong,
        RouteError::Interrupted => match cancel.check() {
            Err(interrupt) => EngineError::Interrupted(interrupt),
            // `find_routes_ds` が中断を返したのに取消し・期限切れが消えている場合は、
            // 種類を失わないための最後の手段として取消しにする。
            Ok(()) => EngineError::Interrupted(Interrupt::Cancelled),
        },
    }
}

/// behavior.md 5章の流れで `RouteOutcome` を作る。
pub fn evaluate(
    engine: &RoutingEngine,
    query: &RouteQuery,
    cancel: &CancelToken,
) -> Result<RouteOutcome, EngineError> {
    evaluate_with_report(engine, query, cancel).map(|(outcome, _)| outcome)
}

/// [`evaluate`] と同じ計算を行い、応答に出さない計測用の値も返す（`routing_eval engine-batch`）。
pub fn evaluate_with_report(
    engine: &RoutingEngine,
    query: &RouteQuery,
    cancel: &CancelToken,
) -> Result<(RouteOutcome, EvaluationReport), EngineError> {
    let started = Instant::now();
    let mode = query.mode();
    let conditions = query.conditions();
    let data = engine.data();
    let graph = &data.graph;

    // 手順1：吸着。
    let start = snap(graph, &data.topology, &data.odindex, query.origin(), mode)
        .ok_or(EngineError::NoRoute)?;
    let end = snap(graph, &data.topology, &data.odindex, query.dest(), mode)
        .ok_or(EngineError::NoRoute)?;
    cancel.check()?;

    // 手順2〜8：探索。
    let ctx = SearchContext {
        graph,
        topology: &data.topology,
        automaton: engine.automaton(mode),
        landmarks: &data.landmarks,
        mode,
        params: engine.params(),
    };
    let mut view = RiskView::new(
        &data.risk,
        graph,
        &data.topology,
        &data.accidents,
        engine.snapshot(),
        mode,
        conditions,
    );
    let detour_ds = (600.0 * query.max_detour_min()).floor() as u64;
    let search = {
        let mut cost = RiskArcCost::new(&mut view);
        find_routes_ds(&ctx, &mut cost, &start, &end, detour_ds, cancel)
    };
    cancel.check()?;
    let search = match search {
        Ok(search) => search,
        Err(error) => return Err(search_error(error, cancel)),
    };
    let mut candidates = Vec::with_capacity(search.candidates.len());
    let mut reports = Vec::with_capacity(search.candidates.len());
    if search.candidates.is_empty() {
        return Err(EngineError::ContractViolation(
            "探索の候補が0本です".to_owned(),
        ));
    }

    // 候補ごとの形状・集合B・リスク・高リスク区間。
    for path in &search.candidates {
        cancel.check()?;
        let distance_dm = path_distance_dm(graph, &path.start, &path.end, &path.dedges)
            .ok_or_else(|| {
                EngineError::ContractViolation("経路の長さを計算できません".to_owned())
            })?;
        let distance_m = distance_dm as f64 / 10.0;
        let shape = geometry::build_shape(graph, path, cancel)?;
        let subdivision = Subdivision::build(&shape.display, cancel)?;
        cancel.check()?;
        let accident_set = accidents::build_accident_set(
            graph,
            &data.topology,
            &data.accidents,
            &data.unassigned,
            engine.snapshot(),
            &subdivision,
            path,
            mode,
            conditions,
            cancel,
        )?;
        cancel.check()?;
        let summary =
            accidents::summarize(engine.snapshot(), conditions, &accident_set.ids, cancel)?;
        let (counts, severities) = accidents::sid_counts(
            graph,
            &data.topology,
            engine.snapshot(),
            path,
            &shape,
            &subdivision,
            &accident_set,
            cancel,
        )?;
        let weights = accidents::high_risk_weights_from_counts(&counts, &severities);
        // しきい値を「2件以上かつ深刻度の合計3以上」にしたときの重み（評価用）。
        let t2_weights: Vec<u64> = counts
            .iter()
            .zip(&severities)
            .map(|(&count, &severity)| {
                if count >= 2 && severity >= HIGH_RISK_MIN_WEIGHT {
                    severity
                } else {
                    0
                }
            })
            .collect();
        let t2_sids = t2_weights
            .iter()
            .filter(|&&weight| weight >= HIGH_RISK_MIN_WEIGHT)
            .count() as u64;
        let (high_risk_sections, high_risk_total) =
            display_sections_checked(&subdivision, &weights, cancel)?;
        let (t2_section_list, _) = display_sections_checked(&subdivision, &t2_weights, cancel)?;
        let weighted_risk = py_round(path_risk(&mut view, path), 2);
        let risk_per_km = py_round(weighted_risk / (distance_m / 1000.0).max(0.001), 3);
        reports.push(CandidateReport {
            full_vertex_count: shape.full.len(),
            simplified: shape.simplified,
            accident_point_count: summary.accident_points.len(),
            t3_sids: high_risk_total,
            t3_sections: high_risk_sections.len() as u64,
            t2_sids,
            t2_sections: t2_section_list.len() as u64,
        });
        candidates.push(Candidate {
            coordinates: shape.display,
            subdivision,
            totals: summary.totals,
            weather_match: summary.weather_match,
            weighted_risk,
            risk_per_km,
            accident_points: summary.accident_points,
            weights,
            high_risk_sections,
            high_risk_total,
            distance_m,
            duration_s: path.time_ds as f64 / 10.0,
        });
    }
    cancel.check()?;

    // 手順9：選定。
    let n = candidates.len();
    let fastest = (0..n)
        .reduce(|best, i| {
            let (a, b) = (&candidates[i], &candidates[best]);
            if (a.duration_s, a.distance_m) < (b.duration_s, b.distance_m) {
                i
            } else {
                best
            }
        })
        .expect("候補は1本以上");
    let fastest_duration = candidates[fastest].duration_s;
    let limit_s = fastest_duration + query.max_detour_min() * 60.0 + DETOUR_EPSILON_S;
    let extra: Vec<f64> = candidates
        .iter()
        .map(|c| py_round((c.duration_s - fastest_duration) / 60.0, 3))
        .collect();
    let within: Vec<bool> = candidates.iter().map(|c| c.duration_s <= limit_s).collect();
    if let Some(i) = within.iter().position(|&ok| !ok) {
        return Err(EngineError::ContractViolation(format!(
            "候補{}の所要時間が遠回りの上限を超えています",
            i + 1
        )));
    }

    let any_accident = candidates.iter().any(|c| c.totals.accident_count > 0);
    let safest = any_accident.then(|| {
        (0..n)
            .filter(|&i| within[i])
            .reduce(|best, i| {
                let key = |j: usize| {
                    (
                        candidates[j].weighted_risk,
                        candidates[j].duration_s,
                        candidates[j].distance_m,
                    )
                };
                if key(i) < key(best) { i } else { best }
            })
            .expect("最短時間経路は常に許容内")
    });
    let base_risk = candidates[fastest].weighted_risk;
    let risk_reduction_percent = safest.and_then(|s| {
        (base_risk > 0.0).then(|| {
            py_round(
                100.0 * (base_risk - candidates[s].weighted_risk) / base_risk,
                1,
            )
        })
    });
    let reason = match safest {
        None => NO_ACCIDENT_REASON.to_owned(),
        Some(s) => {
            let mut reason = if s == fastest {
                SAME_ROUTE_REASON.to_owned()
            } else {
                format!(
                    "取得した{}候補のうち、最短時間経路から+{}分以内で、推定リスクが最小の経路を選びました。追加{:.1}分、リスク{:.1}%減です。",
                    n,
                    format_minutes(query.max_detour_min()),
                    extra[s],
                    // 2経路が異なるなら基準リスクは正で、削減率は必ずある
                    risk_reduction_percent.unwrap_or(0.0),
                )
            };
            if base_risk == 0.0 {
                reason.push_str(ZERO_BASELINE_REASON);
            }
            reason
        }
    };
    let mut warnings = Vec::new();
    if n == 1 {
        warnings.push(SINGLE_CANDIDATE_WARNING.to_owned());
    }
    if !any_accident {
        warnings.push(NO_ACCIDENT_WARNING.to_owned());
    }

    // 候補ごとの表示用区間と、選定候補だけのナビ用データ。
    let mut evaluated = Vec::with_capacity(n);
    for (i, candidate) in candidates.into_iter().enumerate() {
        cancel.check()?;
        let selected = i == fastest || safest == Some(i);
        let navigation: Option<NavigationData> = if selected {
            Some(navigation_data_checked(
                &candidate.subdivision,
                &candidate.weights,
                cancel,
            )?)
        } else {
            None
        };
        evaluated.push(EvaluatedRoute {
            path: RoutedPath {
                duration_s: candidate.duration_s,
                distance_m: candidate.distance_m,
                coordinates: candidate.coordinates,
            },
            totals: candidate.totals,
            weather_match: candidate.weather_match,
            weighted_risk: candidate.weighted_risk,
            risk_per_km: candidate.risk_per_km,
            accident_points: candidate.accident_points,
            high_risk_sections: candidate.high_risk_sections,
            high_risk_total: candidate.high_risk_total,
            navigation,
            extra_minutes: extra[i],
            within_detour: within[i],
        });
    }

    cancel.check()?;
    let aggregation_ms = py_round(started.elapsed().as_secs_f64() * 1000.0, 2);
    let outcome = RouteOutcome {
        dataset_id: engine.dataset().dataset_id.clone(),
        data_years: engine.dataset().data_years.clone(),
        mode,
        conditions,
        candidates: evaluated,
        fastest,
        safest,
        extra_minutes: safest.map(|s| extra[s]),
        risk_reduction_percent,
        reason,
        warnings,
        risk_note: routing_risk_note(mode, conditions.weather),
        aggregation_ms,
    };
    Ok((
        outcome,
        EvaluationReport {
            candidates: reports,
            dropped_too_long: search.dropped_too_long,
        },
    ))
}

/// `RouteOutcome` の相関制約（openapi.yaml・behavior.md 7〜8章）を検査する。
///
/// 評価コマンド（`routing_eval engine-batch`）が応答の値を確かめるのに使う。`testing` の
/// fixture と同じ規則で、`backend/tests` からも同じ関数を使う。
pub fn check_outcome_invariants(outcome: &RouteOutcome) -> Result<(), String> {
    let n = outcome.candidates.len();
    if !(1..=6).contains(&n) {
        return Err(format!("候補数{n}が1〜6の範囲外"));
    }
    if outcome.fastest >= n {
        return Err("fastestが候補にない".into());
    }
    if outcome.safest.is_some_and(|s| s >= n) {
        return Err("safestが候補にない".into());
    }
    // safestがnullになるのは、全候補で集合Bが空のときだけ（behavior.md 8章）
    let any_accident = outcome
        .candidates
        .iter()
        .any(|c| c.totals.accident_count > 0);
    match (outcome.safest.is_some(), any_accident) {
        (false, true) => return Err("集合Bが空でない候補があるのにsafestがnull".into()),
        (true, false) => return Err("全候補で集合Bが空なのにsafestがある".into()),
        _ => {}
    }
    if outcome.extra_minutes.is_some() != outcome.safest.is_some() {
        return Err("extra_minutesの有無がsafestの有無と一致しない".into());
    }
    if outcome.safest.is_none() && outcome.risk_reduction_percent.is_some() {
        return Err("safestがnullなのにrisk_reduction_percentがある".into());
    }
    for (i, c) in outcome.candidates.iter().enumerate() {
        let id = format!("route-{}", i + 1);
        let t = &c.totals;
        if t.fatal_accidents + t.injury_accidents != t.accident_count {
            return Err(format!("{id}: 死亡事故+負傷事故がaccident_countと異なる"));
        }
        if c.weather_match.is_some() != outcome.conditions.weather.is_some() {
            return Err(format!("{id}: weather_matchの有無が天候指定と一致しない"));
        }
        if let Some(w) = &c.weather_match
            && (w.fatal_accidents + w.injury_accidents != w.accident_count
                || w.accident_count > t.accident_count)
        {
            return Err(format!("{id}: weather_matchの件数が不正"));
        }
        let points = (t.accident_count as usize).min(MAX_ACCIDENT_POINTS);
        if c.accident_points.len() != points {
            return Err(format!("{id}: 事故点数がmin(accident_count,250)と異なる"));
        }
        let sections = (c.high_risk_total as usize).min(MAX_HIGH_RISK_SECTIONS);
        if c.high_risk_sections.len() != sections {
            return Err(format!(
                "{id}: 表示区間数がmin(high_risk_total,100)と異なる"
            ));
        }
        if c.high_risk_sections.iter().any(|s| s.severity_weight < 3) {
            return Err(format!("{id}: 重み3未満の表示区間がある"));
        }
        let selected = i == outcome.fastest || outcome.safest == Some(i);
        match (&c.navigation, selected) {
            (Some(nav), true) => check_navigation(&id, c, nav)?,
            (None, false) => {}
            (Some(_), false) => return Err(format!("{id}: 未選定なのにnavigationがある")),
            (None, true) => return Err(format!("{id}: 選定候補なのにnavigationがない")),
        }
    }
    Ok(())
}

fn check_navigation(id: &str, c: &EvaluatedRoute, nav: &NavigationData) -> Result<(), String> {
    let d = &nav.vertex_distances_m;
    if d.len() != c.path.coordinates.len() {
        return Err(format!("{id}: vertex_distances_mの長さが頂点数と異なる"));
    }
    if d.first() != Some(&0.0) || d.last() != Some(&nav.length_m) {
        return Err(format!(
            "{id}: vertex_distances_mの先頭が0、末尾がlength_mでない"
        ));
    }
    if d.windows(2).any(|w| w[0] > w[1]) {
        return Err(format!("{id}: vertex_distances_mが非減少でない"));
    }
    if nav.guidance.is_empty() {
        return Err(format!("{id}: guidanceが空"));
    }
    let mut previous_at_m = 0.0;
    for (i, g) in nav.guidance.iter().enumerate() {
        if !g.at_m.is_finite() || g.at_m < 0.0 {
            return Err(format!("{id}: 案内地点の距離が不正"));
        }
        if g.at_m > nav.length_m {
            return Err(format!("{id}: 案内地点がlength_mを超えた"));
        }
        if g.at_m < previous_at_m {
            return Err(format!("{id}: 案内地点が逆行している"));
        }
        if g.maneuver == Maneuver::Arrive && i + 1 != nav.guidance.len() {
            return Err(format!("{id}: arriveが末尾以外にある"));
        }
        previous_at_m = g.at_m;
    }
    let last = nav.guidance.last().expect("空でない");
    if last.maneuver != Maneuver::Arrive || last.at_m != nav.length_m {
        return Err(format!("{id}: 末尾のarriveがlength_mと一致しない"));
    }
    let mut covered = 0u64;
    let mut previous_end: Option<u32> = None;
    for s in &nav.sections {
        if s.start_segment_index > s.end_segment_index
            || !(0.0 <= s.start_m && s.start_m <= s.end_m && s.end_m <= nav.length_m)
        {
            return Err(format!("{id}: ナビ区間の範囲が不正"));
        }
        if previous_end.is_some_and(|e| s.start_segment_index <= e + 1) {
            return Err(format!("{id}: ナビ区間の順序・間隔が不正"));
        }
        if s.severity_weight < 3 {
            return Err(format!("{id}: 重み3未満のナビ区間がある"));
        }
        covered += u64::from(s.end_segment_index - s.start_segment_index + 1);
        previous_end = Some(s.end_segment_index);
    }
    if covered != c.high_risk_total {
        return Err(format!("{id}: Σ(E−S+1)がhigh_risk_totalと異なる"));
    }
    Ok(())
}
