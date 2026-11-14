//! `RoutingEngine::evaluate`（behavior.md 5〜8章）。
//!
//! 吸着 → 探索 → 候補ごとの形状・事故の集合B・件数・リスク・高リスク区間 → 選定・文面、
//! までを1回で行う。探索の状態は利用者に出さない。

mod accidents;
pub mod geometry;
pub mod guidance;

use std::time::Instant;

use crate::contract::engine::{MAX_ACCIDENT_POINTS, MAX_HIGH_RISK_SECTIONS};
use crate::contract::geo::straight_distance_m;
use crate::contract::{
    AccidentPoint, AccidentTotals, CancelToken, EngineError, EvaluatedRoute, GuidanceStep,
    HighRiskSegment, Interrupt, Maneuver, Mode, NavigationData, RouteEngine, RouteOutcome,
    RouteQuery, RoutedPath, Weather,
};
use crate::domain::candidates::SINGLE_CANDIDATE_WARNING;
use crate::domain::corridor::Subdivision;
use crate::domain::engine::{
    NO_ACCIDENT_WARNING, SAME_ROUTE_REASON, ZERO_BASELINE_REASON, format_minutes,
};
use crate::domain::navigation::{
    HIGH_RISK_MIN_WEIGHT, display_sections_checked, navigation_data_checked, vertex_distances,
};
use crate::domain::rounding::{py_round, risk_reduction_percent};
use crate::domain::routing::risk::RiskView;
use crate::domain::routing::risk::arc_cost::{RiskArcCost, path_risk};
use crate::domain::routing::search::SearchContext;
use crate::domain::routing::search::SearchStatus;
use crate::domain::routing::search::candidates::same_route;
use crate::domain::routing::search::endpoints::path_distance_dm;
use crate::domain::routing::search::route::RouteError;
use crate::domain::routing::search::route::{DetourLimit, find_routes_with_detour};
pub use crate::domain::routing::search::route::{MIDDLE_MIN_DETOUR_MIN, middle_search_detour_ds};
use crate::domain::routing::snap::snap;

use super::engine::RoutingEngine;

/// within_detourの判定の許容（秒）。
const DETOUR_EPSILON_S: f64 = 1e-6;

/// 中間案と事故考慮経路の所要時間の差の下限（秒）。仮置き。docs/multi-route-design.md 4.1節
pub const MIDDLE_MIN_TIME_GAP_S: f64 = 60.0;
/// 中間案の削減率が事故考慮経路の削減率に占める割合の下限。仮置き。docs/multi-route-design.md 4.1節
pub const MIDDLE_MIN_REDUCTION_RATIO: f64 = 0.5;

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
    total_accident_count: u64,
    weather_match: Option<AccidentTotals>,
    weighted_risk: f64,
    risk_per_km: f64,
    accident_points: Vec<AccidentPoint>,
    weights: Vec<u64>,
    high_risk_sections: Vec<HighRiskSegment>,
    high_risk_total: u64,
    distance_m: f64,
    duration_s: f64,
    guidance: Vec<GuidanceStep>,
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
    /// 探索の候補の整理で250,000mを超えるために外した候補の数。
    pub dropped_too_long: u64,
    /// 中間案を探索した場合の `search_best` の状態。探索しなければ `None`。
    pub middle_status: Option<SearchStatus>,
    /// 中間案の楕円の切り出しと探索。中間案を探索しなければ0。
    pub middle_ms: f64,
}

/// 中間案の選定結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MiddleSelection {
    /// `candidates` の添字。中間案を出さないなら `None`。
    pub middle: Option<usize>,
    /// 中間案として採用しなかったために候補から外す添字。
    pub remove: Option<usize>,
    /// 取り除きを反映した fastest の添字。
    pub fastest: usize,
    /// 取り除きを反映した safest の添字。
    pub safest: Option<usize>,
}

/// 中間案の添字を決め、採用しなかった中間案を候補から外すかどうかを返す。
///
/// `matching` は `search.middle` と同じ経路の候補の添字（見つからなければ `None`）。
/// `durations`・`risks` は候補と同じ順の、丸めた所要時間（秒）と `weighted_risk`。
pub fn select_middle(
    fastest: usize,
    safest: Option<usize>,
    any_accident: bool,
    matching: Option<usize>,
    durations: &[f64],
    risks: &[f64],
) -> MiddleSelection {
    let mut selection = MiddleSelection {
        middle: None,
        remove: None,
        fastest,
        safest,
    };
    let Some(k) = matching else {
        return selection;
    };
    let base = risks.get(fastest).copied().unwrap_or(0.0);
    let is_middle = safest.is_some_and(|s| {
        s != fastest
            && any_accident
            && base > 0.0
            && k != fastest
            && k != s
            && durations
                .get(fastest)
                .zip(durations.get(k))
                .zip(durations.get(s))
                .is_some_and(|((&f, &m), &safe)| f < m && m < safe)
            && risks
                .get(k)
                .zip(risks.get(s))
                .is_some_and(|(&m, &safe)| safe < m && m < base)
            && durations
                .get(s)
                .zip(durations.get(k))
                .is_some_and(|(&safe, &m)| safe - m >= MIDDLE_MIN_TIME_GAP_S)
            && risks.get(s).is_some_and(|&safe| {
                risks
                    .get(k)
                    .is_some_and(|&m| (base - m) >= MIDDLE_MIN_REDUCTION_RATIO * (base - safe))
            })
    });
    if is_middle {
        selection.middle = Some(k);
        return selection;
    }
    if k != fastest && safest != Some(k) {
        selection.remove = Some(k);
        if fastest > k {
            selection.fastest -= 1;
        }
        if let Some(s) = selection.safest
            && s > k
        {
            selection.safest = Some(s - 1);
        }
    }
    selection
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

/// 探索の前に `RouteTooLong` とする、出発地と目的地の直線距離（m、behavior.md 5章 手順1.1）。
///
/// 経路の距離は吸着点間の直線距離以上で、吸着は各300m以内なので、直線距離が
/// 250,000m＋600mを超えれば p0 も250,000mを超えるのが原則である。ただし厳密な保証ではない。
/// 辺の長さは辺ごとに0.1m単位へ四捨五入され（1辺あたり最大0.05m短くなる）、局所平面近似の
/// 中央緯度も線分ごとに異なる。余裕の約14.4kmを丸めだけで使い切るには、平均0.9m未満の辺が
/// 約29万本並び、すべて最大限切り捨てられる必要があり、実際の道路網の経路では起きない。
/// これがないと、遠い2点でも全国を探索して期限（35秒）やメモリを使い切ってから422を返す。
pub const EARLY_ROUTE_TOO_LONG_STRAIGHT_M: f64 = 265_000.0;

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

    // 手順1.1：直線距離で明らかに長すぎる要求は、探索せずに `RouteTooLong` にする（issue #39）。
    if straight_distance_m(query.origin(), query.dest()) > EARLY_ROUTE_TOO_LONG_STRAIGHT_M {
        return Err(EngineError::RouteTooLong);
    }

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
    let detour = match query.max_detour_min() {
        Some(max_detour_min) => DetourLimit::Manual(max_detour_min),
        None => DetourLimit::Auto,
    };
    let search = {
        let mut cost = RiskArcCost::new(&mut view);
        find_routes_with_detour(&ctx, &mut cost, &start, &end, detour, cancel)
    };
    cancel.check()?;
    let search = match search {
        Ok(search) => search,
        Err(error) => return Err(search_error(error, cancel)),
    };
    let detour_limit_min = search.detour_limit_min;
    let detour_limit_auto = query.max_detour_min().is_none();
    let mut candidates = Vec::with_capacity(search.candidates.len());
    let mut reports = Vec::with_capacity(search.candidates.len());
    if search.candidates.is_empty() {
        return Err(EngineError::ContractViolation(
            "探索の候補が0本です".to_owned(),
        ));
    }
    // 中間案と同じ経路の候補の添字（250kmの検査などで落ちていれば `None`）。
    let matching_middle = search
        .middle
        .as_ref()
        .and_then(|m| search.candidates.iter().position(|p| same_route(p, m)));

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
        let vertex_distances_m = vertex_distances(&subdivision);
        let guidance = guidance::build_guidance(
            graph,
            &data.topology,
            path,
            &shape,
            mode,
            &vertex_distances_m,
            cancel,
        )?;
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
        let summary = accidents::summarize(engine.snapshot(), conditions, &accident_set, cancel)?;
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
            total_accident_count: accident_set.total,
            weather_match: summary.weather_match,
            weighted_risk,
            risk_per_km,
            accident_points: summary.accident_points,
            weights,
            high_risk_sections,
            high_risk_total,
            distance_m,
            duration_s: path.time_ds as f64 / 10.0,
            guidance,
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
    let limit_s = fastest_duration + detour_limit_min * 60.0 + DETOUR_EPSILON_S;
    let mut extra: Vec<f64> = candidates
        .iter()
        .map(|c| py_round((c.duration_s - fastest_duration) / 60.0, 3))
        .collect();
    let mut within: Vec<bool> = candidates.iter().map(|c| c.duration_s <= limit_s).collect();
    if let Some(i) = within.iter().position(|&ok| !ok) {
        return Err(EngineError::ContractViolation(format!(
            "候補{}の所要時間が遠回りの上限を超えています",
            i + 1
        )));
    }

    let any_accident = candidates.iter().any(|c| c.totals.accident_count > 0);
    // 全候補で事故が0件でも、推定リスクが最小の経路を選ぶ（旧方式の engine.rs は選ばない）。
    let safest_index = (0..n)
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
        .expect("最短時間経路は常に許容内");

    // 中間案の選定。採用しなかった中間案は候補から外し、fastest・safestの添字を合わせる。
    let durations: Vec<f64> = candidates.iter().map(|c| c.duration_s).collect();
    let risks: Vec<f64> = candidates.iter().map(|c| c.weighted_risk).collect();
    let selection = select_middle(
        fastest,
        Some(safest_index),
        any_accident,
        matching_middle,
        &durations,
        &risks,
    );
    let middle = selection.middle;
    let fastest = selection.fastest;
    let safest_index = selection.safest.expect("新方式は常にsafestを選ぶ");
    if let Some(remove) = selection.remove {
        candidates.remove(remove);
        reports.remove(remove);
        extra.remove(remove);
        within.remove(remove);
    }
    let n = candidates.len();
    // 取り除いた中間案だけに事故があった場合に備えて、取り除いた後の候補で数え直す。
    let any_accident = candidates.iter().any(|c| c.totals.accident_count > 0);
    let safest = Some(safest_index);
    let base_risk = candidates[fastest].weighted_risk;
    let risk_reduction_percent =
        safest.and_then(|s| risk_reduction_percent(base_risk, candidates[s].weighted_risk));
    let reason = {
        let mut reason = if safest_index == fastest {
            SAME_ROUTE_REASON.to_owned()
        } else if !any_accident {
            format!(
                "全候補で該当する過去の事故はありません。道路の種類ごとの平均から推定したリスクが最小の経路を選びました。追加{:.1}分、リスク{:.1}%減です。",
                extra[safest_index],
                // 2経路が異なるなら基準リスクは正で、削減率は必ずある
                risk_reduction_percent.unwrap_or(0.0),
            )
        } else {
            format!(
                "道路網を探索して得た{}候補のうち、最短時間経路から+{}分以内で、推定リスクが最小の経路を選びました。追加{:.1}分、リスク{:.1}%減です。",
                n,
                format_minutes(detour_limit_min),
                extra[safest_index],
                // 2経路が異なるなら基準リスクは正で、削減率は必ずある
                risk_reduction_percent.unwrap_or(0.0),
            )
        };
        if base_risk == 0.0 {
            reason.push_str(ZERO_BASELINE_REASON);
        }
        reason
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
        let selected = i == fastest || safest == Some(i) || middle == Some(i);
        let navigation: Option<NavigationData> = if selected {
            let mut navigation =
                navigation_data_checked(&candidate.subdivision, &candidate.weights, cancel)?;
            navigation.guidance = candidate.guidance;
            Some(navigation)
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
            total_accident_count: Some(candidate.total_accident_count),
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
        middle,
        extra_minutes: safest.map(|s| extra[s]),
        risk_reduction_percent,
        reason,
        warnings,
        detour_limit_min,
        detour_limit_auto,
        risk_note: routing_risk_note(mode, conditions.weather),
        aggregation_ms,
    };
    Ok((
        outcome,
        EvaluationReport {
            candidates: reports,
            dropped_too_long: search.dropped_too_long,
            middle_status: search.stats.middle_status,
            middle_ms: search.stats.middle_ms,
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
    if outcome.middle.is_some_and(|m| m >= n) {
        return Err("middleが候補にない".into());
    }
    if let Some(middle) = outcome.middle {
        let Some(safest) = outcome.safest else {
            return Err("middleがあるのにsafestがnull".into());
        };
        if middle == outcome.fastest {
            return Err("middleがfastestと同じ".into());
        }
        if middle == safest {
            return Err("middleがsafestと同じ".into());
        }
        let mid = &outcome.candidates[middle];
        let fast = &outcome.candidates[outcome.fastest];
        let safe = &outcome.candidates[safest];
        if !(fast.path.duration_s < mid.path.duration_s
            && mid.path.duration_s < safe.path.duration_s)
        {
            return Err("middleの所要時間がfastestとsafestのあいだにない".into());
        }
        if !(safe.weighted_risk < mid.weighted_risk && mid.weighted_risk < fast.weighted_risk) {
            return Err("middleのweighted_riskがsafestとfastestのあいだにない".into());
        }
    }
    // safestがnullなのは、事故があるときに限って違反になる（behavior.md 8章）
    let any_accident = outcome
        .candidates
        .iter()
        .any(|c| c.totals.accident_count > 0);
    if outcome.safest.is_none() && any_accident {
        return Err("集合Bが空でない候補があるのにsafestがnull".into());
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
        let selected =
            i == outcome.fastest || outcome.safest == Some(i) || outcome.middle == Some(i);
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
