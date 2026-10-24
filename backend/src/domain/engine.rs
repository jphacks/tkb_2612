//! `RouteEngine` の実装：候補の評価・選定・文言（behavior.md 6〜8章）。

use std::sync::Arc;
use std::time::Instant;

use crate::contract::{
    BoxFuture, CancelToken, CandidateSet, DatasetInfo, EngineError, EvaluatedRoute, RouteEngine,
    RouteOutcome, RouteQuery, Router, Weather,
};

use super::candidates;
use super::corridor::{Subdivision, corridor_hits};
use super::navigation::{display_sections, navigation_data};
use super::rounding::py_round;
use super::scoring::{RouteScore, score_route};
use super::snapshot::AccidentSnapshot;

/// 全候補で集合Bが空のときの警告。
pub const NO_ACCIDENT_WARNING: &str = "該当する事故が0件でも、安全であることの証明ではありません。";
/// 最短時間経路と事故考慮経路が同じときの説明。
pub const SAME_ROUTE_REASON: &str = "今回は最短時間経路が事故を考慮した経路と同じです。";
/// 最短時間経路のリスクが0のときに続ける説明。
pub const ZERO_BASELINE_REASON: &str = "最短時間経路のリスクが0のため、削減率は算出できません。";
/// 全候補で集合Bが空のときの説明。
pub const NO_ACCIDENT_REASON: &str =
    "全候補で該当する過去の事故がないため、事故を考慮した経路は選んでいません。";

const RISK_NOTE_HEAD: &str =
    "過去の事故件数に基づく参考指標です。経路から約72m以内の事故を、近いほど重く数えています。";
const RISK_NOTE_WEATHER: &str = "雨・霧・雪を指定した場合は、同じ天候の事故をさらに重く数えます。";
const RISK_NOTE_TAIL: &str = "交通量や通行回数で補正した事故確率ではありません。";

/// within_detourの判定の許容（秒）。
const DETOUR_EPSILON_S: f64 = 1e-6;

/// 事故データのスナップショットを持つ計算エンジン。
#[derive(Debug, Clone)]
pub struct AccidentEngine {
    snapshot: Arc<AccidentSnapshot>,
    dataset: DatasetInfo,
}

impl AccidentEngine {
    pub fn new(snapshot: Arc<AccidentSnapshot>, dataset: DatasetInfo) -> Self {
        Self { snapshot, dataset }
    }

    pub fn snapshot(&self) -> &AccidentSnapshot {
        &self.snapshot
    }
}

/// `{d}` の表記。整数ならそのまま（10）、それ以外は最短の表記（7.5）。
fn format_minutes(minutes: f64) -> String {
    if minutes.fract() == 0.0 {
        format!("{}", minutes as i64)
    } else {
        format!("{minutes}")
    }
}

/// 説明 `risk_note`。雨・霧・雪の指定時だけ天候の一文を挟む。
pub fn risk_note(weather: Option<Weather>) -> String {
    let mut note = RISK_NOTE_HEAD.to_owned();
    if matches!(weather, Some(Weather::Rain | Weather::Fog | Weather::Snow)) {
        note.push_str(RISK_NOTE_WEATHER);
    }
    note.push_str(RISK_NOTE_TAIL);
    note
}

/// 1候補の評価途中の値。
struct Scored {
    subdivision: Subdivision,
    score: RouteScore,
}

impl RouteEngine for AccidentEngine {
    fn dataset(&self) -> &DatasetInfo {
        &self.dataset
    }

    fn collect_candidates<'a>(
        &'a self,
        router: &'a dyn Router,
        query: &'a RouteQuery,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<CandidateSet, EngineError>> {
        Box::pin(candidates::collect_candidates(router, query, cancel))
    }

    fn evaluate(
        &self,
        query: &RouteQuery,
        candidates: CandidateSet,
        cancel: &CancelToken,
    ) -> Result<RouteOutcome, EngineError> {
        let started = Instant::now();
        let CandidateSet { routes, warnings } = candidates;
        if routes.is_empty() {
            return Err(EngineError::ContractViolation("候補が0本です".to_owned()));
        }
        let conditions = query.conditions();
        let mask = query.mode().mask();

        let mut scored = Vec::with_capacity(routes.len());
        for path in &routes {
            cancel.check()?;
            let subdivision = Subdivision::new(&path.coordinates);
            let hits = corridor_hits(&subdivision, &self.snapshot, mask, cancel)?;
            let score = score_route(
                &hits,
                &self.snapshot,
                conditions,
                path.distance_m,
                subdivision.segments.len(),
                cancel,
            )?;
            scored.push(Scored { subdivision, score });
        }
        cancel.check()?;

        // 最短時間経路：(duration, distance) の辞書順で最小、同値は候補順先頭
        let fastest = (0..routes.len())
            .reduce(|best, i| {
                let (a, b) = (&routes[i], &routes[best]);
                if (a.duration_s, a.distance_m) < (b.duration_s, b.distance_m) {
                    i
                } else {
                    best
                }
            })
            .expect("候補は1本以上");
        let fastest_duration = routes[fastest].duration_s;
        let limit_s = fastest_duration + query.max_detour_min() * 60.0 + DETOUR_EPSILON_S;
        let extra: Vec<f64> = routes
            .iter()
            .map(|r| py_round((r.duration_s - fastest_duration) / 60.0, 3))
            .collect();
        let within: Vec<bool> = routes.iter().map(|r| r.duration_s <= limit_s).collect();

        // 全候補で集合Bが空かは件数で判定する（丸めたスコアでは判定しない）
        let any_accident = scored.iter().any(|s| s.score.totals.accident_count > 0);
        let safest = any_accident.then(|| {
            (0..routes.len())
                .filter(|&i| within[i])
                .reduce(|best, i| {
                    let key = |j: usize| {
                        (
                            scored[j].score.weighted_risk,
                            routes[j].duration_s,
                            routes[j].distance_m,
                        )
                    };
                    if key(i) < key(best) { i } else { best }
                })
                .expect("最短時間経路は常に許容内")
        });

        let base_risk = scored[fastest].score.weighted_risk;
        let risk_reduction_percent = safest.and_then(|s| {
            (base_risk > 0.0).then(|| {
                py_round(
                    100.0 * (base_risk - scored[s].score.weighted_risk) / base_risk,
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
                        "取得した{}候補のうち、最短時間経路から+{}分以内で、距離で重み付けしたリスクが最小の経路を選びました。追加{:.1}分、リスク{:.1}%減です。",
                        routes.len(),
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
        let mut warnings = warnings;
        if !any_accident {
            warnings.push(NO_ACCIDENT_WARNING.to_owned());
        }

        let mut evaluated = Vec::with_capacity(routes.len());
        for (i, (path, Scored { subdivision, score })) in routes.into_iter().zip(scored).enumerate()
        {
            cancel.check()?;
            let (high_risk_sections, high_risk_total) =
                display_sections(&subdivision, &score.segment_weights);
            let selected = i == fastest || safest == Some(i);
            let navigation =
                selected.then(|| navigation_data(&subdivision, &score.segment_weights));
            evaluated.push(EvaluatedRoute {
                path,
                totals: score.totals,
                weather_match: score.weather_match,
                weighted_risk: score.weighted_risk,
                risk_per_km: score.risk_per_km,
                accident_points: score.accident_points,
                high_risk_sections,
                high_risk_total,
                navigation,
                extra_minutes: extra[i],
                within_detour: within[i],
            });
        }

        let aggregation_ms = py_round(started.elapsed().as_secs_f64() * 1000.0, 2);
        Ok(RouteOutcome {
            dataset_id: self.dataset.dataset_id.clone(),
            data_years: self.dataset.data_years.clone(),
            mode: query.mode(),
            conditions,
            candidates: evaluated,
            fastest,
            safest,
            extra_minutes: safest.map(|s| extra[s]),
            risk_reduction_percent,
            reason,
            warnings,
            risk_note: risk_note(conditions.weather),
            aggregation_ms,
        })
    }
}
