//! 評価用のリスクの切り替え（`RiskVariant`）と、`route-batch` の探索の目的（`--objective length|random`）
//! の受け入れ試験（作業書M5.4〜M5.7の準備）。
//!
//! - `Standard` が既定の計算と同じ（1ビットも変えない）。
//! - `no-eb`：事故0件の辺は0、事故がある辺は `c·S·n·(天候の項)`。
//! - `edge-severity`：辺を2つに分けても合計が変わらない（`tests/routing_risk.rs` と同じ作り方）。
//! - `length` の探索は上限内で道のり最小、`random` は同じseedで同じ値。
//! - `class_lengths_dm` の合計が経路の道のりと一致する。

use accident_route_backend::contract::{
    CancelToken, ConditionFilter, DayType, Mode, Period, Weather,
};
use accident_route_backend::domain::routing::NONE;
use accident_route_backend::domain::routing::accident::{
    ClusterAccident, EdgeAccident, EdgeAccidentIndex,
};
use accident_route_backend::domain::routing::graph::restrictions::TurnAutomaton;
use accident_route_backend::domain::routing::graph::synthetic::{EdgeSpec, GraphBuilder, bwd, fwd};
use accident_route_backend::domain::routing::graph::time::{PerMode, TimeModel};
use accident_route_backend::domain::routing::graph::{DirEdge, EdgeId, Graph, HighwayClass};
use accident_route_backend::domain::routing::risk::arc_cost::path_risk;
use accident_route_backend::domain::routing::risk::{
    CELL_COUNT, CoefRecord, EDGE_K_NO_RISK, RiskModel, RiskParams, RiskVariant, RiskView,
    TABLE_LEN, WeatherMultiplier, cluster_class_j, road_class_k, table_index,
};
use accident_route_backend::domain::routing::search::alt::Landmarks;
use accident_route_backend::domain::routing::search::endpoints::path_distance_dm;
use accident_route_backend::domain::routing::search::eval_cost::{
    LengthArcCost, RandomArcCost, class_lengths_dm,
};
use accident_route_backend::domain::routing::search::route::find_routes;
use accident_route_backend::domain::routing::search::{
    ArcCost, Path, SearchContext, SearchParams, risk_to_fx,
};
use accident_route_backend::domain::routing::snap::SnapPoint;
use accident_route_backend::domain::routing::topology::{ClusterRecord, SidewalkParent, Topology};
use accident_route_backend::domain::snapshot::{AccidentColumns, AccidentSnapshot};
use serde_json::json;

// ---------------------------------------------------------------------------------------------
// 組み立ての道具（tests/routing_risk.rs と同じ形）
// ---------------------------------------------------------------------------------------------

fn base_params() -> RiskParams {
    RiskParams {
        train_source_years: vec![2019, 2020, 2021, 2022],
        years: 2,
        c: 1.5,
        c_groups: vec![5, 6, 7, 8],
        car_exposure: None,
        beta: PerMode {
            walk: 0.2,
            bike: 0.3,
            car: 0.4,
        },
        beta_cluster: PerMode {
            walk: 0.25,
            bike: 0.35,
            car: 0.3,
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

fn coef(r: f64, s: f64, a: f64, rho: [f32; 5]) -> CoefRecord {
    CoefRecord {
        r,
        s,
        a,
        rho,
        pad: 0.0,
    }
}

fn empty_model(edge_k: Vec<u8>, cluster_j: Vec<u8>) -> RiskModel {
    RiskModel {
        params: base_params(),
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

fn topology(
    node_cluster: Vec<u32>,
    clusters: Vec<ClusterRecord>,
    cluster_nodes: Vec<u32>,
    sidewalk_parents: Vec<SidewalkParent>,
    edge_count: usize,
) -> Topology {
    let words = edge_count.div_ceil(64);
    Topology {
        params: json!({}),
        snap_car: vec![u64::MAX; words],
        snap_bike: vec![u64::MAX; words],
        snap_walk: vec![u64::MAX; words],
        node_cluster,
        clusters,
        cluster_nodes,
        twins: Vec::new(),
        sidewalk_parents,
    }
}

fn cluster_record(node_start: u32, node_count: u16) -> ClusterRecord {
    ClusterRecord {
        lat_e7: 0,
        lon_e7: 0,
        node_start,
        node_count,
        top_class: 4,
        branches: 3,
        flags: 0,
        pad: [0; 3],
    }
}

/// 1件の事故の条件・人数と、辺・クラスタへの割り当て。
#[derive(Debug, Clone)]
struct AccidentSpec {
    modes: u8,
    period: u8,
    weather: u8,
    day_type: u8,
    source_year: u16,
    deaths: u16,
    injuries: u16,
    /// `(辺, 位置dm, 重み, MIRROR)`。
    edge_targets: Vec<(u32, u32, f32, bool)>,
    /// `(クラスタ, 重み)`。
    cluster_targets: Vec<(u32, f32)>,
}

impl AccidentSpec {
    #[allow(clippy::too_many_arguments)]
    fn on_edge(
        modes: u8,
        period: u8,
        weather: u8,
        day_type: u8,
        source_year: u16,
        deaths: u16,
        injuries: u16,
        edge: u32,
        pos_dm: u32,
        weight: f32,
        mirror: bool,
    ) -> Self {
        Self {
            modes,
            period,
            weather,
            day_type,
            source_year,
            deaths,
            injuries,
            edge_targets: vec![(edge, pos_dm, weight, mirror)],
            cluster_targets: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn on_cluster(
        modes: u8,
        period: u8,
        weather: u8,
        day_type: u8,
        source_year: u16,
        deaths: u16,
        injuries: u16,
        cluster: u32,
        weight: f32,
    ) -> Self {
        Self {
            modes,
            period,
            weather,
            day_type,
            source_year,
            deaths,
            injuries,
            edge_targets: Vec::new(),
            cluster_targets: vec![(cluster, weight)],
        }
    }
}

fn snapshot(specs: &[AccidentSpec]) -> AccidentSnapshot {
    let mut columns = AccidentColumns::default();
    for spec in specs {
        columns.lat.push(35.0);
        columns.lon.push(139.0);
        columns.modes.push(spec.modes);
        columns.period.push(spec.period);
        columns.weather.push(spec.weather);
        columns.day_type.push(spec.day_type);
        columns.deaths.push(spec.deaths);
        columns.injuries.push(spec.injuries);
        columns.source_year.push(spec.source_year);
        columns.date.push(20200101);
    }
    AccidentSnapshot::new(columns)
}

#[allow(clippy::too_many_arguments)]
fn accident_index(
    specs: &[AccidentSpec],
    edge_count: usize,
    cluster_count: usize,
) -> EdgeAccidentIndex {
    let mut per_edge: Vec<Vec<EdgeAccident>> = vec![Vec::new(); edge_count];
    let mut per_cluster: Vec<Vec<ClusterAccident>> = vec![Vec::new(); cluster_count];
    for (accident, spec) in specs.iter().enumerate() {
        for &(edge, pos_dm, weight, mirror) in &spec.edge_targets {
            per_edge[edge as usize].push(EdgeAccident {
                accident: accident as u32,
                pos_dm,
                weight,
                mirror,
            });
        }
        for &(cluster, weight) in &spec.cluster_targets {
            per_cluster[cluster as usize].push(ClusterAccident {
                accident: accident as u32,
                weight,
            });
        }
    }
    let mut edge_ofs = Vec::with_capacity(edge_count + 1);
    edge_ofs.push(0_u32);
    let mut edge_entries = Vec::new();
    for entries in &mut per_edge {
        entries.sort_by_key(|entry| (entry.pos_dm, entry.accident));
        edge_entries.extend_from_slice(entries);
        edge_ofs.push(edge_entries.len() as u32);
    }
    let mut cluster_ofs = Vec::with_capacity(cluster_count + 1);
    cluster_ofs.push(0_u32);
    let mut cluster_entries = Vec::new();
    for entries in &mut per_cluster {
        cluster_entries.extend_from_slice(entries);
        cluster_ofs.push(cluster_entries.len() as u32);
    }
    EdgeAccidentIndex {
        edge_ofs,
        edge_entries,
        cluster_ofs,
        cluster_entries,
    }
}

#[allow(clippy::too_many_arguments)]
fn risk_view<'a>(
    model: &'a RiskModel,
    graph: &'a Graph,
    topology: &'a Topology,
    accidents: &'a EdgeAccidentIndex,
    snapshot: &'a AccidentSnapshot,
    mode: Mode,
    filter: ConditionFilter,
    variant: RiskVariant,
) -> RiskView<'a> {
    RiskView::new(model, graph, topology, accidents, snapshot, mode, filter).with_variant(variant)
}

fn path(dedges: Vec<DirEdge>, start: SnapPoint, end: SnapPoint) -> Path {
    Path {
        dedges,
        start,
        end,
        time_ds: 0,
        risk_fx: 0,
    }
}

fn assert_relative_close(actual: f64, expected: f64, what: &str) {
    let difference = (actual - expected).abs();
    let scale = actual.abs().max(expected.abs());
    if scale == 0.0 {
        assert_eq!(actual, expected, "{what}");
    } else {
        assert!(
            difference / scale <= 1e-9,
            "{what}: actual={actual}, expected={expected}（相対誤差{}）",
            difference / scale
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Standard は既定の計算と同じ
// ---------------------------------------------------------------------------------------------

#[test]
fn standard_variant_matches_the_default_calculation() {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.0, 139.0);
    let b = builder.add_node_deg(35.0, 139.001);
    let e = builder.add_edge(EdgeSpec::new(a, b).length_dm(1000));
    let graph = builder.build();
    let topo = topology(
        vec![NONE, NONE],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        graph.edge_count(),
    );
    let k = road_class_k(6, 1, true);
    let mut model = empty_model(vec![k], Vec::new());
    fill_k(&mut model, Mode::Car, k, |cell| {
        coef(
            1e-5 * (1.0 + cell as f64 * 0.3),
            5.0 + cell as f64 * 0.4,
            0.3 + cell as f64 * 0.01,
            [0.1, 0.2, 0.3, 0.2, 0.2],
        )
    });
    let specs = vec![
        AccidentSpec::on_edge(
            4,
            Period::Daytime.code(),
            Weather::Rain.code(),
            DayType::Weekday.code(),
            2020,
            1,
            2,
            e,
            200,
            1.3,
            false,
        ),
        AccidentSpec::on_edge(
            4,
            Period::Night.code(),
            Weather::Clear.code(),
            DayType::Holiday.code(),
            2021,
            0,
            3,
            e,
            700,
            0.7,
            true,
        ),
    ];
    let snapshot = snapshot(&specs);
    let index = accident_index(&specs, graph.edge_count(), 0);
    let filter = ConditionFilter::default();
    let mut standard = risk_view(
        &model,
        &graph,
        &topo,
        &index,
        &snapshot,
        Mode::Car,
        filter,
        RiskVariant::Standard,
    );
    let mut default = risk_view(
        &model,
        &graph,
        &topo,
        &index,
        &snapshot,
        Mode::Car,
        filter,
        RiskVariant::Standard,
    );
    assert_eq!(RiskVariant::default(), RiskVariant::Standard);
    // 同じ計算であること（`with_variant(Standard)` を通しても値が変わらない）。
    assert_eq!(standard.edge_risk(e), default.edge_risk(e));
    assert_eq!(
        standard.partial_edge_risk(e, 100, 800),
        default.partial_edge_risk(e, 100, 800)
    );
    assert!(standard.edge_risk(e) > 0.0);
}

// ---------------------------------------------------------------------------------------------
// 経験ベイズなし
// ---------------------------------------------------------------------------------------------

#[test]
fn no_empirical_bayes_uses_the_observed_weight() {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.0, 139.0);
    let b = builder.add_node_deg(35.0, 139.001);
    let c = builder.add_node_deg(35.0, 139.002);
    let e = builder.add_edge(EdgeSpec::new(a, b).length_dm(1000));
    let no_accidents = builder.add_edge(EdgeSpec::new(b, c).length_dm(500));
    let graph = builder.build();
    let topo = topology(
        vec![NONE, 0, NONE],
        vec![cluster_record(0, 1)],
        vec![b],
        Vec::new(),
        graph.edge_count(),
    );
    let k = road_class_k(6, 1, true);
    let j = cluster_class_j(4, 0, false);
    let mut model = empty_model(vec![k, k], vec![j]);
    fill_k(&mut model, Mode::Car, k, |_| {
        coef(1e-5, 12.5, 0.4, [0.1, 0.2, 0.3, 0.2, 0.2])
    });
    fill_j(&mut model, Mode::Car, j, |_| {
        coef(1e-3, 20.0, 0.5, [0.1, 0.2, 0.3, 0.2, 0.2])
    });
    let specs = vec![
        AccidentSpec::on_edge(
            4,
            Period::Daytime.code(),
            Weather::Rain.code(),
            DayType::Weekday.code(),
            2020,
            1,
            1,
            e,
            200,
            2.0,
            false,
        ),
        AccidentSpec::on_edge(
            4,
            Period::Daytime.code(),
            Weather::Clear.code(),
            DayType::Weekday.code(),
            2021,
            0,
            1,
            e,
            700,
            0.5,
            false,
        ),
        AccidentSpec::on_cluster(
            4,
            Period::Daytime.code(),
            Weather::Clear.code(),
            DayType::Weekday.code(),
            2020,
            0,
            2,
            0,
            1.25,
        ),
    ];
    let snapshot = snapshot(&specs);
    let index = accident_index(&specs, graph.edge_count(), 1);
    let c = model.params.c;
    let s = 12.5;

    // 晴・曇（天候の項なし）：c·S·n。事故0件の辺は0。
    let mut clear = risk_view(
        &model,
        &graph,
        &topo,
        &index,
        &snapshot,
        Mode::Car,
        ConditionFilter::default(),
        RiskVariant::NoEmpiricalBayes,
    );
    assert_relative_close(clear.edge_risk(e), c * s * 2.5, "no-eb 辺（晴・曇）");
    assert_eq!(clear.edge_risk(no_accidents), 0.0);
    assert_eq!(clear.partial_edge_risk(no_accidents, 0, 500), 0.0);
    assert_relative_close(clear.cluster_risk(0), 20.0 * 1.25, "no-eb クラスタ");

    // 雨：c·S·(n + (M−1)·n^w)。
    let rain = ConditionFilter {
        weather: Some(Weather::Rain),
        ..ConditionFilter::default()
    };
    let mut rainy = risk_view(
        &model,
        &graph,
        &topo,
        &index,
        &snapshot,
        Mode::Car,
        rain,
        RiskVariant::NoEmpiricalBayes,
    );
    assert_relative_close(
        rainy.edge_risk(e),
        c * s * (2.5 + (2.0 - 1.0) * 2.0),
        "no-eb 辺（雨）",
    );
    // 天候に一致しないクラスタの事故だけなら、天候の項は増えない。
    assert_relative_close(rainy.cluster_risk(0), 20.0 * 1.25, "no-eb クラスタ（雨）");
}

// ---------------------------------------------------------------------------------------------
// 深刻度を辺単位にした場合の分割の不変性
// ---------------------------------------------------------------------------------------------

/// 元の直線 A→B→C（e0=1000dm、e1=1200dm）と、B のクラスタ。
fn original_line() -> (Graph, Topology, EdgeId, EdgeId) {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.0, 139.0);
    let b = builder.add_node_deg(35.0, 139.001);
    let c = builder.add_node_deg(35.0, 139.002);
    let e0 = builder.add_edge(EdgeSpec::new(a, b).length_dm(1000));
    let e1 = builder.add_edge(EdgeSpec::new(b, c).length_dm(1200));
    let graph = builder.build();
    let topo = topology(
        vec![NONE, 0, NONE],
        vec![cluster_record(0, 1)],
        vec![b],
        Vec::new(),
        graph.edge_count(),
    );
    (graph, topo, e0, e1)
}

/// e0 を位置 p で2本に分けた直線 A→S→B→C。S はクラスタに属さない。
fn split_line(p: u32) -> (Graph, Topology, EdgeId, EdgeId, EdgeId) {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.0, 139.0);
    let s = builder.add_node_deg(35.0, 139.0005);
    let b = builder.add_node_deg(35.0, 139.001);
    let c = builder.add_node_deg(35.0, 139.002);
    let first = builder.add_edge(EdgeSpec::new(a, s).length_dm(p));
    let second = builder.add_edge(EdgeSpec::new(s, b).length_dm(1000 - p));
    let e1 = builder.add_edge(EdgeSpec::new(b, c).length_dm(1200));
    let graph = builder.build();
    let topo = topology(
        vec![NONE, NONE, 0, NONE],
        vec![cluster_record(0, 1)],
        vec![b],
        Vec::new(),
        graph.edge_count(),
    );
    (graph, topo, first, second, e1)
}

fn split_specs(
    p: u32,
    specs: &[AccidentSpec],
    e0: u32,
    first: u32,
    second: u32,
    e1_original: u32,
    e1_split: u32,
) -> Vec<AccidentSpec> {
    specs
        .iter()
        .map(|spec| {
            let edge_targets = spec
                .edge_targets
                .iter()
                .map(|&(edge, pos_dm, weight, mirror)| {
                    if edge == e0 {
                        if pos_dm < p {
                            (first, pos_dm, weight, mirror)
                        } else {
                            (second, pos_dm - p, weight, mirror)
                        }
                    } else if edge == e1_original {
                        (e1_split, pos_dm, weight, mirror)
                    } else {
                        (edge, pos_dm, weight, mirror)
                    }
                })
                .collect();
            AccidentSpec {
                edge_targets,
                cluster_targets: spec.cluster_targets.clone(),
                ..spec.clone()
            }
        })
        .collect()
}

#[test]
fn edge_severity_is_invariant_when_an_edge_is_split() {
    let p = 400;
    let (original_graph, original_topo, e0, e1) = original_line();
    let (split_graph, split_topo, first, second, split_e1) = split_line(p);

    let k0 = road_class_k(6, 1, true);
    let k1 = road_class_k(2, 0, false);
    let j = cluster_class_j(4, 0, false);
    let mut model = empty_model(vec![k0, k0, k1], vec![j]);
    fill_k(&mut model, Mode::Car, k0, |cell| {
        coef(
            1e-5 * (1.0 + cell as f64 * 0.5),
            5.0 + cell as f64,
            0.2 + cell as f64 * 0.02,
            [0.1, 0.2, 0.3, 0.2, 0.2],
        )
    });
    fill_k(&mut model, Mode::Car, k1, |cell| {
        coef(
            8e-6 * (1.0 + cell as f64 * 0.25),
            4.0 + cell as f64,
            0.3 + cell as f64 * 0.02,
            [0.2, 0.1, 0.3, 0.2, 0.2],
        )
    });
    fill_j(&mut model, Mode::Car, j, |cell| {
        coef(
            1e-3 * (1.0 + cell as f64 * 0.1),
            9.0 + cell as f64,
            0.4,
            [0.2, 0.1, 0.3, 0.2, 0.2],
        )
    });
    // 元のグラフは辺が2本（e0・e1）。分割したグラフの EDGEK は3本。
    let mut original_model = model.clone();
    original_model.edge_k = vec![k0, k1];
    let specs = vec![
        AccidentSpec::on_edge(
            4,
            Period::Daytime.code(),
            Weather::Rain.code(),
            DayType::Weekday.code(),
            2020,
            1,
            1,
            e0,
            100,
            1.3,
            false,
        ),
        AccidentSpec::on_edge(
            4,
            Period::Daytime.code(),
            Weather::Clear.code(),
            DayType::Weekday.code(),
            2021,
            0,
            2,
            e0,
            400,
            0.7,
            false,
        ),
        AccidentSpec::on_edge(
            4,
            Period::Night.code(),
            Weather::Rain.code(),
            DayType::Holiday.code(),
            2022,
            0,
            5,
            e0,
            700,
            2.0,
            false,
        ),
        AccidentSpec::on_edge(
            4,
            Period::Daytime.code(),
            Weather::Clear.code(),
            DayType::Weekday.code(),
            2020,
            0,
            3,
            e1,
            300,
            1.1,
            false,
        ),
        AccidentSpec::on_cluster(
            4,
            Period::Daytime.code(),
            Weather::Rain.code(),
            DayType::Weekday.code(),
            2021,
            1,
            0,
            0,
            0.9,
        ),
    ];
    let split_specs = split_specs(p, &specs, e0, first, second, e1, split_e1);
    let original_snapshot = snapshot(&specs);
    let split_snapshot = snapshot(&split_specs);
    let original_index = accident_index(&specs, original_graph.edge_count(), 1);
    let split_index = accident_index(&split_specs, split_graph.edge_count(), 1);

    let length_e1 = original_graph.edge(e1).length_dm;
    let split_length_e1 = split_graph.edge(split_e1).length_dm;
    let original_paths = [
        // 全長（A→C）。
        path(
            vec![fwd(e0), fwd(e1)],
            SnapPoint::on_edge(&original_graph, e0, 0),
            SnapPoint::on_edge(&original_graph, e1, length_e1),
        ),
        // 出発の辺の途中から。
        path(
            vec![fwd(e0), fwd(e1)],
            SnapPoint::on_edge(&original_graph, e0, 250),
            SnapPoint::on_edge(&original_graph, e1, length_e1),
        ),
        // 1本の辺の中だけ（部分辺の分割）。
        path(
            vec![fwd(e0)],
            SnapPoint::on_edge(&original_graph, e0, 200),
            SnapPoint::on_edge(&original_graph, e0, 800),
        ),
    ];
    let split_paths = [
        path(
            vec![fwd(first), fwd(second), fwd(split_e1)],
            SnapPoint::on_edge(&split_graph, first, 0),
            SnapPoint::on_edge(&split_graph, split_e1, split_length_e1),
        ),
        path(
            vec![fwd(first), fwd(second), fwd(split_e1)],
            SnapPoint::on_edge(&split_graph, first, 250),
            SnapPoint::on_edge(&split_graph, split_e1, split_length_e1),
        ),
        path(
            vec![fwd(first), fwd(second)],
            SnapPoint::on_edge(&split_graph, first, 200),
            SnapPoint::on_edge(&split_graph, second, 800 - p),
        ),
    ];

    let filters = [
        ConditionFilter::default(),
        ConditionFilter {
            weather: Some(Weather::Rain),
            ..ConditionFilter::default()
        },
    ];
    for filter in filters {
        for (case, (original_path, split_path)) in
            original_paths.iter().zip(split_paths.iter()).enumerate()
        {
            let mut original_view = risk_view(
                &original_model,
                &original_graph,
                &original_topo,
                &original_index,
                &original_snapshot,
                Mode::Car,
                filter,
                RiskVariant::EdgeSeverity,
            );
            let mut split_view = risk_view(
                &model,
                &split_graph,
                &split_topo,
                &split_index,
                &split_snapshot,
                Mode::Car,
                filter,
                RiskVariant::EdgeSeverity,
            );
            let original_risk = path_risk(&mut original_view, original_path);
            let split_risk = path_risk(&mut split_view, split_path);
            assert_relative_close(
                split_risk,
                original_risk,
                &format!("edge-severity の分割の不変性 filter={filter:?} case={case}"),
            );
            assert!(original_risk > 0.0);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// length・random の ArcCost と class_lengths
// ---------------------------------------------------------------------------------------------

fn all_edges_bits(edges: usize) -> Vec<u64> {
    (0..edges.div_ceil(64))
        .map(|w| {
            let in_word = (edges - w * 64).min(64);
            if in_word == 64 {
                u64::MAX
            } else {
                (1_u64 << in_word) - 1
            }
        })
        .collect()
}

fn empty_topology(graph: &Graph) -> Topology {
    Topology {
        params: serde_json::Value::Null,
        snap_car: all_edges_bits(graph.edge_count()),
        snap_bike: all_edges_bits(graph.edge_count()),
        snap_walk: all_edges_bits(graph.edge_count()),
        node_cluster: vec![NONE; graph.node_count()],
        clusters: vec![],
        cluster_nodes: vec![],
        twins: vec![],
        sidewalk_parents: vec![],
    }
}

struct World {
    graph: Graph,
    topology: Topology,
    automaton: TurnAutomaton,
    landmarks: Landmarks,
    mode: Mode,
}

impl World {
    fn new(builder: &GraphBuilder, mode: Mode) -> World {
        let (graph, restrictions) = builder.build_all();
        let topology = empty_topology(&graph);
        let automaton = TurnAutomaton::build(&restrictions, mode);
        World {
            graph,
            topology,
            automaton,
            landmarks: Landmarks::empty(),
            mode,
        }
    }

    fn ctx(&self, params: SearchParams) -> SearchContext<'_> {
        SearchContext {
            graph: &self.graph,
            topology: &self.topology,
            automaton: &self.automaton,
            landmarks: &self.landmarks,
            mode: self.mode,
            params,
        }
    }
}

fn unlimited_params() -> SearchParams {
    SearchParams {
        rcsp_budget_ms: u64::MAX / 4,
        larac_iterations: 10,
        ellipse_node_limit: u32::MAX,
        rcsp_label_limit: u32::MAX,
    }
}

fn snap_at(graph: &Graph, edge: EdgeId, offset_dm: u32) -> SnapPoint {
    SnapPoint::on_edge(graph, edge, offset_dm)
}

#[test]
fn length_cost_uses_the_travelled_length() {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.0, 139.0);
    let b = builder.add_node_deg(35.0, 139.001);
    let e = builder.add_edge(EdgeSpec::new(a, b).length_dm(1234));
    let graph = builder.build();
    let mut cost = LengthArcCost::new(&graph);
    assert_eq!(cost.risk_fx(fwd(e)), risk_to_fx(123.4));
    assert_eq!(cost.partial_risk_fx(fwd(e), 0, 500), risk_to_fx(50.0));
    assert_eq!(cost.partial_risk_fx(fwd(e), 500, 500), 0);
    assert_eq!(cost.cluster_entry_risk_fx(fwd(e)), 0);
    assert_eq!(cost.risk_fx(fwd(e)), cost.risk_fx(bwd(e)));
}

#[test]
fn random_cost_is_deterministic_for_the_same_seed_and_edge() {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.0, 139.0);
    let b = builder.add_node_deg(35.0, 139.001);
    let e = builder.add_edge(EdgeSpec::new(a, b).length_dm(1000));
    let graph = builder.build();
    let mut first = RandomArcCost::new(&graph, 42);
    let mut second = RandomArcCost::new(&graph, 42);
    let mut other = RandomArcCost::new(&graph, 43);
    let first_value = first.risk_fx(fwd(e));
    assert_eq!(first_value, second.risk_fx(fwd(e)));
    assert_eq!(first_value, first.partial_risk_fx(fwd(e), 0, 1000));
    assert_eq!(first_value, first.risk_fx(bwd(e)));
    assert_ne!(first_value, other.risk_fx(fwd(e)));
    // 長さ×0.5〜2.0 の範囲（100m なので 50〜200m の固定小数点）。
    assert!((risk_to_fx(50.0)..=risk_to_fx(200.0)).contains(&first_value));
    // 部分辺も同じ u(e) を使う（長さに比例する）。
    let partial = first.partial_risk_fx(fwd(e), 0, 500);
    assert!((partial as f64 - first_value as f64 / 2.0).abs() <= 1.0);
}

#[test]
fn length_objective_finds_the_shortest_distance_within_the_budget() {
    let mut builder = GraphBuilder::new();
    let s = builder.add_node_deg(35.0, 139.0);
    let t = builder.add_node_deg(35.0, 139.02);
    let a = builder.add_node_deg(35.001, 139.01);
    // 直進は速いが長い。迂回は遅いが短い。
    let direct = builder.add_edge(
        EdgeSpec::new(s, t)
            .length_dm(20_000)
            .class(HighwayClass::Motorway),
    );
    let short_a = builder.add_edge(
        EdgeSpec::new(s, a)
            .length_dm(5_000)
            .class(HighwayClass::Residential),
    );
    let short_b = builder.add_edge(
        EdgeSpec::new(a, t)
            .length_dm(5_000)
            .class(HighwayClass::Residential),
    );
    let mut time_model = TimeModel::synthetic();
    time_model.speed_kmh.car[HighwayClass::Motorway.code() as usize] = 60.0;
    time_model.speed_kmh.car[HighwayClass::Residential.code() as usize] = 15.0;
    builder.set_time_model(time_model);
    let world = World::new(&builder, Mode::Car);
    let g = &world.graph;
    let start = snap_at(g, direct, 0);
    let end = snap_at(g, direct, g.edge(direct).length_dm);
    let ctx = world.ctx(unlimited_params());
    let cancel = CancelToken::unbounded();
    let mut cost = LengthArcCost::new(g);
    let result = find_routes(&ctx, &mut cost, &start, &end, 10, &cancel).unwrap();
    let p0_distance =
        path_distance_dm(g, &result.p0.start, &result.p0.end, &result.p0.dedges).expect("p0 の形");
    let best_distance =
        path_distance_dm(g, &result.best.start, &result.best.end, &result.best.dedges)
            .expect("best の形");
    assert_eq!(result.p0.dedges, vec![fwd(direct)]);
    assert_eq!(
        result.best.dedges,
        vec![fwd(short_a), fwd(short_b)],
        "p0={:?} p0_time={} best={:?} best_time={} budget={}",
        result.p0.dedges,
        result.p0.time_ds,
        result.best.dedges,
        result.best.time_ds,
        result.budget_ds
    );
    assert!(best_distance < p0_distance);
    assert_eq!(best_distance, 10_000);
    assert!(result.best.time_ds <= result.budget_ds);
    // 候補にも最短距離の経路が入る。
    assert!(
        result
            .candidates
            .iter()
            .any(|p| p.dedges == vec![fwd(short_a), fwd(short_b)])
    );
}

#[test]
fn random_objective_is_reproducible_for_the_same_seed() {
    let mut builder = GraphBuilder::new();
    let s = builder.add_node_deg(35.0, 139.0);
    let t = builder.add_node_deg(35.0, 139.002);
    let a = builder.add_node_deg(35.001, 139.001);
    let b = builder.add_node_deg(34.999, 139.001);
    let direct = builder.add_edge(EdgeSpec::new(s, t).length_dm(2000));
    builder.add_edge(EdgeSpec::new(s, a).length_dm(1500));
    builder.add_edge(EdgeSpec::new(a, t).length_dm(1500));
    builder.add_edge(EdgeSpec::new(s, b).length_dm(1800));
    builder.add_edge(EdgeSpec::new(b, t).length_dm(1800));
    let world = World::new(&builder, Mode::Car);
    let g = &world.graph;
    let ctx = world.ctx(unlimited_params());
    let cancel = CancelToken::unbounded();
    let start = snap_at(g, direct, 0);
    let end = snap_at(g, direct, g.edge(direct).length_dm);
    let run = |seed: u64| {
        let mut cost = RandomArcCost::new(g, seed);
        find_routes(&ctx, &mut cost, &start, &end, 10, &cancel).unwrap()
    };
    let first = run(7);
    let second = run(7);
    assert_eq!(first.best, second.best);
    assert_eq!(first.candidates, second.candidates);
}

#[test]
fn class_lengths_sum_to_the_path_distance() {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.0, 139.0);
    let b = builder.add_node_deg(35.0, 139.001);
    let c = builder.add_node_deg(35.0, 139.002);
    let residential = builder.add_edge(
        EdgeSpec::new(a, b)
            .length_dm(1000)
            .class(HighwayClass::Residential),
    );
    let primary = builder.add_edge(
        EdgeSpec::new(b, c)
            .length_dm(500)
            .class(HighwayClass::Primary),
    );
    let graph = builder.build();
    let full = path(
        vec![fwd(residential), fwd(primary)],
        SnapPoint::on_edge(&graph, residential, 0),
        SnapPoint::on_edge(&graph, primary, 500),
    );
    let lengths = class_lengths_dm(&graph, &full);
    assert_eq!(
        lengths.get(&HighwayClass::Residential.code()).copied(),
        Some(1000)
    );
    assert_eq!(
        lengths.get(&HighwayClass::Primary.code()).copied(),
        Some(500)
    );
    let total: u64 = lengths.values().sum();
    assert_eq!(
        total,
        path_distance_dm(&graph, &full.start, &full.end, &full.dedges).unwrap()
    );

    // 出発・到着の部分辺は通った長さだけを数える。
    let partial = path(
        vec![fwd(residential), fwd(primary)],
        SnapPoint::on_edge(&graph, residential, 200),
        SnapPoint::on_edge(&graph, primary, 300),
    );
    let lengths = class_lengths_dm(&graph, &partial);
    assert_eq!(
        lengths.get(&HighwayClass::Residential.code()).copied(),
        Some(800)
    );
    assert_eq!(
        lengths.get(&HighwayClass::Primary.code()).copied(),
        Some(300)
    );
    let total: u64 = lengths.values().sum();
    assert_eq!(
        total,
        path_distance_dm(&graph, &partial.start, &partial.end, &partial.dedges).unwrap()
    );
    assert!(!lengths.contains_key(&EDGE_K_NO_RISK));
    // 形が正しくない経路は空。
    let broken = path(vec![fwd(primary)], partial.start, partial.end);
    assert!(class_lengths_dm(&graph, &broken).is_empty());
}

#[test]
fn expected_count_variant_parses_and_round_trips() {
    use accident_route_backend::domain::routing::risk::RiskVariant;
    assert_eq!(
        RiskVariant::parse("expected-count"),
        Some(RiskVariant::ExpectedCount)
    );
    assert_eq!(RiskVariant::ExpectedCount.as_str(), "expected-count");
}
