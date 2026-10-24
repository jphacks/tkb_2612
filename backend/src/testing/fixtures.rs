//! 共通fixtureの読込。②と③が同じ入力・期待値を使う。
//!
//! - 入力：`docs/api/fixtures/route-example-inputs.json`（人工事故・要求・固定経路）
//! - 期待値：`backend/tests/fixtures/route-responses.json`（openapi.yamlの応答例から生成）
//!
//! fixtureは仕様の一部なので、読めない・形が違う場合はpanicする。

use std::fs;
use std::path::PathBuf;

use serde_json::Value;

use crate::contract::engine::{MAX_ACCIDENT_POINTS, MAX_HIGH_RISK_SECTIONS};
use crate::contract::{
    AccidentPoint, AccidentTotals, ConditionFilter, DatasetInfo, DayType, EvaluatedRoute,
    HighRiskSegment, LatLon, LonLat, Mode, NavigationData, NavigationSection, Period, RouteOutcome,
    RouteQuery, RoutedPath, Weather,
};

/// `docs/api/fixtures/` 配下のパス。
pub fn docs_fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../docs/api/fixtures")
        .join(name)
}

/// `backend/tests/fixtures/` 配下のパス。
pub fn backend_fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn read_json(path: PathBuf) -> Value {
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("fixtureを読めません: {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("fixtureのJSONが不正です: {}: {e}", path.display()))
}

/// route-example-inputs.jsonの人工事故1件。②はこれから検証用のスナップショットを作る。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FixtureAccident {
    pub point: AccidentPoint,
    /// `Mode::mask` のOR。
    pub modes: u8,
}

/// route-example-inputs.jsonの例1件。
#[derive(Debug, Clone, PartialEq)]
pub struct RouteExample {
    pub name: String,
    pub query: RouteQuery,
    /// この例で使う事故ID（全候補共通の入力）。
    pub accident_ids: Vec<u64>,
    /// 固定経路（提供元の順）。`ScriptedRouter` の初回応答に使う。
    pub routes: Vec<RoutedPath>,
}

pub fn route_example_accidents() -> Vec<FixtureAccident> {
    let doc = read_json(docs_fixture_path("route-example-inputs.json"));
    array(&doc, "accidents")
        .iter()
        .map(|feature| FixtureAccident {
            point: parse_accident_feature(feature),
            modes: array(feature, "modes")
                .iter()
                .map(|m| {
                    Mode::parse(as_str(m, "modes[]"))
                        .expect("modesはwalk/bike/car")
                        .mask()
                })
                .fold(0, |acc, m| acc | m),
        })
        .collect()
}

pub fn route_examples() -> Vec<RouteExample> {
    let doc = read_json(docs_fixture_path("route-example-inputs.json"));
    array(&doc, "examples")
        .iter()
        .map(|example| RouteExample {
            name: as_str(field(example, "name"), "name").to_owned(),
            query: parse_canonical_request(field(example, "request")),
            accident_ids: array(example, "accident_ids")
                .iter()
                .map(|v| as_u64(v, "accident_ids[]"))
                .collect(),
            routes: array(example, "routes")
                .iter()
                .map(|r| RoutedPath {
                    duration_s: as_f64(field(r, "duration"), "duration"),
                    distance_m: as_f64(field(r, "distance"), "distance"),
                    coordinates: parse_line_string(field(r, "geometry")),
                })
                .collect(),
        })
        .collect()
}

pub fn route_example(name: &str) -> RouteExample {
    route_examples()
        .into_iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("route example {name} がありません"))
}

/// 応答例の名前の一覧（route-responses.json）。
pub fn route_response_names() -> Vec<String> {
    let doc = read_json(backend_fixture_path("route-responses.json"));
    field(&doc, "examples")
        .as_object()
        .expect("examplesはobject")
        .keys()
        .cloned()
        .collect()
}

/// 応答例の生JSON。③がDTO変換の結果と比べるときに使う。
pub fn route_response_json(name: &str) -> Value {
    let doc = read_json(backend_fixture_path("route-responses.json"));
    field(field(&doc, "examples"), name).clone()
}

/// 応答例をドメイン結果へ戻したもの。②の計算結果の期待値で、③のDTO変換の入力にもなる。
/// `aggregation_ms` は計測値なので比較から除く。
pub fn expected_route_outcome(name: &str) -> RouteOutcome {
    parse_route_response(&route_response_json(name))
}

/// 既存3例（`singleRoute`・`noAccidents`・`twoSelected`）が共有する人工事故の件数（ID 0〜2）。
/// 間引きの例のための大量の事故（ID 3〜）は数えない。
const BASE_ACCIDENT_COUNT: u64 = 3;

/// 応答例の集計に使った人工データの識別情報。`accidents` は既存3例の人工事故3件。
pub fn fixture_dataset_info() -> DatasetInfo {
    let zero = "0".repeat(64);
    DatasetInfo {
        release_id: zero.clone(),
        dataset_id: zero,
        accidents: BASE_ACCIDENT_COUNT,
        data_years: vec![2025],
        load_seconds: 0.0,
        grid_cells: None,
    }
}

/// `RouteOutcome` の相関制約（openapi.yaml・behavior.md 7〜8章）を検査する。
/// ②は計算結果に、③はDTO変換前の入力に使える。
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

fn parse_canonical_request(v: &Value) -> RouteQuery {
    let conditions = ConditionFilter {
        period: optional_label(v, "period", Period::from_label),
        day_type: optional_label(v, "day_type", DayType::from_label),
        weather: optional_label(v, "weather", Weather::from_label),
    };
    RouteQuery::new(
        parse_lat_lon(field(v, "origin")),
        parse_lat_lon(field(v, "dest")),
        v.get("mode").map_or(Mode::Walk, |m| {
            Mode::parse(as_str(m, "mode")).expect("mode")
        }),
        v.get("max_detour_min")
            .map_or(10.0, |m| as_f64(m, "max_detour_min")),
        conditions,
    )
    .expect("fixtureの要求は有効")
}

/// 正規入力（日本語ラベルまたはall）だけを受ける。
fn optional_label<T>(v: &Value, key: &str, parse: fn(&str) -> Option<T>) -> Option<T> {
    match v.get(key).map(|x| as_str(x, key)) {
        None | Some("all") => None,
        Some(label) => Some(parse(label).unwrap_or_else(|| panic!("{key}の値が不正: {label}"))),
    }
}

/// `effective_conditions` の値。未指定の表記（全時間帯等）は `None`。
fn effective_label<T>(
    v: &Value,
    key: &str,
    unspecified: &str,
    parse: fn(&str) -> Option<T>,
) -> Option<T> {
    let label = as_str(field(v, key), key);
    if label == unspecified {
        return None;
    }
    Some(parse(label).unwrap_or_else(|| panic!("{key}の値が不正: {label}")))
}

fn parse_route_response(v: &Value) -> RouteOutcome {
    let candidates: Vec<EvaluatedRoute> = array(v, "candidates")
        .iter()
        .enumerate()
        .map(|(i, c)| {
            assert_eq!(as_str(field(c, "id"), "id"), format!("route-{}", i + 1));
            parse_candidate(c)
        })
        .collect();
    let effective = field(v, "effective_conditions");
    RouteOutcome {
        dataset_id: as_str(field(v, "dataset_id"), "dataset_id").to_owned(),
        data_years: array(v, "data_years")
            .iter()
            .map(|y| as_u64(y, "data_years[]") as u16)
            .collect(),
        mode: Mode::parse(as_str(field(effective, "mode"), "mode")).expect("mode"),
        conditions: ConditionFilter {
            period: effective_label(effective, "period", "全時間帯", Period::from_label),
            day_type: effective_label(effective, "day_type", "全曜日", DayType::from_label),
            weather: effective_label(effective, "weather", "全天候", Weather::from_label),
        },
        fastest: route_index(field(v, "fastest_id")),
        safest: nullable(field(v, "safest_id")).map(route_index),
        extra_minutes: nullable(field(v, "extra_minutes")).map(|x| as_f64(x, "extra_minutes")),
        risk_reduction_percent: nullable(field(v, "risk_reduction_percent"))
            .map(|x| as_f64(x, "risk_reduction_percent")),
        reason: as_str(field(v, "reason"), "reason").to_owned(),
        warnings: array(v, "warnings")
            .iter()
            .map(|w| as_str(w, "warnings[]").to_owned())
            .collect(),
        risk_note: as_str(field(v, "risk_note"), "risk_note").to_owned(),
        aggregation_ms: as_f64(field(v, "aggregation_ms"), "aggregation_ms"),
        candidates,
    }
}

fn parse_candidate(c: &Value) -> EvaluatedRoute {
    EvaluatedRoute {
        path: RoutedPath {
            duration_s: as_f64(field(c, "duration"), "duration"),
            distance_m: as_f64(field(c, "distance"), "distance"),
            coordinates: parse_line_string(field(c, "geometry")),
        },
        totals: parse_totals(c),
        weather_match: nullable(field(c, "weather_match")).map(parse_totals),
        weighted_risk: as_f64(field(c, "weighted_risk"), "weighted_risk"),
        risk_per_km: as_f64(field(c, "risk_per_km"), "risk_per_km"),
        accident_points: array(field(c, "accident_points"), "features")
            .iter()
            .map(parse_accident_feature)
            .collect(),
        high_risk_sections: array(field(c, "high_risk_sections"), "features")
            .iter()
            .map(parse_high_risk_feature)
            .collect(),
        high_risk_total: as_u64(field(c, "high_risk_total"), "high_risk_total"),
        navigation: nullable(field(c, "navigation")).map(parse_navigation),
        extra_minutes: as_f64(field(c, "extra_minutes"), "extra_minutes"),
        within_detour: field(c, "within_detour")
            .as_bool()
            .expect("within_detourはbool"),
    }
}

fn parse_totals(v: &Value) -> AccidentTotals {
    let get = |key: &str| as_u64(field(v, key), key);
    AccidentTotals {
        accident_count: get("accident_count"),
        fatal_accidents: get("fatal_accidents"),
        injury_accidents: get("injury_accidents"),
        deaths: get("deaths"),
        injuries: get("injuries"),
    }
}

fn parse_accident_feature(f: &Value) -> AccidentPoint {
    let p = field(f, "properties");
    let geometry = field(f, "geometry");
    assert_eq!(as_str(field(geometry, "type"), "geometry.type"), "Point");
    AccidentPoint {
        id: as_u64(field(f, "id"), "id"),
        position: parse_position(field(geometry, "coordinates")),
        deaths: as_u64(field(p, "deaths"), "deaths") as u16,
        injuries: as_u64(field(p, "injuries"), "injuries") as u16,
        source_year: as_u64(field(p, "source_year"), "source_year") as u16,
        date: as_u64(field(p, "date"), "date") as u32,
        weather: Weather::from_label(as_str(field(p, "weather"), "weather")).expect("weather"),
        period: Period::from_label(as_str(field(p, "period"), "period")).expect("period"),
        day_type: DayType::from_label(as_str(field(p, "day_type"), "day_type")).expect("day_type"),
    }
}

fn parse_high_risk_feature(f: &Value) -> HighRiskSegment {
    let p = field(f, "properties");
    let coordinates = parse_line_string(field(f, "geometry"));
    assert_eq!(coordinates.len(), 2, "表示区間は両端だけ");
    let start_index = as_u64(
        field(p, "source_vertex_start_index"),
        "source_vertex_start_index",
    );
    assert_eq!(
        as_u64(
            field(p, "source_vertex_end_index"),
            "source_vertex_end_index"
        ),
        start_index + 1
    );
    HighRiskSegment {
        segment_index: as_u64(field(p, "segment_index"), "segment_index") as u32,
        source_vertex_start_index: start_index as u32,
        start: coordinates[0],
        end: coordinates[1],
        severity_weight: as_u64(field(p, "severity_weight"), "severity_weight"),
    }
}

fn parse_navigation(v: &Value) -> NavigationData {
    NavigationData {
        projection_lat_deg: as_f64(field(v, "projection_lat_deg"), "projection_lat_deg"),
        length_m: as_f64(field(v, "length_m"), "length_m"),
        vertex_distances_m: array(v, "vertex_distances_m")
            .iter()
            .map(|d| as_f64(d, "vertex_distances_m[]"))
            .collect(),
        sections: array(v, "sections")
            .iter()
            .map(|s| NavigationSection {
                start_segment_index: as_u64(field(s, "start_segment_index"), "start_segment_index")
                    as u32,
                end_segment_index: as_u64(field(s, "end_segment_index"), "end_segment_index")
                    as u32,
                start_m: as_f64(field(s, "start_m"), "start_m"),
                end_m: as_f64(field(s, "end_m"), "end_m"),
                severity_weight: as_u64(field(s, "severity_weight"), "severity_weight"),
            })
            .collect(),
    }
}

fn parse_line_string(v: &Value) -> Vec<LonLat> {
    assert_eq!(as_str(field(v, "type"), "geometry.type"), "LineString");
    array(v, "coordinates").iter().map(parse_position).collect()
}

/// GeoJSONの `[lon, lat]`。
fn parse_position(v: &Value) -> LonLat {
    let pair = v.as_array().expect("Positionは配列");
    assert_eq!(pair.len(), 2, "Positionは[lon, lat]の2要素");
    LonLat::new(as_f64(&pair[0], "lon"), as_f64(&pair[1], "lat"))
}

fn parse_lat_lon(v: &Value) -> LatLon {
    LatLon::new(
        as_f64(field(v, "lat"), "lat"),
        as_f64(field(v, "lon"), "lon"),
    )
}

fn route_index(v: &Value) -> usize {
    let id = as_str(v, "route id");
    let n: usize = id
        .strip_prefix("route-")
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("経路IDが不正: {id}"));
    n - 1
}

fn nullable(v: &Value) -> Option<&Value> {
    (!v.is_null()).then_some(v)
}

fn field<'a>(v: &'a Value, key: &str) -> &'a Value {
    v.get(key)
        .unwrap_or_else(|| panic!("fixtureに{key}がありません"))
}

fn array<'a>(v: &'a Value, key: &str) -> &'a Vec<Value> {
    field(v, key)
        .as_array()
        .unwrap_or_else(|| panic!("{key}は配列"))
}

fn as_str<'a>(v: &'a Value, what: &str) -> &'a str {
    v.as_str().unwrap_or_else(|| panic!("{what}は文字列"))
}

fn as_f64(v: &Value, what: &str) -> f64 {
    v.as_f64().unwrap_or_else(|| panic!("{what}は数値"))
}

fn as_u64(v: &Value, what: &str) -> u64 {
    v.as_u64().unwrap_or_else(|| panic!("{what}は非負整数"))
}
