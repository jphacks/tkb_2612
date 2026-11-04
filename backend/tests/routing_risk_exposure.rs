//! 道路区分ごとの曝露の補正（計画書5章、形式書14.4・14.5節）のテスト。
//!
//! - `risk-fit` の係数の計算（route-code・osm・factors）。
//! - `PARAMS.car_exposure` の形と読込時の検査。
//! - エンジン（`RiskView`）が辺・交差点クラスタに係数を掛けること。
//! - エンジンを通した区分ごとのリスクの合計が、与えた事故率の比を再現すること。
use accident_route_backend::contract::{ConditionFilter, Mode};
use accident_route_backend::data::routing_files::{Deps, decode_risk, encode_risk, required_deps};
use accident_route_backend::domain::routing::accident::{
    AccidentAttr, AccidentAttrTable, AssignTarget, Assignment, EdgeAccidentIndex, UnassignedReason,
    UnassignedRecord,
};
use accident_route_backend::domain::routing::graph::synthetic::{EdgeSpec, GraphBuilder};
use accident_route_backend::domain::routing::graph::time::{PerMode, TimeModel};
use accident_route_backend::domain::routing::graph::{
    Coord, DirEdge, EdgeRecord, Graph, GraphParts,
};
use accident_route_backend::domain::routing::prep::attrs::GraphAttrs;
use accident_route_backend::domain::routing::prep::risk_fit::{
    EXPOSURE_CATEGORIES, ExposureClusters, ExposureConfig, ExposureMethod, RiskFitConfig,
    RiskFitInput, RiskFitOutput, fit_risk,
};
use accident_route_backend::domain::routing::risk::{
    CELL_COUNT, CarExposure, CarExposureCategory, CoefRecord, RiskModel, RiskParams, RiskVariant,
    RiskView, TABLE_LEN, WeatherMultiplier, road_class_k, table_index,
};
use accident_route_backend::domain::routing::topology::{
    ClusterRecord, SidewalkParent, Topology, TwinPair,
};
use accident_route_backend::domain::snapshot::{AccidentColumns, AccidentSnapshot};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------------------------
// 合成入力（tests/routing_risk_fit.rs と同じ形）
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct EdgeSpecInput {
    tail: u32,
    head: u32,
    length_dm: u32,
    class: u8,
    flags: u8,
}

impl EdgeSpecInput {
    fn road(class: u8, length_dm: u32) -> Self {
        Self {
            tail: 0,
            head: 1,
            length_dm,
            class,
            flags: 0,
        }
    }
}

#[derive(Clone)]
struct AccidentSpec {
    lat: f64,
    lon: f64,
    modes: u8,
    period: u8,
    weather: u8,
    day_type: u8,
    deaths: u16,
    injuries: u16,
    year: u16,
    /// (kind, target, pos_dm, weight, mirror)
    targets: Vec<(u8, u32, u32, f32, bool)>,
    terrain: u8,
    route_class: u8,
}

impl AccidentSpec {
    fn car_edge(year: u16, target: u32, weight: f32) -> Self {
        Self {
            lat: 35.0,
            lon: 135.0,
            modes: Mode::Car.mask(),
            period: 0,
            weather: 1,
            day_type: 0,
            deaths: 0,
            injuries: 1,
            year,
            targets: vec![(0, target, 0, weight, false)],
            terrain: 2,
            route_class: 0xFF,
        }
    }

    fn car_cluster(year: u16, target: u32, weight: f32) -> Self {
        Self {
            targets: vec![(1, target, u32::MAX, weight, false)],
            ..Self::car_edge(year, target, weight)
        }
    }
}

struct Fixture {
    parts: GraphParts,
    attrs: GraphAttrs,
    topology: Topology,
    accident_attrs: AccidentAttrTable,
    assignment: Assignment,
    columns: AccidentColumns,
    coords: Vec<Coord>,
    edges: Vec<EdgeSpecInput>,
}

#[allow(clippy::too_many_arguments)]
fn fixture(
    edges: Vec<EdgeSpecInput>,
    coords: Vec<Coord>,
    lanes: Vec<u8>,
    widths: Vec<u16>,
    twins: Vec<(u32, u32)>,
    sidewalk: Vec<(u32, u32)>,
    clusters: Vec<ClusterRecord>,
    accidents: Vec<AccidentSpec>,
) -> Fixture {
    let edge_count = edges.len();
    let node_count = coords.len();
    let mut offsets = Vec::with_capacity(accidents.len() + 1);
    offsets.push(0_u32);
    let mut targets = Vec::new();
    let mut unassigned = Vec::new();
    let mut columns = AccidentColumns::default();
    let mut attrs = Vec::with_capacity(accidents.len());
    for (i, a) in accidents.iter().enumerate() {
        columns.lat.push(a.lat);
        columns.lon.push(a.lon);
        columns.modes.push(a.modes);
        columns.period.push(a.period);
        columns.weather.push(a.weather);
        columns.day_type.push(a.day_type);
        columns.deaths.push(a.deaths);
        columns.injuries.push(a.injuries);
        columns.source_year.push(a.year);
        columns.date.push(u32::from(a.year) * 10_000 + 101);
        let mut attr = AccidentAttr::MISSING;
        attr.terrain = a.terrain;
        attr.route_class = a.route_class;
        attrs.push(attr);
        let mut accident_targets = a.targets.clone();
        accident_targets.sort_by_key(|&(kind, target, pos_dm, _, _)| (kind, target, pos_dm));
        for (kind, target, pos_dm, weight, mirror) in accident_targets {
            targets.push(AssignTarget {
                target,
                pos_dm,
                weight,
                kind,
                flags: u8::from(mirror),
                reserved: 0,
            });
        }
        if a.targets.is_empty() {
            unassigned.push(UnassignedRecord {
                accident: i as u32,
                reason: UnassignedReason::Other,
            });
        }
        offsets.push(targets.len() as u32);
    }
    let assignment = Assignment {
        params: Value::Null,
        offsets,
        targets,
        unassigned,
    };
    let topology = Topology {
        params: Value::Null,
        snap_car: Vec::new(),
        snap_bike: Vec::new(),
        snap_walk: Vec::new(),
        node_cluster: vec![u32::MAX; node_count],
        clusters,
        cluster_nodes: Vec::new(),
        twins: twins.into_iter().map(|(a, b)| TwinPair { a, b }).collect(),
        sidewalk_parents: sidewalk
            .into_iter()
            .map(|(sidewalk, parent)| SidewalkParent { sidewalk, parent })
            .collect(),
    };
    let attrs_table = AccidentAttrTable {
        params: Value::Null,
        attrs,
    };
    let graph_attrs = GraphAttrs {
        params: Value::Null,
        way_id: Vec::new(),
        node_id: Vec::new(),
        ref_idx: Vec::new(),
        name_idx: Vec::new(),
        strtab: vec![0],
        lanes,
        width_dm: widths,
        layer: vec![0; edge_count],
    };
    let parts = GraphParts {
        params: Value::Null,
        time_model: TimeModel::synthetic(),
        node_ofs: Vec::new(),
        adj: Vec::new(),
        node_ll: coords.clone(),
        node_flags: vec![0; node_count],
        edges: edges
            .iter()
            .map(|e| EdgeRecord {
                tail: e.tail,
                head: e.head,
                length_dm: e.length_dm,
                class: e.class,
                access: 0x3f,
                flags: e.flags,
                maxspeed_kmh: 0,
            })
            .collect(),
        shape_ofs: vec![0; edge_count + 1],
        shape: Vec::new(),
    };
    Fixture {
        parts,
        attrs: graph_attrs,
        topology,
        accident_attrs: attrs_table,
        assignment,
        columns,
        coords,
        edges,
    }
}

fn coords_for(edges: &[EdgeSpecInput]) -> Vec<Coord> {
    let nodes = edges
        .iter()
        .map(|e| e.tail.max(e.head) as usize + 1)
        .max()
        .unwrap_or(0);
    (0..nodes)
        .map(|i| Coord::new(350_000_000 + i as i32 * 1000, 1_350_000_000))
        .collect()
}

fn run_output(
    fixture: &Fixture,
    train: &[u16],
    validate: Option<u16>,
    exposure: Option<ExposureConfig>,
) -> RiskFitOutput {
    let input = RiskFitInput {
        graph: &fixture.parts,
        graph_attrs: &fixture.attrs,
        topology: &fixture.topology,
        accident_attrs: &fixture.accident_attrs,
        assignment: &fixture.assignment,
        accidents: &fixture.columns,
    };
    let config = RiskFitConfig {
        train_years: train.to_vec(),
        validate_year: validate,
        c: 1.0,
        exposure,
        inherit: None,
    };
    fit_risk(&input, &config, &|_| {}).unwrap()
}

fn route_code(rates: [f64; 3], lengths: [f64; 3], clusters: ExposureClusters) -> ExposureConfig {
    ExposureConfig {
        method: ExposureMethod::RouteCode {
            lengths_km: lengths,
        },
        rates: Some(rates),
        clusters,
        route_code_lengths_km: Some(lengths),
    }
}

fn osm(rates: [f64; 3], clusters: ExposureClusters) -> ExposureConfig {
    ExposureConfig {
        method: ExposureMethod::Osm,
        rates: Some(rates),
        clusters,
        route_code_lengths_km: None,
    }
}

fn factors(values: [f64; 3], clusters: ExposureClusters) -> ExposureConfig {
    ExposureConfig {
        method: ExposureMethod::Factors(values),
        rates: None,
        clusters,
        route_code_lengths_km: None,
    }
}

fn category(model: &RiskModel, index: usize) -> &CarExposureCategory {
    &model
        .params
        .car_exposure
        .as_ref()
        .expect("car_exposureがある")
        .categories[index]
}

fn value(entry: &Value, key: &str) -> f64 {
    entry[key]
        .as_f64()
        .unwrap_or_else(|| panic!("{key}が数値ではありません: {entry}"))
}

fn assert_relative(actual: f64, expected: f64, what: &str) {
    let scale = actual.abs().max(expected.abs()).max(1.0);
    assert!(
        (actual - expected).abs() <= 1e-9 * scale,
        "{what}: {actual} != {expected}（相対誤差）"
    );
}

fn valid_graph_parts(fixture: &Fixture) -> GraphParts {
    let edge_count = fixture.edges.len();
    let node_count = fixture.coords.len();
    let mut node_ofs = Vec::with_capacity(node_count + 1);
    let mut adj = Vec::new();
    node_ofs.push(0_u32);
    for node in 0..node_count {
        let mut out = Vec::new();
        for (e, edge) in fixture.edges.iter().enumerate() {
            if edge.tail as usize == node {
                out.push(DirEdge::new(e as u32, false));
            }
            if edge.head as usize == node {
                out.push(DirEdge::new(e as u32, true));
            }
        }
        out.sort_unstable();
        adj.extend(out);
        node_ofs.push(adj.len() as u32);
    }
    GraphParts {
        params: Value::Null,
        time_model: TimeModel::synthetic(),
        node_ofs,
        adj,
        node_ll: fixture.coords.clone(),
        node_flags: vec![0; node_count],
        edges: fixture.parts.edges.clone(),
        shape_ofs: vec![0; edge_count + 1],
        shape: Vec::new(),
    }
}

// ---------------------------------------------------------------------------------------------
// 係数の計算
// ---------------------------------------------------------------------------------------------

#[test]
fn route_code_categories_match_hand_calculation() {
    let edges = vec![
        EdgeSpecInput::road(0, 1000),
        EdgeSpecInput::road(2, 1000),
        EdgeSpecInput::road(11, 1000),
    ];
    let coords = coords_for(&edges);
    let mut accidents = Vec::new();
    for _ in 0..2 {
        let mut a = AccidentSpec::car_edge(2020, 0, 1.0);
        a.targets.clear();
        a.route_class = 5;
        accidents.push(a);
    }
    for _ in 0..3 {
        let mut a = AccidentSpec::car_edge(2020, 1, 1.0);
        a.targets.clear();
        a.route_class = 0;
        accidents.push(a);
    }
    for _ in 0..5 {
        let mut a = AccidentSpec::car_edge(2020, 2, 1.0);
        a.targets.clear();
        a.route_class = 4;
        accidents.push(a);
    }
    let fixture = fixture(
        edges,
        coords,
        vec![2, 2, 2],
        vec![0, 0, 0],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        accidents,
    );
    let rates = [9.0, 62.0, 142.0];
    let lengths = [10.0, 20.0, 30.0];
    let output = run_output(
        &fixture,
        &[2020],
        Some(2021),
        Some(route_code(rates, lengths, ExposureClusters::TopGroup)),
    );
    let model = output.model;

    let counts = [2.0, 3.0, 5.0];
    let density = [0.2, 0.15, 5.0 / 30.0];
    let traffic = [
        density[0] / rates[0],
        density[1] / rates[1],
        density[2] / rates[2],
    ];
    let expected = [traffic[1] / traffic[0], 1.0, traffic[1] / traffic[2]];
    for cat in 0..3 {
        let entry = category(&model, cat);
        assert_eq!(entry.name, EXPOSURE_CATEGORIES[cat]);
        assert_relative(
            value(&output.audit["exposure"]["categories"][cat], "count"),
            counts[cat],
            "count",
        );
        assert_relative(
            value(&output.audit["exposure"]["categories"][cat], "length_km"),
            lengths[cat],
            "length_km",
        );
        assert_relative(
            value(&output.audit["exposure"]["categories"][cat], "density"),
            density[cat],
            "density",
        );
        assert_relative(
            value(&output.audit["exposure"]["categories"][cat], "t"),
            traffic[cat],
            "t",
        );
        assert_relative(entry.factor, expected[cat], "factor");
    }
    let exposure = &output.audit["exposure"];
    assert_eq!(exposure["method"], "route-code");
    assert_eq!(exposure["clusters"], "top_group");
    // route-code のもう一方（osm）は常に計算できる（事故はすべて割り当てなしなので0）。
    assert_eq!(exposure["alternate"]["method"], "osm");
}

#[test]
fn route_code_counts_unassigned_train_car_accidents_and_skips_other_classes() {
    let edges = vec![
        EdgeSpecInput::road(0, 1000),
        EdgeSpecInput::road(2, 1000),
        EdgeSpecInput::road(11, 1000),
    ];
    let coords = coords_for(&edges);
    let make = |year: u16, route_class: u8, modes: u8| {
        let mut a = AccidentSpec::car_edge(year, 0, 1.0);
        a.targets.clear();
        a.route_class = route_class;
        a.modes = modes;
        a
    };
    let accidents = vec![
        make(2020, 5, Mode::Car.mask()),    // 自専道・学習年・自動車
        make(2020, 0, Mode::Car.mask()),    // 幹線
        make(2020, 4, Mode::Car.mask()),    // 生活
        make(2020, 8, Mode::Car.mask()),    // その他（数えない）
        make(2020, 14, Mode::Car.mask()),   // その他（数えない）
        make(2020, 0xFF, Mode::Car.mask()), // 欠損（数えない）
        make(2020, 5, Mode::Walk.mask()),   // 自動車が関与しない（数えない）
        make(2021, 5, Mode::Car.mask()),    // 学習年以外（数えない）
    ];
    let fixture = fixture(
        edges,
        coords,
        vec![2, 2, 2],
        vec![0, 0, 0],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        accidents,
    );
    let output = run_output(
        &fixture,
        &[2020],
        Some(2021),
        Some(route_code(
            [9.0, 62.0, 142.0],
            [10.0, 20.0, 30.0],
            ExposureClusters::None,
        )),
    );
    for (cat, expected) in [1.0, 1.0, 1.0].into_iter().enumerate() {
        assert_relative(
            value(&output.audit["exposure"]["categories"][cat], "count"),
            expected,
            "count",
        );
    }
}

#[test]
fn osm_uses_assignment_weights_without_mirror_and_halves_twin_lengths() {
    let edges = vec![
        EdgeSpecInput::road(0, 100_000),
        EdgeSpecInput::road(2, 200_000),
        EdgeSpecInput::road(11, 300_000),
        EdgeSpecInput::road(11, 300_000),
    ];
    let coords = coords_for(&edges);
    let twins = vec![(2, 3), (3, 2)];
    let mut accidents = Vec::new();
    for _ in 0..3 {
        accidents.push(AccidentSpec::car_edge(2020, 0, 1.0));
    }
    for _ in 0..4 {
        accidents.push(AccidentSpec::car_edge(2020, 1, 0.5));
    }
    for _ in 0..5 {
        let mut a = AccidentSpec::car_edge(2020, 2, 1.0);
        // 双子の辺への写しは数えない。
        a.targets.push((0, 3, 0, 1.0, true));
        accidents.push(a);
    }
    let fixture = fixture(
        edges,
        coords,
        vec![2, 2, 2, 2],
        vec![0, 0, 0, 0],
        twins,
        Vec::new(),
        Vec::new(),
        accidents,
    );
    let rates = [9.0, 62.0, 142.0];
    let output = run_output(
        &fixture,
        &[2020],
        Some(2021),
        Some(osm(rates, ExposureClusters::TopGroup)),
    );
    let model = output.model;
    let counts = [3.0, 2.0, 5.0];
    let lengths = [10.0, 20.0, 30.0];
    let density = [
        counts[0] / lengths[0],
        counts[1] / lengths[1],
        counts[2] / lengths[2],
    ];
    let traffic = [
        density[0] / rates[0],
        density[1] / rates[1],
        density[2] / rates[2],
    ];
    for cat in 0..3 {
        assert_relative(
            value(&output.audit["exposure"]["categories"][cat], "count"),
            counts[cat],
            "count",
        );
        assert_relative(
            value(&output.audit["exposure"]["categories"][cat], "length_km"),
            lengths[cat],
            "length_km",
        );
        assert_relative(
            value(&output.audit["exposure"]["categories"][cat], "t"),
            traffic[cat],
            "t",
        );
    }
    assert_relative(
        category(&model, 0).factor,
        traffic[1] / traffic[0],
        "motorway",
    );
    assert_relative(category(&model, 2).factor, traffic[1] / traffic[2], "local");
}

#[test]
fn factors_are_used_as_given_and_have_null_calibration() {
    let edges = vec![EdgeSpecInput::road(11, 1000)];
    let coords = coords_for(&edges);
    let fixture = fixture(
        edges,
        coords,
        vec![2],
        vec![0],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![AccidentSpec::car_edge(2020, 0, 1.0)],
    );
    let values = [0.27, 1.5, 13.5];
    let output = run_output(
        &fixture,
        &[2020],
        Some(2021),
        Some(factors(values, ExposureClusters::None)),
    );
    let model = output.model;
    for (cat, &expected) in values.iter().enumerate() {
        let entry = &output.audit["exposure"]["categories"][cat];
        assert_eq!(entry["rate"], Value::Null);
        assert_eq!(entry["length_km"], Value::Null);
        assert_eq!(entry["count"], Value::Null);
        assert_eq!(entry["density"], Value::Null);
        assert_eq!(entry["t"], Value::Null);
        assert_relative(category(&model, cat).factor, expected, "factor");
    }
}

#[test]
fn default_fit_does_not_write_car_exposure() {
    let edges = vec![EdgeSpecInput::road(11, 1000)];
    let coords = coords_for(&edges);
    let fixture = fixture(
        edges,
        coords,
        vec![2],
        vec![0],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![AccidentSpec::car_edge(2020, 0, 1.0)],
    );
    let output = run_output(&fixture, &[2020], Some(2021), None);
    assert!(output.model.params.car_exposure.is_none());
    assert!(output.model.raw_params.get("car_exposure").is_none());
    assert!(output.audit.get("exposure").is_none());
}

// ---------------------------------------------------------------------------------------------
// 読込
// ---------------------------------------------------------------------------------------------

fn params_value() -> Value {
    let params = RiskParams {
        train_source_years: vec![2020],
        years: 1,
        c: 1.0,
        c_groups: vec![5, 6, 7, 8],
        car_exposure: None,
        beta: PerMode {
            walk: 0.2,
            bike: 0.3,
            car: 0.4,
        },
        beta_cluster: PerMode {
            walk: 0.2,
            bike: 0.3,
            car: 0.4,
        },
        weather_multiplier: WeatherMultiplier {
            rain: 2.0,
            fog: 4.0,
            snow: 4.0,
        },
        urban: json!({}),
        pooling: json!({}),
        fit: json!({}),
    };
    serde_json::to_value(params).unwrap()
}

fn exposure_json(categories: Value) -> Value {
    json!({
        "method": "factors",
        "clusters": "top_group",
        "reference": "arterial",
        "categories": categories,
    })
}

#[test]
fn car_exposure_is_checked_on_load() {
    let mut negative = params_value();
    negative["car_exposure"] = exposure_json(json!([
        {"name": "motorway", "groups": [0], "factor": -0.5},
        {"name": "arterial", "groups": [1, 2, 3], "factor": 1.0},
        {"name": "local", "groups": [4], "factor": 2.0}
    ]));
    assert!(serde_json::from_value::<RiskParams>(negative).is_err());

    let mut duplicate = params_value();
    duplicate["car_exposure"] = exposure_json(json!([
        {"name": "motorway", "groups": [0], "factor": 0.5},
        {"name": "arterial", "groups": [0, 1], "factor": 1.0},
        {"name": "local", "groups": [4], "factor": 2.0}
    ]));
    assert!(serde_json::from_value::<RiskParams>(duplicate).is_err());

    let mut groups_out_of_range = params_value();
    groups_out_of_range["car_exposure"] = exposure_json(json!([
        {"name": "motorway", "groups": [16], "factor": 0.5}
    ]));
    assert!(serde_json::from_value::<RiskParams>(groups_out_of_range).is_err());

    let mut clusters = params_value();
    clusters["car_exposure"] = json!({
        "method": "factors",
        "clusters": "unknown",
        "categories": [
            {"name": "motorway", "groups": [0], "factor": 0.5}
        ],
    });
    assert!(serde_json::from_value::<RiskParams>(clusters).is_err());

    let old = params_value();
    assert!(
        serde_json::from_value::<RiskParams>(old)
            .unwrap()
            .car_exposure
            .is_none()
    );

    let mut valid = params_value();
    valid["car_exposure"] = exposure_json(json!([
        {"name": "motorway", "groups": [0], "factor": 0.5, "rate": 9.0},
        {"name": "arterial", "groups": [1, 2, 3], "factor": 1.0},
        {"name": "local", "groups": [4, 5, 6, 7, 8, 9, 10], "factor": 2.0}
    ]));
    let params: RiskParams = serde_json::from_value(valid).unwrap();
    assert_eq!(params.car_exposure.as_ref().unwrap().clusters, "top_group");
}

#[test]
fn car_exposure_extra_keys_survive_encode_and_decode() {
    let mut model = empty_model(vec![road_class_k(6, 0, false)], Vec::new(), None);
    let mut extra = serde_json::Map::new();
    extra.insert("reference".to_owned(), json!("arterial"));
    let mut category_extra = serde_json::Map::new();
    category_extra.insert("route_classes".to_owned(), json!([5, 6, 7]));
    category_extra.insert("rate".to_owned(), json!(9.0));
    model.params.car_exposure = Some(CarExposure {
        method: "route-code".to_owned(),
        clusters: "top_group".to_owned(),
        categories: vec![
            CarExposureCategory {
                name: "motorway".to_owned(),
                groups: vec![0],
                factor: 0.27,
                extra: category_extra,
            },
            CarExposureCategory {
                name: "arterial".to_owned(),
                groups: vec![1, 2, 3],
                factor: 1.0,
                extra: serde_json::Map::new(),
            },
        ],
        extra,
    });
    let deps: Deps = required_deps("risk.bin")
        .iter()
        .map(|key| ((*key).to_owned(), "0".repeat(64)))
        .collect();
    let bytes = encode_risk(&model, &deps);
    let decoded = decode_risk(&bytes).unwrap().value;
    let exposure = &decoded.raw_params["car_exposure"];
    assert_eq!(exposure["reference"], "arterial");
    assert_eq!(exposure["categories"][0]["route_classes"], json!([5, 6, 7]));
    assert_eq!(exposure["categories"][0]["rate"], 9.0);
}

// ---------------------------------------------------------------------------------------------
// エンジン
// ---------------------------------------------------------------------------------------------

fn base_params() -> RiskParams {
    RiskParams {
        train_source_years: vec![2020],
        years: 1,
        c: 1.0,
        c_groups: vec![5, 6, 7, 8],
        car_exposure: None,
        beta: PerMode {
            walk: 0.2,
            bike: 0.3,
            car: 0.4,
        },
        beta_cluster: PerMode {
            walk: 0.2,
            bike: 0.3,
            car: 0.4,
        },
        weather_multiplier: WeatherMultiplier {
            rain: 2.0,
            fog: 4.0,
            snow: 4.0,
        },
        urban: json!({}),
        pooling: json!({}),
        fit: json!({}),
    }
}

fn exposure_for(method: &str, clusters: &str, factors: [f64; 3]) -> CarExposure {
    let groups: [&[u8]; 3] = [&[0], &[1, 2, 3], &[4, 5, 6, 7, 8, 9, 10]];
    CarExposure {
        method: method.to_owned(),
        clusters: clusters.to_owned(),
        categories: (0..3)
            .map(|cat| CarExposureCategory {
                name: EXPOSURE_CATEGORIES[cat].to_owned(),
                groups: groups[cat].to_vec(),
                factor: factors[cat],
                extra: serde_json::Map::new(),
            })
            .collect(),
        extra: serde_json::Map::new(),
    }
}

fn empty_model(
    edge_k: Vec<u8>,
    cluster_j: Vec<u8>,
    car_exposure: Option<CarExposure>,
) -> RiskModel {
    let mut params = base_params();
    params.car_exposure = car_exposure;
    RiskModel {
        params,
        raw_params: json!({}),
        edge_k,
        cluster_j,
        k_table: vec![CoefRecord::default(); TABLE_LEN],
        j_table: vec![CoefRecord::default(); TABLE_LEN],
    }
}

fn fill_k(model: &mut RiskModel, mode: Mode, k: u8, f: impl Fn(usize) -> CoefRecord) {
    for cell in 0..CELL_COUNT {
        model.k_table[table_index(mode, k, cell)] = f(cell);
    }
}

fn fill_j(model: &mut RiskModel, mode: Mode, j: u8, f: impl Fn(usize) -> CoefRecord) {
    for cell in 0..CELL_COUNT {
        model.j_table[table_index(mode, j, cell)] = f(cell);
    }
}

fn simple_graph(edges: usize) -> Graph {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.0, 139.0);
    let b = builder.add_node_deg(35.0, 139.001);
    for _ in 0..edges {
        builder.add_edge(EdgeSpec::new(a, b).length_dm(1000));
    }
    builder.build()
}

fn simple_topology(_edge_count: usize, clusters: usize, sidewalk: Vec<SidewalkParent>) -> Topology {
    Topology {
        params: json!({}),
        snap_car: Vec::new(),
        snap_bike: Vec::new(),
        snap_walk: Vec::new(),
        node_cluster: Vec::new(),
        clusters: (0..clusters)
            .map(|_| ClusterRecord {
                lat_e7: 0,
                lon_e7: 0,
                node_start: 0,
                node_count: 0,
                top_class: 11,
                branches: 3,
                flags: 0,
                pad: [0; 3],
            })
            .collect(),
        cluster_nodes: Vec::new(),
        twins: Vec::new(),
        sidewalk_parents: sidewalk,
    }
}

fn empty_index(edge_count: usize, cluster_count: usize) -> EdgeAccidentIndex {
    EdgeAccidentIndex {
        edge_ofs: vec![0; edge_count + 1],
        edge_entries: Vec::new(),
        cluster_ofs: vec![0; cluster_count + 1],
        cluster_entries: Vec::new(),
    }
}

fn car_coef(cell: usize) -> CoefRecord {
    CoefRecord {
        r: 1e-5 * (1.0 + cell as f64 * 0.1),
        s: 10.0 + cell as f64,
        a: 0.2,
        rho: [0.2, 0.2, 0.2, 0.2, 0.2],
        pad: 0.0,
    }
}

fn cluster_coef(cell: usize) -> CoefRecord {
    CoefRecord {
        r: 0.01 * (1.0 + cell as f64 * 0.1),
        s: 10.0 + cell as f64,
        a: 0.3,
        rho: [0.2, 0.2, 0.2, 0.2, 0.2],
        pad: 0.0,
    }
}

#[allow(clippy::too_many_arguments)]
fn edge_risk(
    model: &RiskModel,
    graph: &Graph,
    topology: &Topology,
    index: &EdgeAccidentIndex,
    snapshot: &AccidentSnapshot,
    mode: Mode,
    variant: RiskVariant,
    edge: u32,
) -> f64 {
    let mut view = RiskView::new(
        model,
        graph,
        topology,
        index,
        snapshot,
        mode,
        ConditionFilter::default(),
    )
    .with_variant(variant);
    view.edge_risk(edge)
}

#[allow(clippy::too_many_arguments)]
fn cluster_risk(
    model: &RiskModel,
    graph: &Graph,
    topology: &Topology,
    index: &EdgeAccidentIndex,
    snapshot: &AccidentSnapshot,
    mode: Mode,
    variant: RiskVariant,
    cluster: u32,
) -> f64 {
    let mut view = RiskView::new(
        model,
        graph,
        topology,
        index,
        snapshot,
        mode,
        ConditionFilter::default(),
    )
    .with_variant(variant);
    view.cluster_risk(cluster)
}

#[test]
fn engine_scales_car_edges_by_group_and_keeps_other_modes() {
    let kinds = [
        road_class_k(0, 0, false),
        road_class_k(2, 1, false),
        road_class_k(6, 2, true),
        road_class_k(11, 0, false),
    ];
    let factors = [0.27, 1.0, 13.5];
    let graph = simple_graph(4);
    let topology = simple_topology(4, 0, Vec::new());
    let index = empty_index(4, 0);
    let snapshot = AccidentSnapshot::new(AccidentColumns::default());
    let mut plain = empty_model(kinds.to_vec(), Vec::new(), None);
    let mut exposed = empty_model(
        kinds.to_vec(),
        Vec::new(),
        Some(exposure_for("factors", "top_group", factors)),
    );
    for mode in Mode::ALL {
        for &k in &kinds {
            fill_k(&mut plain, mode, k, car_coef);
            fill_k(&mut exposed, mode, k, car_coef);
        }
    }
    let expected = [factors[0], factors[1], factors[2], 1.0];
    for (e, &ratio) in expected.iter().enumerate() {
        let plain_risk = edge_risk(
            &plain,
            &graph,
            &topology,
            &index,
            &snapshot,
            Mode::Car,
            RiskVariant::Standard,
            e as u32,
        );
        let exposed_risk = edge_risk(
            &exposed,
            &graph,
            &topology,
            &index,
            &snapshot,
            Mode::Car,
            RiskVariant::Standard,
            e as u32,
        );
        assert_relative(exposed_risk, plain_risk * ratio, "自動車の辺の係数");
        for mode in [Mode::Walk, Mode::Bike] {
            let plain_risk = edge_risk(
                &plain,
                &graph,
                &topology,
                &index,
                &snapshot,
                mode,
                RiskVariant::Standard,
                e as u32,
            );
            let exposed_risk = edge_risk(
                &exposed,
                &graph,
                &topology,
                &index,
                &snapshot,
                mode,
                RiskVariant::Standard,
                e as u32,
            );
            assert_relative(exposed_risk, plain_risk, "徒歩・自転車の辺は変えない");
        }
        // 期待件数には係数を掛けない。
        let plain_risk = edge_risk(
            &plain,
            &graph,
            &topology,
            &index,
            &snapshot,
            Mode::Car,
            RiskVariant::ExpectedCount,
            e as u32,
        );
        let exposed_risk = edge_risk(
            &exposed,
            &graph,
            &topology,
            &index,
            &snapshot,
            Mode::Car,
            RiskVariant::ExpectedCount,
            e as u32,
        );
        assert_relative(exposed_risk, plain_risk, "期待件数は変えない");
    }
}

#[test]
fn engine_scales_clusters_for_top_group_only() {
    let cluster_j = vec![1_u8, 17, 49];
    let factors = [0.27, 1.0, 13.5];
    let graph = simple_graph(1);
    let topology = simple_topology(1, 3, Vec::new());
    let index = empty_index(1, 3);
    let snapshot = AccidentSnapshot::new(AccidentColumns::default());
    let mut plain = empty_model(vec![road_class_k(6, 0, false)], cluster_j.clone(), None);
    let mut top_group = empty_model(
        vec![road_class_k(6, 0, false)],
        cluster_j.clone(),
        Some(exposure_for("factors", "top_group", factors)),
    );
    let mut no_clusters = empty_model(
        vec![road_class_k(6, 0, false)],
        cluster_j,
        Some(exposure_for("factors", "none", factors)),
    );
    for mode in Mode::ALL {
        for &j in &[1_u8, 17, 49] {
            fill_j(&mut plain, mode, j, cluster_coef);
            fill_j(&mut top_group, mode, j, cluster_coef);
            fill_j(&mut no_clusters, mode, j, cluster_coef);
        }
    }
    for (c, &ratio) in [factors[0], factors[1], factors[2]].iter().enumerate() {
        let plain_risk = cluster_risk(
            &plain,
            &graph,
            &topology,
            &index,
            &snapshot,
            Mode::Car,
            RiskVariant::Standard,
            c as u32,
        );
        let exposed_risk = cluster_risk(
            &top_group,
            &graph,
            &topology,
            &index,
            &snapshot,
            Mode::Car,
            RiskVariant::Standard,
            c as u32,
        );
        assert_relative(exposed_risk, plain_risk * ratio, "top_groupの交差点");
        let none_risk = cluster_risk(
            &no_clusters,
            &graph,
            &topology,
            &index,
            &snapshot,
            Mode::Car,
            RiskVariant::Standard,
            c as u32,
        );
        assert_relative(none_risk, plain_risk, "clusters=noneの交差点");
        for mode in [Mode::Walk, Mode::Bike] {
            let risk = cluster_risk(
                &top_group,
                &graph,
                &topology,
                &index,
                &snapshot,
                mode,
                RiskVariant::Standard,
                c as u32,
            );
            let plain_risk = cluster_risk(
                &plain,
                &graph,
                &topology,
                &index,
                &snapshot,
                mode,
                RiskVariant::Standard,
                c as u32,
            );
            assert_relative(risk, plain_risk, "徒歩・自転車の交差点は変えない");
        }
        let expected = cluster_risk(
            &top_group,
            &graph,
            &topology,
            &index,
            &snapshot,
            Mode::Car,
            RiskVariant::ExpectedCount,
            c as u32,
        );
        let plain_expected = cluster_risk(
            &plain,
            &graph,
            &topology,
            &index,
            &snapshot,
            Mode::Car,
            RiskVariant::ExpectedCount,
            c as u32,
        );
        assert_relative(expected, plain_expected, "期待件数の交差点は変えない");
    }
}

#[test]
fn partial_risk_is_scaled_and_additive_and_sidewalk_is_unchanged() {
    let local = road_class_k(6, 1, true);
    let factors = [0.27, 1.0, 13.5];
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.0, 139.0);
    let b = builder.add_node_deg(35.0, 139.001);
    let car_edge = builder.add_edge(EdgeSpec::new(a, b).length_dm(1000));
    let sidewalk = builder.add_edge(EdgeSpec::new(a, b).length_dm(300).flags(0x02));
    let graph = builder.build();
    let topology = simple_topology(
        2,
        0,
        vec![SidewalkParent {
            sidewalk,
            parent: car_edge,
        }],
    );
    let index = empty_index(2, 0);
    let snapshot = AccidentSnapshot::new(AccidentColumns::default());
    let mut plain = empty_model(vec![local, 0xFE], Vec::new(), None);
    let mut exposed = empty_model(
        vec![local, 0xFE],
        Vec::new(),
        Some(exposure_for("factors", "top_group", factors)),
    );
    for mode in Mode::ALL {
        fill_k(&mut plain, mode, local, car_coef);
        fill_k(&mut exposed, mode, local, car_coef);
    }

    let full_plain = edge_risk(
        &plain,
        &graph,
        &topology,
        &index,
        &snapshot,
        Mode::Car,
        RiskVariant::Standard,
        car_edge,
    );
    let mut view = RiskView::new(
        &exposed,
        &graph,
        &topology,
        &index,
        &snapshot,
        Mode::Car,
        ConditionFilter::default(),
    );
    let full = view.partial_edge_risk(car_edge, 0, 1000);
    assert_relative(full, full_plain * factors[2], "部分辺の係数");
    let first = view.partial_edge_risk(car_edge, 0, 400);
    let second = view.partial_edge_risk(car_edge, 400, 1000);
    assert_relative(first + second, full, "辺の分割の不変性");

    // 歩道のリスクは徒歩・自転車用で、車の係数の影響を受けない。
    for mode in [Mode::Walk, Mode::Bike] {
        let plain_risk = edge_risk(
            &plain,
            &graph,
            &topology,
            &index,
            &snapshot,
            mode,
            RiskVariant::Standard,
            sidewalk,
        );
        let exposed_risk = edge_risk(
            &exposed,
            &graph,
            &topology,
            &index,
            &snapshot,
            mode,
            RiskVariant::Standard,
            sidewalk,
        );
        assert_relative(exposed_risk, plain_risk, "歩道は変えない");
    }
}

// ---------------------------------------------------------------------------------------------
// エンジンを通した集計
// ---------------------------------------------------------------------------------------------

fn cluster_record(top_class: u8) -> ClusterRecord {
    ClusterRecord {
        lat_e7: 0,
        lon_e7: 0,
        node_start: 0,
        node_count: 0,
        top_class,
        branches: 3,
        flags: 0,
        pad: [0; 3],
    }
}

#[test]
fn engine_aggregate_reproduces_the_rates_used_by_the_fit() {
    // 区分ごとに1本の辺（1km）と1つの交差点クラスタを作り、学習年・検証年に同じ数の
    // 事故を割り当てる。Wが1に近くなるので、経験ベイズの事前分布の影響は小さい。
    // 右辺の Σrisk はエンジンが返す辺とクラスタのリスクの和。係数 f と監査の T_cat を
    // 掛けた分母で割ると、各区分で S·r_cat（S は平均深刻度、r_cat は与えた事故率）に
    // なるので、区分間の比は事故率の比になる。
    const N: usize = 300;
    let edges = vec![
        EdgeSpecInput::road(0, 10_000),
        EdgeSpecInput::road(2, 10_000),
        EdgeSpecInput::road(11, 10_000),
    ];
    let coords = coords_for(&edges);
    let clusters = vec![cluster_record(0), cluster_record(2), cluster_record(11)];
    let mut accidents = Vec::new();
    for year in [2020_u16, 2021] {
        for target in 0..3u32 {
            for _ in 0..N {
                accidents.push(AccidentSpec::car_edge(year, target, 1.0));
                accidents.push(AccidentSpec::car_cluster(year, target, 1.0));
            }
        }
    }
    let fixture = fixture(
        edges,
        coords,
        vec![2, 2, 2],
        vec![0, 0, 0],
        Vec::new(),
        Vec::new(),
        clusters,
        accidents,
    );
    let rates = [9.0, 62.0, 142.0];
    let output = run_output(
        &fixture,
        &[2020],
        Some(2021),
        Some(osm(rates, ExposureClusters::TopGroup)),
    );
    let model = output.model;

    let parts = valid_graph_parts(&fixture);
    let graph = Graph::from_parts(parts).unwrap();
    let index = EdgeAccidentIndex::build(
        &fixture.assignment,
        graph.edge_count(),
        model.cluster_j.len(),
    );
    let snapshot = AccidentSnapshot::new(fixture.columns.clone());
    let mut view = RiskView::new(
        &model,
        &graph,
        &fixture.topology,
        &index,
        &snapshot,
        Mode::Car,
        ConditionFilter::default(),
    );
    let mut total = [0.0_f64; 3];
    for (e, &k) in model.edge_k.iter().enumerate() {
        let cat = match k / 8 {
            0 => 0,
            1..=3 => 1,
            4..=10 => 2,
            _ => continue,
        };
        total[cat] += view.edge_risk(e as u32);
    }
    for (c, &j) in model.cluster_j.iter().enumerate() {
        let cat = match j / 8 {
            0 => 0,
            1..=3 => 1,
            4..=10 => 2,
            _ => continue,
        };
        total[cat] += view.cluster_risk(c as u32);
    }

    let mut estimated = [0.0_f64; 3];
    for cat in 0..3 {
        let entry = &output.audit["exposure"]["categories"][cat];
        let length = value(entry, "length_km");
        let traffic = value(entry, "t");
        let factor = value(entry, "factor");
        estimated[cat] = total[cat] / (length * traffic * factor);
    }
    for cat in 0..3 {
        let expected = rates[cat] / rates[1];
        let actual = estimated[cat] / estimated[1];
        assert!(
            (actual - expected).abs() / expected <= 0.05,
            "区分{cat}の事故率の比: エンジンの集計{actual}、与えた値{expected}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// CLIの引数の検査（入力のファイルは読まずに、引数の解析だけで失敗する）
// ---------------------------------------------------------------------------------------------

fn risk_fit_with(extra: &[&str]) -> std::process::Output {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_routing_prep"));
    command.args([
        "risk-fit",
        "--graph",
        "x",
        "--attrs",
        "x",
        "--topology",
        "x",
        "--accident-attrs",
        "x",
        "--assignment",
        "x",
        "--accidents",
        "x",
        "--output",
        "x",
    ]);
    command.args(extra);
    command.output().expect("routing_prepを起動できる")
}

fn assert_cli_error(extra: &[&str], message: &str) {
    let output = risk_fit_with(extra);
    assert!(!output.status.success(), "失敗するはず: {extra:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(message),
        "{extra:?} のエラーに{message:?}がない:\n{stderr}"
    );
}

#[test]
fn cli_rejects_inconsistent_exposure_arguments() {
    assert_cli_error(
        &["--exposure-rates", "motorway=9,arterial=62,local=142"],
        "--exposure-method",
    );
    assert_cli_error(
        &[
            "--exposure-method",
            "route-code",
            "--exposure-rates",
            "motorway=9,arterial=62,local=142",
        ],
        "--exposure-lengths",
    );
    assert_cli_error(
        &[
            "--exposure-method",
            "route-code",
            "--exposure-lengths",
            "motorway=12675,arterial=181204,local=1015861",
        ],
        "--exposure-rates",
    );
    assert_cli_error(&["--exposure-method", "osm"], "--exposure-rates");
    assert_cli_error(&["--exposure-method", "factors"], "--exposure-factors");
    assert_cli_error(
        &[
            "--exposure-method",
            "route-code",
            "--exposure-rates",
            "motorway=9,arterial=62",
            "--exposure-lengths",
            "motorway=12675,arterial=181204,local=1015861",
        ],
        "motorway・arterial・local",
    );
    assert_cli_error(
        &[
            "--exposure-method",
            "route-code",
            "--exposure-rates",
            "motorway=-9,arterial=62,local=142",
            "--exposure-lengths",
            "motorway=12675,arterial=181204,local=1015861",
        ],
        "有限の正の数",
    );
    assert_cli_error(
        &["--exposure-method", "unknown"],
        "route-code・osm・factors",
    );
    assert_cli_error(
        &[
            "--exposure-method",
            "factors",
            "--exposure-factors",
            "motorway=0.27,arterial=1,local=13.5",
            "--exposure-clusters",
            "unknown",
        ],
        "top_group",
    );
}

#[test]
fn exposure_with_c_other_than_one_is_rejected() {
    let edges = vec![EdgeSpecInput::road(11, 1000)];
    let coords = coords_for(&edges);
    let fixture = fixture(
        edges,
        coords,
        vec![2],
        vec![0],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![AccidentSpec::car_edge(2020, 0, 1.0)],
    );
    let input = RiskFitInput {
        graph: &fixture.parts,
        graph_attrs: &fixture.attrs,
        topology: &fixture.topology,
        accident_attrs: &fixture.accident_attrs,
        assignment: &fixture.assignment,
        accidents: &fixture.columns,
    };
    let config = RiskFitConfig {
        train_years: vec![2020],
        validate_year: Some(2021),
        c: 2.0,
        exposure: Some(factors([0.27, 1.0, 13.5], ExposureClusters::TopGroup)),
        inherit: None,
    };
    let error = match fit_risk(&input, &config, &|_| {}) {
        Ok(_) => panic!("--cが1以外なら失敗するはず"),
        Err(error) => error,
    };
    assert!(error.contains("--c"), "{error}");
}

#[test]
fn osm_audit_includes_route_code_alternate_when_lengths_are_given() {
    let edges = vec![
        EdgeSpecInput::road(0, 10_000),
        EdgeSpecInput::road(2, 20_000),
        EdgeSpecInput::road(11, 30_000),
    ];
    let coords = coords_for(&edges);
    let accidents = vec![
        AccidentSpec::car_edge(2020, 0, 1.0),
        AccidentSpec::car_edge(2020, 1, 1.0),
        AccidentSpec::car_edge(2020, 2, 1.0),
    ];
    let fixture = fixture(
        edges,
        coords,
        vec![2, 2, 2],
        vec![0, 0, 0],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        accidents,
    );
    let lengths = [10.0, 20.0, 30.0];
    let mut exposure = osm([9.0, 62.0, 142.0], ExposureClusters::TopGroup);
    exposure.route_code_lengths_km = Some(lengths);
    let output = run_output(&fixture, &[2020], Some(2021), Some(exposure));
    let alternate = &output.audit["exposure"]["alternate"];
    assert_eq!(alternate["method"], "route-code");
    assert_eq!(
        alternate["categories"][0]["route_classes"],
        json!([5, 6, 7])
    );
    for cat in 0..3 {
        assert_relative(
            value(&alternate["categories"][cat], "length_km"),
            lengths[cat],
            "route-codeの延長",
        );
    }
}
