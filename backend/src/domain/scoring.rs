//! 集合B・件数・スコア・表示事故点・小線分の深刻度（behavior.md 7章）。

use crate::contract::engine::{DISTANCE_WEIGHT_SIGMA_M, MAX_ACCIDENT_POINTS};
use crate::contract::{
    AccidentPoint, AccidentTotals, CancelToken, ConditionFilter, DayType, Interrupt, LonLat,
    Period, Weather,
};

use super::corridor::{CANCEL_CHECK_INTERVAL, CorridorHit};
use super::rounding::py_round;
use super::snapshot::AccidentSnapshot;

/// 深刻度 `10×deaths + injuries`。
pub fn severity(deaths: u16, injuries: u16) -> u64 {
    10 * u64::from(deaths) + u64::from(injuries)
}

/// 距離重み `exp(−d²/(2σ²))`、σ=24m。
pub fn distance_weight(distance_sq: f64) -> f64 {
    (-distance_sq / (2.0 * DISTANCE_WEIGHT_SIGMA_M * DISTANCE_WEIGHT_SIGMA_M)).exp()
}

/// 天候倍率。指定が雨で事故も雨なら2、霧・雪で事故も同じなら4、それ以外は1。
pub fn weather_multiplier(requested: Option<Weather>, accident_weather_code: u8) -> f64 {
    match requested {
        Some(w @ Weather::Rain) if w.code() == accident_weather_code => 2.0,
        Some(w @ (Weather::Fog | Weather::Snow)) if w.code() == accident_weather_code => 4.0,
        _ => 1.0,
    }
}

/// `sample(items, limit)`：件数がlimit以下なら全件、それ以外は `items[floor(i*N/limit)]`。
pub fn sample<T: Copy>(items: &[T], limit: usize) -> Vec<T> {
    if limit == 0 {
        return Vec::new();
    }
    if items.len() <= limit {
        return items.to_vec();
    }
    let n = items.len();
    (0..limit).map(|i| items[i * n / limit]).collect()
}

/// 1候補の集計結果（ナビ区間・選定の前）。
#[derive(Debug, Clone, PartialEq)]
pub struct RouteScore {
    pub totals: AccidentTotals,
    pub weather_match: Option<AccidentTotals>,
    pub weighted_risk: f64,
    pub risk_per_km: f64,
    pub accident_points: Vec<AccidentPoint>,
    /// sidごとの深刻度の和（集合Bの事故を最寄り小線分1本に加算）。長さは小線分数。
    pub segment_weights: Vec<u64>,
}

fn add(totals: &mut AccidentTotals, deaths: u16, injuries: u16) {
    totals.accident_count += 1;
    if deaths > 0 {
        totals.fatal_accidents += 1;
    } else {
        totals.injury_accidents += 1;
    }
    totals.deaths += u64::from(deaths);
    totals.injuries += u64::from(injuries);
}

/// 回廊内の事故（事故ID昇順）から集合Bを作って集計する。天候ではBを減らさない。
pub fn score_route(
    hits: &[CorridorHit],
    snapshot: &AccidentSnapshot,
    conditions: ConditionFilter,
    distance_m: f64,
    segment_count: usize,
    cancel: &CancelToken,
) -> Result<RouteScore, Interrupt> {
    let columns = snapshot.columns();
    let period = conditions.period.map(Period::code);
    let day_type = conditions.day_type.map(DayType::code);
    let mut totals = AccidentTotals::default();
    let mut weather_match = conditions.weather.map(|_| AccidentTotals::default());
    let mut score = 0.0;
    let mut segment_weights = vec![0u64; segment_count];
    let mut fatal = Vec::new();
    let mut other = Vec::new();
    for (i, hit) in hits.iter().enumerate() {
        if i % CANCEL_CHECK_INTERVAL == 0 {
            cancel.check()?;
        }
        let id = hit.accident as usize;
        if period.is_some_and(|p| columns.period[id] != p)
            || day_type.is_some_and(|d| columns.day_type[id] != d)
        {
            continue;
        }
        let (deaths, injuries) = (columns.deaths[id], columns.injuries[id]);
        add(&mut totals, deaths, injuries);
        if let (Some(m), Some(w)) = (weather_match.as_mut(), conditions.weather)
            && columns.weather[id] == w.code()
        {
            add(m, deaths, injuries);
        }
        let s = severity(deaths, injuries);
        score += distance_weight(hit.distance_sq)
            * weather_multiplier(conditions.weather, columns.weather[id])
            * s as f64;
        segment_weights[hit.sid as usize] += s;
        if deaths > 0 {
            fatal.push(hit.accident);
        } else {
            other.push(hit.accident);
        }
    }
    let weighted_risk = py_round(score, 2);
    let risk_per_km = py_round(weighted_risk / (distance_m / 1000.0).max(0.001), 3);
    // 死亡事故を優先し、各群の元順（事故ID昇順）から等間隔に抽出する
    let mut ids = sample(&fatal, MAX_ACCIDENT_POINTS);
    ids.extend(sample(&other, MAX_ACCIDENT_POINTS - ids.len()));
    let accident_points = ids
        .into_iter()
        .map(|id| accident_point(snapshot, id))
        .collect();
    Ok(RouteScore {
        totals,
        weather_match,
        weighted_risk,
        risk_per_km,
        accident_points,
        segment_weights,
    })
}

fn accident_point(snapshot: &AccidentSnapshot, id: u32) -> AccidentPoint {
    let c = snapshot.columns();
    let i = id as usize;
    AccidentPoint {
        id: u64::from(id),
        position: LonLat::new(c.lon[i], c.lat[i]),
        deaths: c.deaths[i],
        injuries: c.injuries[i],
        source_year: c.source_year[i],
        date: c.date[i],
        weather: Weather::from_code(c.weather[i]).expect("ロード時に検証済み"),
        period: Period::from_code(c.period[i]).expect("ロード時に検証済み"),
        day_type: DayType::from_code(c.day_type[i]).expect("ロード時に検証済み"),
    }
}
