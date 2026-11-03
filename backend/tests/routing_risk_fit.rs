//! P6 `routing_prep risk-fit` の推定（形式書14章、作業書M3b.1）の合成入力によるテスト。
use accident_route_backend::contract::{ConditionFilter, Mode};
use accident_route_backend::data::routing_files::{Deps, decode_risk, encode_risk, required_deps};
use accident_route_backend::domain::routing::accident::{
    AccidentAttr, AccidentAttrTable, AssignTarget, Assignment, EdgeAccidentIndex, UnassignedReason,
    UnassignedRecord,
};
use accident_route_backend::domain::routing::graph::time::TimeModel;
use accident_route_backend::domain::routing::graph::{
    Coord, DirEdge, EdgeRecord, Graph, GraphParts,
};
use accident_route_backend::domain::routing::prep::attrs::GraphAttrs;
use accident_route_backend::domain::routing::prep::risk_fit::{
    InheritedFit, RiskFitConfig, RiskFitInput, RiskFitOutput, fit_risk,
};
use accident_route_backend::domain::routing::risk::{EDGE_K_NO_RISK, EDGE_K_SIDEWALK, RiskView};
use accident_route_backend::domain::routing::topology::{
    ClusterRecord, SidewalkParent, Topology, TwinPair,
};
use accident_route_backend::domain::snapshot::{AccidentColumns, AccidentSnapshot};

#[derive(Clone, Copy)]
struct EdgeSpec {
    tail: u32,
    head: u32,
    length_dm: u32,
    class: u8,
    flags: u8,
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
}

impl AccidentSpec {
    fn edge(year: u16, target: u32) -> Self {
        Self {
            lat: 35.0,
            lon: 135.0,
            modes: Mode::Walk.mask(),
            period: 0,
            weather: 1,
            day_type: 0,
            deaths: 0,
            injuries: 1,
            year,
            targets: vec![(0, target, 0, 1.0, false)],
            terrain: 2,
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
    edges: Vec<EdgeSpec>,
}

#[allow(clippy::too_many_arguments)]
fn fixture(
    edges: Vec<EdgeSpec>,
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
        attrs.push(attr);
        for &(kind, target, pos_dm, weight, mirror) in &a.targets {
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
        params: serde_json::Value::Null,
        offsets,
        targets,
        unassigned,
    };
    let attrs_table = AccidentAttrTable {
        params: serde_json::Value::Null,
        attrs,
    };
    let topology = Topology {
        params: serde_json::Value::Null,
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
    let graph_attrs = GraphAttrs {
        params: serde_json::Value::Null,
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
        params: serde_json::Value::Null,
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
                access: 0x30,
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

fn run_output(
    fixture: &Fixture,
    train: &[u16],
    validate: Option<u16>,
    inherit: Option<InheritedFit>,
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
        inherit,
    };
    fit_risk(&input, &config, &|_| {}).unwrap()
}

fn run(
    fixture: &Fixture,
    train: &[u16],
    validate: u16,
) -> accident_route_backend::domain::routing::risk::RiskModel {
    run_output(fixture, train, Some(validate), None).model
}

fn road(class: u8) -> EdgeSpec {
    EdgeSpec {
        tail: 0,
        head: 1,
        length_dm: 1000,
        class,
        flags: 0,
    }
}

fn coords_for(edges: &[EdgeSpec]) -> Vec<Coord> {
    let nodes = edges
        .iter()
        .map(|e| e.tail.max(e.head) as usize + 1)
        .max()
        .unwrap_or(0);
    (0..nodes)
        .map(|i| Coord::new(350_000_000 + i as i32 * 1000, 1_350_000_000))
        .collect()
}

fn approx(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "{a} != {b}");
}

#[test]
fn edgek_follows_sidewalk_no_risk_band_and_twins() {
    let residential = 11;
    let edges = vec![
        road(18),          // 0: 歩道（親あり）
        road(16),          // 1: path（車道でない）
        road(residential), // 2
        road(residential), // 3
        road(residential), // 4
        road(residential), // 5
        road(residential), // 6
        road(residential), // 7
        road(residential), // 8 双子
        road(residential), // 9 双子
        road(residential), // 10 双子
        road(residential), // 11 双子
        road(residential), // 12 双子
        road(residential), // 13 双子
        EdgeSpec {
            flags: 0x02,
            ..road(residential)
        }, // 14 歩道の印だけ
    ];
    let coords = coords_for(&edges);
    let twins = vec![(8, 9), (9, 8), (10, 11), (11, 10), (12, 13), (13, 12)];
    let fixture = fixture(
        edges,
        coords,
        vec![0, 0, 1, 2, 0, 0, 3, 4, 1, 3, 0, 0, 0, 0, 2],
        vec![0, 0, 0, 400, 54, 55, 1400, 10, 0, 0, 30, 40, 0, 0, 0],
        twins,
        vec![(0, 2)],
        Vec::new(),
        Vec::new(),
    );
    let model = run(&fixture, &[2020], 2021);
    assert_eq!(model.edge_k[0], EDGE_K_SIDEWALK);
    assert_eq!(model.edge_k[1], EDGE_K_NO_RISK);
    // group6（residential）: k=6*8+band*2+urban
    assert_eq!(model.edge_k[2], 6 * 8 + 2);
    assert_eq!(model.edge_k[3], 6 * 8 + 4);
    assert_eq!(model.edge_k[4], 6 * 8 + 2);
    assert_eq!(model.edge_k[5], 6 * 8 + 4);
    assert_eq!(model.edge_k[6], 6 * 8 + 4);
    assert_eq!(model.edge_k[7], 6 * 8 + 6);
    assert_eq!(model.edge_k[8], 6 * 8 + 6);
    assert_eq!(model.edge_k[9], 6 * 8 + 6);
    assert_eq!(model.edge_k[10], 6 * 8 + 4);
    assert_eq!(model.edge_k[11], 6 * 8 + 4);
    assert_eq!(model.edge_k[12], 6 * 8);
    assert_eq!(model.edge_k[13], 6 * 8);
    assert_eq!(model.edge_k[14], EDGE_K_NO_RISK);
}

#[test]
fn urban_is_decided_from_the_surrounding_train_accidents() {
    let edges = vec![
        EdgeSpec {
            tail: 0,
            head: 1,
            length_dm: 1000,
            class: 11,
            flags: 0,
        },
        EdgeSpec {
            tail: 2,
            head: 3,
            length_dm: 1000,
            class: 11,
            flags: 0,
        },
    ];
    let coords = vec![
        Coord::new(350_000_000, 1_350_000_000),
        Coord::new(350_000_000, 1_350_000_000),
        Coord::new(360_000_000, 1_360_000_000),
        Coord::new(360_000_000, 1_360_000_000),
    ];
    // 1本目: セルと周囲8セルで10件中6件が地形1 → urban=1。
    // 2本目: 9件だけ → urban=0。
    let mut accidents = Vec::new();
    for i in 0..10 {
        let mut a = AccidentSpec::edge(2020, 0);
        a.targets.clear();
        a.terrain = if i < 6 { 1 } else { 2 };
        if i % 2 == 0 {
            a.lat = 35.001;
        } else {
            a.lon = 135.0025;
        }
        accidents.push(a);
    }
    for _ in 0..9 {
        let mut a = AccidentSpec::edge(2020, 0);
        a.targets.clear();
        a.lat = 36.0;
        a.lon = 136.0;
        a.terrain = 1;
        accidents.push(a);
    }
    let fixture = fixture(
        edges,
        coords,
        vec![2, 2],
        vec![0, 0],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        accidents,
    );
    let model = run(&fixture, &[2020], 2021);
    assert_eq!(model.edge_k[0], 6 * 8 + 4 + 1);
    assert_eq!(model.edge_k[1], 6 * 8 + 4);
}

#[test]
fn clusj_uses_top_group_branch_band_and_signal() {
    let cluster = |top_class, branches, signal| ClusterRecord {
        lat_e7: 0,
        lon_e7: 0,
        node_start: 0,
        node_count: 0,
        top_class,
        branches,
        flags: if signal { 0x01 } else { 0 },
        pad: [0; 3],
    };
    let fixture = fixture(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![
            cluster(11, 4, true),
            cluster(15, 5, false),
            cluster(255, 0, false),
        ],
        Vec::new(),
    );
    let model = run(&fixture, &[2020], 2021);
    assert_eq!(model.cluster_j, vec![6 * 8 + 2 + 1, 9 * 8 + 4, 9 * 8]);
}

#[test]
fn density_severity_and_weather_match_hand_calculation() {
    let edges = vec![EdgeSpec {
        length_dm: 10_000,
        ..road(11)
    }];
    let coords = coords_for(&edges);
    let mut accidents = Vec::new();
    for i in 0..20 {
        let mut a = AccidentSpec::edge(if i < 10 { 2019 } else { 2020 }, 0);
        a.weather = match i {
            0..=3 => 3,
            4..=6 => 4,
            7..=9 => 5,
            _ => 1,
        };
        if i < 2 {
            a.deaths = 1;
            a.injuries = 0;
        }
        accidents.push(a);
    }
    let fixture = fixture(
        edges,
        coords,
        vec![2],
        vec![0],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        accidents,
    );
    let model = run(&fixture, &[2019, 2020], 2021);
    let kind = 6 * 8 + 4;
    let coef = model.k_coef(Mode::Walk, kind, 0);
    approx(coef.r, 20.0 / (2.0 * 1000.0));
    approx(coef.s, 38.0 / 20.0);
    approx(f64::from(coef.rho[2]), 0.2);
    approx(f64::from(coef.rho[3]), 0.15);
    approx(f64::from(coef.rho[4]), 0.15);
    approx(f64::from(coef.rho[0]), 0.5);
}

#[test]
fn pooling_uses_the_group_and_urban_parent() {
    let edges = vec![road(11), road(11)];
    let coords = (0..4)
        .map(|i| Coord::new(350_000_000 + i * 10, 1_350_000_000))
        .collect();
    let mut accidents = Vec::new();
    for _ in 0..10 {
        let mut a = AccidentSpec::edge(2020, 0);
        a.deaths = 1;
        a.injuries = 0;
        a.weather = 3;
        accidents.push(a);
    }
    for _ in 0..30 {
        accidents.push(AccidentSpec::edge(2020, 1));
    }
    let fixture = fixture(
        edges,
        coords,
        vec![1, 2],
        vec![0, 0],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        accidents,
    );
    let model = run(&fixture, &[2020], 2021);
    let narrow = 6 * 8 + 2;
    let middle = 6 * 8 + 4;
    approx(model.k_coef(Mode::Walk, narrow, 0).r, 40.0 / 200.0);
    approx(model.k_coef(Mode::Walk, middle, 0).r, 30.0 / 100.0);
    approx(model.k_coef(Mode::Walk, narrow, 0).s, 130.0 / 40.0);
    approx(f64::from(model.k_coef(Mode::Walk, narrow, 0).rho[2]), 0.25);
    let pooling = &model.params.pooling;
    let entry = pooling["edges"]["walk"]["r"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["kind"] == narrow)
        .expect("狭い種別のまとめがある");
    assert_eq!(entry["level"], "group_urban");
}

#[test]
fn shrinkage_weight_is_large_for_correlated_edges_and_small_for_average_edges() {
    let make = |strong: bool| {
        let edges: Vec<EdgeSpec> = (0..100)
            .map(|i| EdgeSpec {
                tail: i * 2,
                head: i * 2 + 1,
                length_dm: 1000,
                class: 11,
                flags: 0,
            })
            .collect();
        let coords: Vec<Coord> = (0..200)
            .map(|i| Coord::new(350_000_000 + i * 10, 1_350_000_000))
            .collect();
        let mut accidents = Vec::new();
        for i in 0..100 {
            accidents.push(AccidentSpec::edge(2020, if strong { 0 } else { i }));
        }
        for i in 0..100 {
            accidents.push(AccidentSpec::edge(2021, if strong { 0 } else { i }));
        }
        let fixture = fixture(
            edges,
            coords,
            vec![2; 100],
            vec![0; 100],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            accidents,
        );
        let model = run(&fixture, &[2020], 2021);
        let kind = 6 * 8 + 4;
        model.params.fit["edges"]["walk"]["kinds"][kind.to_string()]["w"]
            .as_f64()
            .unwrap()
    };
    let strong = make(true);
    let average = make(false);
    assert!(strong >= 0.9, "相関が強いときの W={strong}");
    assert!(average <= 0.02, "平均に近いときの W={average}");
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
        params: serde_json::Value::Null,
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

#[test]
fn model_round_trips_and_risk_view_is_finite() {
    let edges = vec![road(11), road(11)];
    let coords = coords_for(&edges);
    let mut accidents = vec![AccidentSpec::edge(2020, 0), AccidentSpec::edge(2020, 1)];
    accidents[0].weather = 3;
    let fixture = fixture(
        edges,
        coords,
        vec![2, 1],
        vec![0, 0],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        accidents,
    );
    let model = run(&fixture, &[2020], 2021);

    let deps: Deps = required_deps("risk.bin")
        .iter()
        .map(|key| ((*key).to_owned(), "0".repeat(64)))
        .collect();
    let bytes = encode_risk(&model, &deps);
    let decoded = decode_risk(&bytes).unwrap();
    assert_eq!(decoded.value.edge_k, model.edge_k);
    assert_eq!(decoded.value.cluster_j, model.cluster_j);
    assert_eq!(decoded.value.k_table, model.k_table);
    assert_eq!(decoded.value.j_table, model.j_table);
    assert_eq!(
        decoded.value.params.train_source_years,
        model.params.train_source_years
    );
    assert_eq!(decoded.value.params.years, model.params.years);
    assert_eq!(decoded.value.params.c, model.params.c);
    assert_eq!(decoded.value.params.c_groups, model.params.c_groups);
    assert_eq!(decoded.value.params.beta, model.params.beta);
    assert_eq!(decoded.value.params.beta_cluster, model.params.beta_cluster);
    assert_eq!(
        decoded.value.params.weather_multiplier,
        model.params.weather_multiplier
    );
    assert_eq!(decoded.value.params.urban, model.params.urban);
    assert_eq!(decoded.value.params.pooling, model.params.pooling);
    if decoded.value.params.fit != model.params.fit {
        let a = serde_json::to_string(&decoded.value.params.fit).unwrap();
        let b = serde_json::to_string(&model.params.fit).unwrap();
        let i = a
            .bytes()
            .zip(b.bytes())
            .position(|(x, y)| x != y)
            .unwrap_or(a.len().min(b.len()));
        panic!(
            "fitが違います（{}バイトと{}バイト、{}文字目）: {} / {}",
            a.len(),
            b.len(),
            i,
            &a[i.saturating_sub(120)..(i + 180).min(a.len())],
            &b[i.saturating_sub(120)..(i + 180).min(b.len())],
        );
    }
    if decoded.value.raw_params != model.raw_params {
        let a = serde_json::to_string(&decoded.value.raw_params).unwrap();
        let b = serde_json::to_string(&model.raw_params).unwrap();
        let i = a
            .bytes()
            .zip(b.bytes())
            .position(|(x, y)| x != y)
            .unwrap_or(a.len().min(b.len()));
        panic!(
            "raw_paramsが違います（{}バイトと{}バイト、{}文字目）: {} / {}",
            a.len(),
            b.len(),
            i,
            &a[i.saturating_sub(80)..(i + 120).min(a.len())],
            &b[i.saturating_sub(80)..(i + 120).min(b.len())],
        );
    }
    assert_eq!(decoded.deps, deps);
    assert_eq!(decoded.value, model);

    let parts = valid_graph_parts(&fixture);
    let graph = Graph::from_parts(parts).unwrap();
    let index = EdgeAccidentIndex::build(&fixture.assignment, 2, 0);
    let snapshot = AccidentSnapshot::new(fixture.columns.clone());
    let mut view = RiskView::new(
        &decoded.value,
        &graph,
        &fixture.topology,
        &index,
        &snapshot,
        Mode::Walk,
        ConditionFilter::default(),
    );
    for e in 0..2 {
        let risk = view.edge_risk(e);
        assert!(risk.is_finite() && risk >= 0.0, "edge{e}のrisk={risk}");
    }
}

fn inherited_fixture() -> Fixture {
    let edges = vec![
        EdgeSpec {
            tail: 0,
            head: 1,
            length_dm: 1000,
            class: 11,
            flags: 0,
        },
        EdgeSpec {
            tail: 0,
            head: 1,
            length_dm: 1000,
            class: 11,
            flags: 0,
        },
    ];
    let coords = coords_for(&edges);
    let cluster = ClusterRecord {
        lat_e7: 0,
        lon_e7: 0,
        node_start: 0,
        node_count: 0,
        top_class: 11,
        branches: 4,
        flags: 0x01,
        pad: [0; 3],
    };
    let all_modes = Mode::Walk.mask() | Mode::Bike.mask() | Mode::Car.mask();
    let mut accidents = Vec::new();
    for i in 0..80 {
        let mut a = AccidentSpec::edge(2020, if i < 60 { 0 } else { 1 });
        a.modes = all_modes;
        a.weather = [1, 3, 4, 5, 1][i % 5];
        accidents.push(a);
    }
    for i in 0..40 {
        let mut a = AccidentSpec::edge(2021, if i < 30 { 0 } else { 1 });
        a.modes = all_modes;
        a.weather = [1, 3, 4, 5, 1][i % 5];
        accidents.push(a);
    }
    for i in 0..20 {
        let mut a = AccidentSpec::edge(2020, 0);
        a.targets = vec![(1, 0, u32::MAX, 1.0, false)];
        a.modes = all_modes;
        a.weather = [1, 3, 4, 5, 1][i % 5];
        accidents.push(a);
    }
    for i in 0..10 {
        let mut a = AccidentSpec::edge(2021, 0);
        a.targets = vec![(1, 0, u32::MAX, 1.0, false)];
        a.modes = all_modes;
        a.weather = [1, 3, 4, 5, 1][i % 5];
        accidents.push(a);
    }
    fixture(
        edges,
        coords,
        vec![2, 1],
        vec![0, 0],
        Vec::new(),
        Vec::new(),
        vec![cluster],
        accidents,
    )
}

fn assert_relative(actual: f64, expected: f64, what: &str) {
    let scale = actual.abs().max(expected.abs()).max(1.0);
    assert!(
        (actual - expected).abs() <= 1e-9 * scale,
        "{what}: {actual} != {expected}（相対誤差）"
    );
}

fn kind_entries<'a>(
    value: &'a serde_json::Value,
    section: &str,
    mode: Mode,
) -> &'a serde_json::Map<String, serde_json::Value> {
    value[section][mode.as_str()]["kinds"]
        .as_object()
        .unwrap_or_else(|| panic!("{section}.{}.kindsがない", mode.as_str()))
}

#[test]
fn inherited_fit_reuses_w_beta_and_a_for_the_same_train_years() {
    let fixture = inherited_fixture();
    let reference = run(&fixture, &[2020], 2021);
    let inherited = InheritedFit::from_model(&reference, serde_json::json!({"test": "same years"}))
        .expect("参照から引き継げる");
    let inherited_model = run_output(&fixture, &[2020], None, Some(inherited)).model;

    for mode in Mode::ALL {
        assert_relative(
            *inherited_model.params.beta.get(mode),
            *reference.params.beta.get(mode),
            &format!("{}のbeta", mode.as_str()),
        );
        assert_relative(
            *inherited_model.params.beta_cluster.get(mode),
            *reference.params.beta_cluster.get(mode),
            &format!("{}のbeta_cluster", mode.as_str()),
        );
        let reference_edges = kind_entries(&reference.params.fit, "edges", mode);
        let inherited_edges = kind_entries(&inherited_model.params.fit, "edges", mode);
        assert_eq!(reference_edges.len(), inherited_edges.len());
        for (kind, entry) in reference_edges {
            let expected = entry["w"].as_f64().expect("参照のW");
            let actual = inherited_edges[kind]["w"].as_f64().expect("引き継いだW");
            assert_relative(actual, expected, &format!("{}の辺{kind}のW", mode.as_str()));
        }
        let reference_clusters = kind_entries(&reference.params.fit, "clusters", mode);
        let inherited_clusters = kind_entries(&inherited_model.params.fit, "clusters", mode);
        assert_eq!(reference_clusters.len(), inherited_clusters.len());
        for (kind, entry) in reference_clusters {
            let expected = entry["w"].as_f64().expect("参照のW");
            let actual = inherited_clusters[kind]["w"].as_f64().expect("引き継いだW");
            assert_relative(
                actual,
                expected,
                &format!("{}の交差点{kind}のW", mode.as_str()),
            );
        }
        for cell in 0..10 {
            for kind in 0..256_u16 {
                let k = kind as u8;
                assert_relative(
                    inherited_model.k_coef(mode, k, cell).a,
                    reference.k_coef(mode, k, cell).a,
                    &format!("{}の辺{kind}のa", mode.as_str()),
                );
                assert_relative(
                    inherited_model.j_coef(mode, k, cell).a,
                    reference.j_coef(mode, k, cell).a,
                    &format!("{}の交差点{kind}のa", mode.as_str()),
                );
            }
        }
    }
}

#[test]
fn inherited_a_is_recomputed_from_the_new_train_years() {
    let fixture = inherited_fixture();
    let reference = run(&fixture, &[2020], 2021);
    let inherited = InheritedFit::from_model(&reference, serde_json::json!({"test": "new years"}))
        .expect("参照から引き継げる");
    let model = run_output(&fixture, &[2020, 2021], None, Some(inherited)).model;
    let years = 2.0;

    for mode in Mode::ALL {
        for section in ["edges", "clusters"] {
            let kinds = kind_entries(&model.params.fit, section, mode);
            for (kind, entry) in kinds {
                let w = entry["w"].as_f64().expect("W");
                let r = entry["r_all_cells"].as_f64().expect("R");
                let actual = entry["a"].as_f64().expect("a");
                let expected = years * r * (1.0 / w - 1.0);
                assert_relative(
                    actual,
                    expected,
                    &format!("{}の{section}{kind}のa", mode.as_str()),
                );
            }
        }
    }
}

#[test]
fn inherited_fit_falls_back_to_a_parent_when_the_reference_lacks_the_kind() {
    let fixture = inherited_fixture();
    let mut reference = run(&fixture, &[2020], 2021);
    let removed = {
        let kinds = reference.params.fit["edges"]["walk"]["kinds"]
            .as_object_mut()
            .expect("辺の種別");
        assert!(kinds.len() >= 2, "親をたどる種別が2つ以上ある");
        let removed = kinds.keys().next().expect("種別").clone();
        kinds.remove(&removed);
        removed
    };
    let inherited = InheritedFit::from_model(&reference, serde_json::json!({"test": "missing W"}))
        .expect("参照にない種別があっても引き継げる");
    let output = run_output(&fixture, &[2020], None, Some(inherited));
    let entry = &output.audit["fit"]["edges"]["walk"]["kinds"][removed.as_str()];
    assert!(entry["w"].as_f64().is_some());
    assert_eq!(entry["w_source"], "inherited-parent");
}
