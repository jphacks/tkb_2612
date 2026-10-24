//! `RouteOutcome` → `RouteResponse` のJSON（openapi.yaml）。
//!
//! ID文字列・`api_version`・`exact_condition_count`・`points_shown`・`effective_conditions` の
//! 表記・固定値（`relaxed_conditions`・`radius_m`・`distance_weight_sigma_m`）だけを組み立て、
//! 数値・件数・順序・選定・文言は②の値をそのまま使う（再計算しない）。
//!
//! 変換の前に相関制約・OpenAPIの値の範囲・JSONの安全整数範囲・有限値・navigationのサイズ
//! （5,000,000バイト）を検査し、違反は内部契約違反（500）にする。間引いた200は返さない。

use std::io;

use serde::ser::{Serialize, SerializeSeq, SerializeTuple, Serializer};

use crate::contract::engine::{
    DISTANCE_WEIGHT_SIGMA_M, MAX_ACCIDENT_POINTS, MAX_CANDIDATES, MAX_HIGH_RISK_SECTIONS, RADIUS_M,
};
use crate::contract::geo::{LAT_RANGE, LON_RANGE};
use crate::contract::{
    AccidentPoint, AccidentTotals, EvaluatedRoute, HighRiskSegment, LonLat, NavigationData,
    RouteOutcome,
};

/// JSONの安全整数の上限（2^53−1）。
pub const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;
/// 選定経路分のnavigationの合計（空白なしUTF-8 JSON）の上限。
pub const MAX_NAVIGATION_BYTES: u64 = 5_000_000;
/// OpenAPIの `NavigationData`・`NavigationSection` の範囲。
pub const NAV_PROJECTION_LAT_RANGE: std::ops::RangeInclusive<f64> = 20.0..=46.0;
pub const NAV_MAX_LENGTH_M: f64 = 110_000.0;
pub const NAV_MAX_VERTICES: usize = 15_000;
pub const NAV_MAX_SECTIONS: usize = 8_233;
pub const NAV_MAX_SEGMENT_INDEX: u32 = 16_465;
/// OpenAPIの `LineString`（頂点数）・`RouteCandidate`（`distance`）の範囲。
pub const ROUTE_VERTICES_RANGE: std::ops::RangeInclusive<usize> = 2..=15_000;
pub const ROUTE_MAX_DISTANCE_M: f64 = 80_000.0;
/// OpenAPIの `AccidentFeature`・`Years` の範囲。
pub const ACCIDENT_MAX_PEOPLE: u16 = 999;
pub const MIN_SOURCE_YEAR: u16 = 1900;
pub const ACCIDENT_DATE_RANGE: std::ops::RangeInclusive<u32> = 19_000_101..=99_991_231;

/// 変換できない結果（内部契約違反）。内容は内部の説明で、応答には出さない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidOutcome(pub String);

/// 応答JSON（空白なしUTF-8）を作る。
pub fn render_route_response(outcome: &RouteOutcome) -> Result<Vec<u8>, InvalidOutcome> {
    check_outcome(outcome)?;
    let dto = RouteResponseDto::new(outcome);
    let nav_bytes = navigation_bytes(&dto);
    if nav_bytes > MAX_NAVIGATION_BYTES {
        return Err(InvalidOutcome(format!(
            "navigationが{MAX_NAVIGATION_BYTES}バイトを超えた"
        )));
    }
    serde_json::to_vec(&dto).map_err(|e| InvalidOutcome(format!("JSONにできない: {e}")))
}

/// 選定経路分（同一IDは1回）のnavigationのバイト数。
fn navigation_bytes(dto: &RouteResponseDto<'_>) -> u64 {
    dto.candidates
        .iter()
        .filter_map(|c| c.navigation.as_ref())
        .map(|nav| {
            let mut counter = ByteCounter(0);
            serde_json::to_writer(&mut counter, nav).expect("有限値だけのJSON");
            counter.0
        })
        .sum()
}

struct ByteCounter(u64);

impl io::Write for ByteCounter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn route_id(index: usize) -> String {
    format!("route-{}", index + 1)
}

// ---- 検査 ---------------------------------------------------------------------

fn violation<T>(message: impl Into<String>) -> Result<T, InvalidOutcome> {
    Err(InvalidOutcome(message.into()))
}

fn safe(value: u64, what: &str) -> Result<(), InvalidOutcome> {
    if value > MAX_SAFE_INTEGER {
        return violation(format!("{what}がJSONの安全整数範囲を超えた"));
    }
    Ok(())
}

fn finite(value: f64, what: &str) -> Result<(), InvalidOutcome> {
    if !value.is_finite() {
        return violation(format!("{what}が有限値でない"));
    }
    Ok(())
}

fn finite_non_negative(value: f64, what: &str) -> Result<(), InvalidOutcome> {
    finite(value, what)?;
    if value < 0.0 {
        return violation(format!("{what}が負"));
    }
    Ok(())
}

/// `Position`：有限で、日本の矩形（経度122〜154・緯度20〜46）の内側。
fn check_position(p: LonLat, what: &str) -> Result<(), InvalidOutcome> {
    finite(p.lon, what)?;
    finite(p.lat, what)?;
    if !LON_RANGE.contains(&p.lon) || !LAT_RANGE.contains(&p.lat) {
        return violation(format!("{what}: 座標が日本の範囲外"));
    }
    Ok(())
}

fn check_totals(t: &AccidentTotals, what: &str) -> Result<(), InvalidOutcome> {
    for value in [
        t.accident_count,
        t.fatal_accidents,
        t.injury_accidents,
        t.deaths,
        t.injuries,
    ] {
        safe(value, what)?;
    }
    if t.fatal_accidents + t.injury_accidents != t.accident_count {
        return violation(format!("{what}: 死亡事故+負傷事故がaccident_countと異なる"));
    }
    Ok(())
}

/// openapi.yamlの相関制約と、JSONにできる値であることを検査する。
pub fn check_outcome(outcome: &RouteOutcome) -> Result<(), InvalidOutcome> {
    let n = outcome.candidates.len();
    if !(1..=MAX_CANDIDATES).contains(&n) {
        return violation(format!("候補数{n}が1〜6の範囲外"));
    }
    if outcome.fastest >= n || outcome.safest.is_some_and(|s| s >= n) {
        return violation("fastest・safestが候補にない");
    }
    if outcome.dataset_id.len() != 64
        || !outcome
            .dataset_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return violation("dataset_idが64桁の小文字hexでない");
    }
    let any_accident = outcome
        .candidates
        .iter()
        .any(|c| c.totals.accident_count > 0);
    if outcome.safest.is_some() != any_accident {
        return violation("safestの有無が集合Bの有無と一致しない");
    }
    if outcome.extra_minutes.is_some() != outcome.safest.is_some()
        || (outcome.safest.is_none() && outcome.risk_reduction_percent.is_some())
    {
        return violation("extra_minutes・risk_reduction_percentの有無が不正");
    }
    if let Some(m) = outcome.extra_minutes {
        finite_non_negative(m, "extra_minutes")?;
    }
    if let Some(p) = outcome.risk_reduction_percent {
        finite(p, "risk_reduction_percent")?;
        if !(0.0..=100.0).contains(&p) {
            return violation("risk_reduction_percentが0〜100の範囲外");
        }
    }
    finite_non_negative(outcome.aggregation_ms, "aggregation_ms")?;
    if outcome.reason.is_empty() || outcome.risk_note.is_empty() {
        return violation("reason・risk_noteが空");
    }
    if outcome.data_years.iter().any(|&y| y < MIN_SOURCE_YEAR)
        || outcome.data_years.windows(2).any(|w| w[0] >= w[1])
    {
        return violation("data_yearsが1900以上の昇順・重複なしでない");
    }
    for (i, c) in outcome.candidates.iter().enumerate() {
        check_candidate(outcome, i, c)?;
    }
    Ok(())
}

fn check_candidate(
    outcome: &RouteOutcome,
    i: usize,
    c: &EvaluatedRoute,
) -> Result<(), InvalidOutcome> {
    let id = route_id(i);
    finite(c.path.duration_s, &id)?;
    finite(c.path.distance_m, &id)?;
    if c.path.duration_s <= 0.0
        || c.path.distance_m <= 0.0
        || c.path.distance_m > ROUTE_MAX_DISTANCE_M
    {
        return violation(format!("{id}: duration・distanceが範囲外"));
    }
    if !ROUTE_VERTICES_RANGE.contains(&c.path.coordinates.len()) {
        return violation(format!("{id}: geometryの頂点数が範囲外"));
    }
    for p in &c.path.coordinates {
        check_position(*p, &id)?;
    }
    check_totals(&c.totals, &id)?;
    if c.weather_match.is_some() != outcome.conditions.weather.is_some() {
        return violation(format!("{id}: weather_matchの有無が天候指定と一致しない"));
    }
    if let Some(w) = &c.weather_match {
        check_totals(w, &id)?;
        if w.accident_count > c.totals.accident_count {
            return violation(format!("{id}: weather_matchの件数が不正"));
        }
    }
    finite_non_negative(c.weighted_risk, &id)?;
    finite_non_negative(c.risk_per_km, &id)?;
    finite_non_negative(c.extra_minutes, &id)?;
    let points = (c.totals.accident_count as usize).min(MAX_ACCIDENT_POINTS);
    if c.accident_points.len() != points {
        return violation(format!("{id}: 事故点数がmin(accident_count,250)と異なる"));
    }
    for p in &c.accident_points {
        safe(p.id, &id)?;
        check_position(p.position, &id)?;
        if p.deaths > ACCIDENT_MAX_PEOPLE
            || p.injuries > ACCIDENT_MAX_PEOPLE
            || p.source_year < MIN_SOURCE_YEAR
            || !ACCIDENT_DATE_RANGE.contains(&p.date)
        {
            return violation(format!("{id}: 事故点の属性が範囲外"));
        }
    }
    safe(c.high_risk_total, &id)?;
    let sections = usize::try_from(c.high_risk_total)
        .unwrap_or(usize::MAX)
        .min(MAX_HIGH_RISK_SECTIONS);
    if c.high_risk_sections.len() != sections {
        return violation(format!(
            "{id}: 表示区間数がmin(high_risk_total,100)と異なる"
        ));
    }
    for s in &c.high_risk_sections {
        safe(s.severity_weight, &id)?;
        check_position(s.start, &id)?;
        check_position(s.end, &id)?;
        if s.severity_weight < 3
            || (s.source_vertex_start_index as usize) + 1 >= c.path.coordinates.len()
        {
            return violation(format!("{id}: 表示区間が不正"));
        }
    }
    let selected = i == outcome.fastest || outcome.safest == Some(i);
    match (&c.navigation, selected) {
        (Some(nav), true) => check_navigation(&id, c, nav),
        (None, false) => Ok(()),
        (Some(_), false) => violation(format!("{id}: 未選定なのにnavigationがある")),
        (None, true) => violation(format!("{id}: 選定候補なのにnavigationがない")),
    }
}

fn check_navigation(
    id: &str,
    c: &EvaluatedRoute,
    nav: &NavigationData,
) -> Result<(), InvalidOutcome> {
    finite(nav.projection_lat_deg, id)?;
    finite_non_negative(nav.length_m, id)?;
    if !NAV_PROJECTION_LAT_RANGE.contains(&nav.projection_lat_deg) {
        return violation(format!("{id}: projection_lat_degが範囲外"));
    }
    // 頂点の距離は非減少で末尾がlength_mなので、各距離・区間の端もこの上限以下になる
    if nav.length_m > NAV_MAX_LENGTH_M {
        return violation(format!("{id}: length_mが範囲外"));
    }
    let d = &nav.vertex_distances_m;
    if d.len() != c.path.coordinates.len() {
        return violation(format!("{id}: vertex_distances_mの長さが頂点数と異なる"));
    }
    if !(2..=NAV_MAX_VERTICES).contains(&d.len()) {
        return violation(format!("{id}: vertex_distances_mの長さが範囲外"));
    }
    if nav.sections.len() > NAV_MAX_SECTIONS {
        return violation(format!("{id}: ナビ区間数が範囲外"));
    }
    for v in d {
        finite_non_negative(*v, id)?;
    }
    if d.first() != Some(&0.0) || d.last() != Some(&nav.length_m) {
        return violation(format!("{id}: vertex_distances_mの先頭・末尾が不正"));
    }
    if d.windows(2).any(|w| w[0] > w[1]) {
        return violation(format!("{id}: vertex_distances_mが非減少でない"));
    }
    let mut covered = 0u64;
    let mut previous_end: Option<u32> = None;
    let mut previous_end_m = 0.0;
    for s in &nav.sections {
        finite(s.start_m, id)?;
        finite(s.end_m, id)?;
        safe(s.severity_weight, id)?;
        if s.start_segment_index > s.end_segment_index
            || s.end_segment_index > NAV_MAX_SEGMENT_INDEX
            || !(0.0 <= s.start_m && s.start_m <= s.end_m && s.end_m <= nav.length_m)
            || s.severity_weight < 3
        {
            return violation(format!("{id}: ナビ区間が不正"));
        }
        if previous_end.is_some_and(|e| s.start_segment_index <= e.saturating_add(1)) {
            return violation(format!("{id}: ナビ区間の順序・間隔が不正"));
        }
        // 距離も区間の順に進む（0長の区間と、前区間の終わりちょうどからの開始は認める）
        if s.start_m < previous_end_m {
            return violation(format!("{id}: ナビ区間の距離が逆行・重複している"));
        }
        covered += u64::from(s.end_segment_index - s.start_segment_index) + 1;
        previous_end = Some(s.end_segment_index);
        previous_end_m = s.end_m;
    }
    if covered != c.high_risk_total {
        return violation(format!("{id}: Σ(E−S+1)がhigh_risk_totalと異なる"));
    }
    Ok(())
}

// ---- DTO --------------------------------------------------------------------

/// GeoJSONの `[lon, lat]`。
struct Position(LonLat);

impl Serialize for Position {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut tuple = serializer.serialize_tuple(2)?;
        tuple.serialize_element(&self.0.lon)?;
        tuple.serialize_element(&self.0.lat)?;
        tuple.end()
    }
}

struct Positions<'a>(&'a [LonLat]);

impl Serialize for Positions<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
        for p in self.0 {
            seq.serialize_element(&Position(*p))?;
        }
        seq.end()
    }
}

#[derive(serde::Serialize)]
struct LineStringDto<'a> {
    r#type: &'static str,
    coordinates: Positions<'a>,
}

#[derive(serde::Serialize)]
struct PointDto {
    r#type: &'static str,
    coordinates: Position,
}

#[derive(serde::Serialize)]
struct FeatureCollection<T> {
    r#type: &'static str,
    features: Vec<T>,
}

impl<T> FeatureCollection<T> {
    fn new(features: Vec<T>) -> Self {
        Self {
            r#type: "FeatureCollection",
            features,
        }
    }
}

#[derive(serde::Serialize)]
struct AccidentPropertiesDto {
    deaths: u16,
    injuries: u16,
    source_year: u16,
    date: u32,
    weather: &'static str,
    period: &'static str,
    day_type: &'static str,
}

#[derive(serde::Serialize)]
struct AccidentFeatureDto {
    r#type: &'static str,
    id: u64,
    geometry: PointDto,
    properties: AccidentPropertiesDto,
}

impl From<&AccidentPoint> for AccidentFeatureDto {
    fn from(p: &AccidentPoint) -> Self {
        Self {
            r#type: "Feature",
            id: p.id,
            geometry: PointDto {
                r#type: "Point",
                coordinates: Position(p.position),
            },
            properties: AccidentPropertiesDto {
                deaths: p.deaths,
                injuries: p.injuries,
                source_year: p.source_year,
                date: p.date,
                weather: p.weather.label(),
                period: p.period.label(),
                day_type: p.day_type.label(),
            },
        }
    }
}

#[derive(serde::Serialize)]
struct HighRiskPropertiesDto {
    route_id: String,
    severity_weight: u64,
    segment_index: u32,
    source_vertex_start_index: u32,
    source_vertex_end_index: u32,
}

/// 表示区間の小線分（両端の2点）。
#[derive(serde::Serialize)]
struct SegmentLineDto {
    r#type: &'static str,
    coordinates: [Position; 2],
}

#[derive(serde::Serialize)]
struct HighRiskFeatureDto {
    r#type: &'static str,
    id: String,
    geometry: SegmentLineDto,
    properties: HighRiskPropertiesDto,
}

impl HighRiskFeatureDto {
    fn new(route: &str, s: &HighRiskSegment) -> Self {
        Self {
            r#type: "Feature",
            id: format!("{route}:segment-{}", s.segment_index),
            geometry: SegmentLineDto {
                r#type: "LineString",
                coordinates: [Position(s.start), Position(s.end)],
            },
            properties: HighRiskPropertiesDto {
                route_id: route.to_owned(),
                severity_weight: s.severity_weight,
                segment_index: s.segment_index,
                source_vertex_start_index: s.source_vertex_start_index,
                source_vertex_end_index: s.source_vertex_start_index + 1,
            },
        }
    }
}

#[derive(serde::Serialize)]
struct NavigationSectionDto {
    id: String,
    start_segment_index: u32,
    end_segment_index: u32,
    start_m: f64,
    end_m: f64,
    severity_weight: u64,
}

#[derive(serde::Serialize)]
struct NavigationDto<'a> {
    projection_lat_deg: f64,
    length_m: f64,
    vertex_distances_m: &'a [f64],
    sections: Vec<NavigationSectionDto>,
}

impl<'a> NavigationDto<'a> {
    fn new(route: &str, nav: &'a NavigationData) -> Self {
        Self {
            projection_lat_deg: nav.projection_lat_deg,
            length_m: nav.length_m,
            vertex_distances_m: &nav.vertex_distances_m,
            sections: nav
                .sections
                .iter()
                .map(|s| NavigationSectionDto {
                    id: format!(
                        "{route}:nav-{}-{}",
                        s.start_segment_index, s.end_segment_index
                    ),
                    start_segment_index: s.start_segment_index,
                    end_segment_index: s.end_segment_index,
                    start_m: s.start_m,
                    end_m: s.end_m,
                    severity_weight: s.severity_weight,
                })
                .collect(),
        }
    }
}

#[derive(serde::Serialize)]
struct TotalsDto {
    accident_count: u64,
    fatal_accidents: u64,
    injury_accidents: u64,
    deaths: u64,
    injuries: u64,
}

impl From<&AccidentTotals> for TotalsDto {
    fn from(t: &AccidentTotals) -> Self {
        Self {
            accident_count: t.accident_count,
            fatal_accidents: t.fatal_accidents,
            injury_accidents: t.injury_accidents,
            deaths: t.deaths,
            injuries: t.injuries,
        }
    }
}

#[derive(serde::Serialize)]
struct CandidateDto<'a> {
    id: String,
    duration: f64,
    distance: f64,
    geometry: LineStringDto<'a>,
    accident_count: u64,
    fatal_accidents: u64,
    injury_accidents: u64,
    deaths: u64,
    injuries: u64,
    weather_match: Option<TotalsDto>,
    weighted_risk: f64,
    risk_per_km: f64,
    accident_points: FeatureCollection<AccidentFeatureDto>,
    points_shown: usize,
    high_risk_sections: FeatureCollection<HighRiskFeatureDto>,
    high_risk_total: u64,
    navigation: Option<NavigationDto<'a>>,
    exact_condition_count: u64,
    extra_minutes: f64,
    within_detour: bool,
}

impl<'a> CandidateDto<'a> {
    fn new(index: usize, c: &'a EvaluatedRoute) -> Self {
        let id = route_id(index);
        let high_risk = c
            .high_risk_sections
            .iter()
            .map(|s| HighRiskFeatureDto::new(&id, s))
            .collect();
        Self {
            duration: c.path.duration_s,
            distance: c.path.distance_m,
            geometry: LineStringDto {
                r#type: "LineString",
                coordinates: Positions(&c.path.coordinates),
            },
            accident_count: c.totals.accident_count,
            fatal_accidents: c.totals.fatal_accidents,
            injury_accidents: c.totals.injury_accidents,
            deaths: c.totals.deaths,
            injuries: c.totals.injuries,
            weather_match: c.weather_match.as_ref().map(TotalsDto::from),
            weighted_risk: c.weighted_risk,
            risk_per_km: c.risk_per_km,
            points_shown: c.accident_points.len(),
            accident_points: FeatureCollection::new(
                c.accident_points
                    .iter()
                    .map(AccidentFeatureDto::from)
                    .collect(),
            ),
            high_risk_sections: FeatureCollection::new(high_risk),
            high_risk_total: c.high_risk_total,
            navigation: c
                .navigation
                .as_ref()
                .map(|nav| NavigationDto::new(&id, nav)),
            exact_condition_count: c
                .weather_match
                .as_ref()
                .map_or(c.totals.accident_count, |w| w.accident_count),
            extra_minutes: c.extra_minutes,
            within_detour: c.within_detour,
            id,
        }
    }
}

#[derive(serde::Serialize)]
struct EffectiveConditionsDto {
    mode: &'static str,
    period: &'static str,
    day_type: &'static str,
    weather: &'static str,
}

#[derive(serde::Serialize)]
struct RouteResponseDto<'a> {
    api_version: &'static str,
    dataset_id: &'a str,
    fastest_id: String,
    safest_id: Option<String>,
    candidates: Vec<CandidateDto<'a>>,
    extra_minutes: Option<f64>,
    risk_reduction_percent: Option<f64>,
    reason: &'a str,
    warnings: &'a [String],
    relaxed_conditions: [&'static str; 0],
    effective_conditions: EffectiveConditionsDto,
    radius_m: f64,
    distance_weight_sigma_m: f64,
    aggregation_ms: f64,
    data_years: &'a [u16],
    risk_note: &'a str,
}

impl<'a> RouteResponseDto<'a> {
    fn new(outcome: &'a RouteOutcome) -> Self {
        let conditions = outcome.conditions;
        Self {
            api_version: "2.0",
            dataset_id: &outcome.dataset_id,
            fastest_id: route_id(outcome.fastest),
            safest_id: outcome.safest.map(route_id),
            candidates: outcome
                .candidates
                .iter()
                .enumerate()
                .map(|(i, c)| CandidateDto::new(i, c))
                .collect(),
            extra_minutes: outcome.extra_minutes,
            risk_reduction_percent: outcome.risk_reduction_percent,
            reason: &outcome.reason,
            warnings: &outcome.warnings,
            relaxed_conditions: [],
            effective_conditions: EffectiveConditionsDto {
                mode: outcome.mode.as_str(),
                period: conditions.period.map_or("全時間帯", |p| p.label()),
                day_type: conditions.day_type.map_or("全曜日", |d| d.label()),
                weather: conditions.weather.map_or("全天候", |w| w.label()),
            },
            radius_m: RADIUS_M,
            distance_weight_sigma_m: DISTANCE_WEIGHT_SIGMA_M,
            aggregation_ms: outcome.aggregation_ms,
            data_years: &outcome.data_years,
            risk_note: &outcome.risk_note,
        }
    }
}
