//! リクエストごとのリスク（形式書14.5節）の受け入れ試験（作業書M3b.2）。
//!
//! - 分割の不変性：辺を任意の位置で2つに分けたグラフで、同じ経路の `path_risk` が1e-9の
//!   相対誤差で一致する（behavior.md 11章、計画書5.1節）。
//! - 手計算との一致：辺・歩道・NO_RISK・クラスタ・条件の絞り込み・MIRROR。
//! - `RiskArcCost::cluster_entry_risk_fx` が外から入る有向辺でだけ正になること。
//! - 境界：`partial_edge_risk(e, 0, length_dm) == edge_risk(e)`。

use accident_route_backend::contract::{ConditionFilter, DayType, Mode, Period, Weather};
use accident_route_backend::domain::routing::NONE;
use accident_route_backend::domain::routing::accident::{
    ClusterAccident, EdgeAccident, EdgeAccidentIndex,
};
use accident_route_backend::domain::routing::graph::synthetic::{EdgeSpec, GraphBuilder, bwd, fwd};
use accident_route_backend::domain::routing::graph::time::PerMode;
use accident_route_backend::domain::routing::graph::{DirEdge, EdgeId, Graph};
use accident_route_backend::domain::routing::risk::arc_cost::{RiskArcCost, path_risk};
use accident_route_backend::domain::routing::risk::{
    CELL_COUNT, CoefRecord, EDGE_K_NO_RISK, EDGE_K_SIDEWALK, RiskModel, RiskParams, RiskView,
    TABLE_LEN, WeatherMultiplier, mode_index, road_class_k, table_index,
};
use accident_route_backend::domain::routing::search::{ArcCost, Path, risk_to_fx};
use accident_route_backend::domain::routing::snap::SnapPoint;
use accident_route_backend::domain::routing::topology::{ClusterRecord, SidewalkParent, Topology};
use accident_route_backend::domain::snapshot::{AccidentColumns, AccidentSnapshot};
use serde_json::json;

// ---------------------------------------------------------------------------------------------
// 組み立ての道具
// ---------------------------------------------------------------------------------------------

fn base_params() -> RiskParams {
    RiskParams {
        train_source_years: vec![2019, 2020, 2021, 2022],
        years: 2,
        c: 1.5,
        c_groups: vec![5, 6, 7, 8],
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

/// 1件の事故の条件と、辺・クラスタへの割り当て。
#[derive(Debug, Clone)]
struct AccidentSpec {
    modes: u8,
    period: u8,
    weather: u8,
    day_type: u8,
    source_year: u16,
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
            edge_targets: vec![(edge, pos_dm, weight, mirror)],
            cluster_targets: Vec::new(),
        }
    }

    fn on_cluster(
        modes: u8,
        period: u8,
        weather: u8,
        day_type: u8,
        source_year: u16,
        cluster: u32,
        weight: f32,
    ) -> Self {
        Self {
            modes,
            period,
            weather,
            day_type,
            source_year,
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
        columns.deaths.push(0);
        columns.injuries.push(0);
        columns.source_year.push(spec.source_year);
        columns.date.push(20200101);
    }
    AccidentSnapshot::new(columns)
}

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
    for entries in &per_cluster {
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

fn risk_view<'a>(
    model: &'a RiskModel,
    graph: &'a Graph,
    topology: &'a Topology,
    accidents: &'a EdgeAccidentIndex,
    snapshot: &'a AccidentSnapshot,
    mode: Mode,
    filter: ConditionFilter,
) -> RiskView<'a> {
    RiskView::new(model, graph, topology, accidents, snapshot, mode, filter)
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
// 分割の不変性
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

fn original_paths(graph: &Graph, e0: EdgeId, e1: EdgeId) -> Vec<Path> {
    let length_e1 = graph.edge(e1).length_dm;
    vec![
        // 全長（出発Aのノードから到着Cのノードまで）
        path(
            vec![fwd(e0), fwd(e1)],
            SnapPoint::on_edge(graph, e0, 0),
            SnapPoint::on_edge(graph, e1, length_e1),
        ),
        // 出発の辺の途中から
        path(
            vec![fwd(e0), fwd(e1)],
            SnapPoint::on_edge(graph, e0, 250),
            SnapPoint::on_edge(graph, e1, length_e1),
        ),
        // 到着の辺の途中まで
        path(
            vec![fwd(e0), fwd(e1)],
            SnapPoint::on_edge(graph, e0, 0),
            SnapPoint::on_edge(graph, e1, 300),
        ),
        // 1本の辺の中だけ
        path(
            vec![fwd(e0)],
            SnapPoint::on_edge(graph, e0, 200),
            SnapPoint::on_edge(graph, e0, 800),
        ),
    ]
}

fn split_paths(graph: &Graph, first: EdgeId, second: EdgeId, e1: EdgeId, p: u32) -> Vec<Path> {
    let length_e1 = graph.edge(e1).length_dm;
    vec![
        path(
            vec![fwd(first), fwd(second), fwd(e1)],
            SnapPoint::on_edge(graph, first, 0),
            SnapPoint::on_edge(graph, e1, length_e1),
        ),
        path(
            vec![fwd(first), fwd(second), fwd(e1)],
            SnapPoint::on_edge(graph, first, 250),
            SnapPoint::on_edge(graph, e1, length_e1),
        ),
        path(
            vec![fwd(first), fwd(second), fwd(e1)],
            SnapPoint::on_edge(graph, first, 0),
            SnapPoint::on_edge(graph, e1, 300),
        ),
        path(
            vec![fwd(first), fwd(second)],
            SnapPoint::on_edge(graph, first, 200),
            SnapPoint::on_edge(graph, second, 800 - p),
        ),
    ]
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
fn splitting_an_edge_preserves_path_risk() {
    let p = 400;
    let (original_graph, original_topo, e0, e1) = original_line();
    let (split_graph, split_topo, first, second, split_e1) = split_line(p);

    let k0 = road_class_k(6, 1, true);
    let k1 = road_class_k(2, 0, false);
    let j = 35;
    let mut model = empty_model(vec![k0, k0, k1], vec![j]);
    for mode in Mode::ALL {
        let mode_factor = mode_index(mode) as f64 + 1.0;
        fill_k(&mut model, mode, k0, |cell| {
            coef(
                1e-5 * (1.0 + cell as f64 * 0.5) * mode_factor,
                5.0 + cell as f64,
                0.2 + 0.1 * mode_factor,
                [0.1, 0.2, 0.3, 0.2, 0.2],
            )
        });
        fill_k(&mut model, mode, k1, |cell| {
            coef(
                8e-6 * (1.0 + cell as f64 * 0.25) * mode_factor,
                4.0 + cell as f64,
                0.3 + 0.1 * mode_factor,
                [0.2, 0.1, 0.3, 0.2, 0.2],
            )
        });
        fill_j(&mut model, mode, j, |cell| {
            coef(
                0.005 * (1.0 + cell as f64) * mode_factor,
                10.0 + cell as f64,
                0.4 + 0.1 * mode_factor,
                [0.2, 0.1, 0.3, 0.2, 0.2],
            )
        });
    }
    // 元のグラフの辺は e0・e1、分割したグラフは e0の2本・e1。係数表は共通で、EDGEKだけが違う。
    let split_model = model;
    let mut original_model = split_model.clone();
    original_model.edge_k = vec![k0, k1];

    // 条件・天候・手段をまたぐ事故。位置400は分割の境界に置く。
    let mut specs = Vec::new();
    for (i, pos) in [0_u32, 250, 400, 777, 1000].into_iter().enumerate() {
        specs.push(AccidentSpec::on_edge(
            [5_u8, 7, 3, 5, 1][i],
            [0_u8, 1, 3, 4, 0][i],
            [3_u8, 1, 3, 4, 3][i],
            [0_u8, 1, 0, 1, 0][i],
            [2020, 2021, 2019, 2022, 1990][i],
            e0,
            pos,
            [0.5, 0.4, 0.3, 0.6, 0.2][i],
            i == 3,
        ));
    }
    for (i, pos) in [0_u32, 300, 1200].into_iter().enumerate() {
        specs.push(AccidentSpec::on_edge(
            [4_u8, 6, 1][i],
            [0_u8, 4, 1][i],
            [3_u8, 5, 3][i],
            [0_u8, 1, 0][i],
            [2020, 2019, 2021][i],
            e1,
            pos,
            [0.7, 0.5, 0.9][i],
            false,
        ));
    }
    specs.push(AccidentSpec::on_cluster(4_u8, 0, 3, 0, 2020, 0, 0.8));
    specs.push(AccidentSpec::on_cluster(7_u8, 1, 2, 1, 2021, 0, 0.6));

    let snap = snapshot(&specs);
    let original_index = accident_index(&specs, original_graph.edge_count(), 1);
    let split_specs = split_specs(p, &specs, e0, first, second, e1, split_e1);
    let split_index = accident_index(&split_specs, split_graph.edge_count(), 1);

    let original_paths = original_paths(&original_graph, e0, e1);
    let split_paths = split_paths(&split_graph, first, second, split_e1, p);
    assert_eq!(original_paths.len(), split_paths.len());

    for mode in Mode::ALL {
        for weather in [
            None,
            Some(Weather::Clear),
            Some(Weather::Rain),
            Some(Weather::Snow),
        ] {
            for filter in [
                ConditionFilter {
                    period: None,
                    day_type: None,
                    weather,
                },
                ConditionFilter {
                    period: Some(Period::Daytime),
                    day_type: Some(DayType::Holiday),
                    weather,
                },
                ConditionFilter {
                    period: Some(Period::EarlyMorning),
                    day_type: Some(DayType::Weekday),
                    weather,
                },
            ] {
                for (case, (original_path, split_path)) in
                    original_paths.iter().zip(split_paths.iter()).enumerate()
                {
                    let mut original_view = risk_view(
                        &original_model,
                        &original_graph,
                        &original_topo,
                        &original_index,
                        &snap,
                        mode,
                        filter,
                    );
                    let mut split_view = risk_view(
                        &split_model,
                        &split_graph,
                        &split_topo,
                        &split_index,
                        &snap,
                        mode,
                        filter,
                    );
                    let original_risk = path_risk(&mut original_view, original_path);
                    let split_risk = path_risk(&mut split_view, split_path);
                    assert_relative_close(
                        split_risk,
                        original_risk,
                        &format!(
                            "分割の不変性 mode={mode:?} weather={weather:?} filter={filter:?} case={case}"
                        ),
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 手計算との一致
// ---------------------------------------------------------------------------------------------

/// 手計算用のグラフと位相。e0（A→B、1000dm）、e1（B→C、500dm、歩道）、
/// e2（C→D、300dm、NO_RISK）、e3（D→A、400dm）。
fn hand_graph() -> (Graph, Topology, [EdgeId; 4]) {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.0, 139.0);
    let b = builder.add_node_deg(35.0, 139.001);
    let c = builder.add_node_deg(35.0, 139.002);
    let d = builder.add_node_deg(35.0, 139.003);
    let e0 = builder.add_edge(EdgeSpec::new(a, b).length_dm(1000));
    let e1 = builder.add_edge(EdgeSpec::new(b, c).length_dm(500));
    let e2 = builder.add_edge(EdgeSpec::new(c, d).length_dm(300));
    let e3 = builder.add_edge(EdgeSpec::new(d, a).length_dm(400));
    let graph = builder.build();
    let topo = topology(
        vec![NONE, 0, NONE, NONE],
        vec![cluster_record(0, 1)],
        vec![b],
        vec![SidewalkParent {
            sidewalk: e1,
            parent: e0,
        }],
        graph.edge_count(),
    );
    (graph, topo, [e0, e1, e2, e3])
}

fn hand_model() -> RiskModel {
    let k_residential = road_class_k(6, 1, true);
    let k_primary = road_class_k(2, 0, false);
    let j = 35;
    let mut model = empty_model(
        vec![k_residential, EDGE_K_SIDEWALK, EDGE_K_NO_RISK, k_primary],
        vec![j],
    );
    let rho = [0.1_f32, 0.2, 0.25, 0.15, 0.3];
    model.k_table[table_index(Mode::Car, k_residential, 0)] = coef(1e-5, 10.0, 0.5, rho);
    model.k_table[table_index(Mode::Car, k_primary, 0)] = coef(1e-5, 10.0, 0.5, rho);
    model.j_table[table_index(Mode::Car, j, 0)] = coef(0.01, 20.0, 0.2, [0.2, 0.1, 0.5, 0.1, 0.1]);
    model
}

fn hand_specs(edges: [EdgeId; 4]) -> Vec<AccidentSpec> {
    let e0 = edges[0];
    let e1 = edges[1];
    vec![
        // 数える（MIRROR でない）
        AccidentSpec::on_edge(4, 0, 3, 0, 2020, e0, 500, 0.6, false),
        AccidentSpec::on_edge(4, 0, 1, 0, 2021, e0, 100, 0.4, false),
        // source_year が学習年に入らない
        AccidentSpec::on_edge(4, 0, 3, 0, 1999, e0, 200, 0.5, false),
        // 手段が違う
        AccidentSpec::on_edge(1, 0, 3, 0, 2020, e0, 300, 0.3, false),
        // MIRROR も数える
        AccidentSpec::on_edge(4, 0, 3, 0, 2020, e0, 400, 0.2, true),
        // セルが違う（休日）
        AccidentSpec::on_edge(4, 0, 3, 1, 2020, e0, 600, 0.9, false),
        // 歩道の事故は歩道の係数ではなく親の密度を使う
        AccidentSpec::on_edge(4, 0, 3, 0, 2020, e1, 100, 0.7, false),
        // n_e には入るが、雨の n^w_e には入らない
        AccidentSpec::on_edge(4, 0, 4, 0, 2020, e0, 700, 0.8, false),
        // クラスタ
        AccidentSpec::on_cluster(4, 0, 3, 0, 2020, 0, 0.7),
        AccidentSpec::on_cluster(4, 0, 1, 0, 2021, 0, 0.5),
        AccidentSpec::on_cluster(4, 0, 3, 0, 1999, 0, 0.9),
        AccidentSpec::on_cluster(2, 0, 3, 0, 2020, 0, 0.4),
    ]
}

#[test]
fn edge_and_cluster_risks_match_hand_calculations() {
    let (graph, topo, edges) = hand_graph();
    let specs = hand_specs(edges);
    let snap = snapshot(&specs);
    let index = accident_index(&specs, graph.edge_count(), 1);
    let model = hand_model();
    let filter = ConditionFilter {
        period: Some(Period::EarlyMorning),
        day_type: Some(DayType::Weekday),
        weather: Some(Weather::Rain),
    };
    let mut view = risk_view(&model, &graph, &topo, &index, &snap, Mode::Car, filter);

    // 辺e0：R=1e-5、ℓ=100m、T=2、a=0.5、n_e=0.6+0.4+0.2+0.8=2.0、n^w_e=0.6+0.2=0.8、ρ=0.25。
    let r = 1e-5_f64;
    let length_m = 100.0_f64;
    let t = 2.0_f64;
    let a = 0.5_f64;
    let n_e = f64::from(0.6_f32) + f64::from(0.4_f32) + f64::from(0.2_f32) + f64::from(0.8_f32);
    let n_weather = f64::from(0.6_f32) + f64::from(0.2_f32);
    let mu = t * length_m * r;
    let w = t * r / (t * r + a);
    let y = (1.0 - w) * mu + w * n_e;
    let rho = 0.25_f64;
    let beta = model.params.beta.car;
    let w_weather = t * r * rho / (t * r * rho + beta);
    let y_weather = (1.0 - w_weather) * (mu * rho) + w_weather * n_weather;
    let y_total = y + (model.params.weather_multiplier.rain - 1.0) * y_weather;
    let expected_residential = model.params.c * 10.0 * y_total;
    let residential = view.edge_risk(edges[0]);
    assert_relative_close(residential, expected_residential, "住宅道路の辺");

    // 生活道路でない辺（primary）には c を掛けない。事故がないので n_e=n^w_e=0。
    let mu_primary = t * 40.0 * r;
    let y_primary = (1.0 - w) * mu_primary;
    let y_primary_weather = (1.0 - w_weather) * (mu_primary * rho);
    let expected_primary =
        10.0 * (y_primary + (model.params.weather_multiplier.rain - 1.0) * y_primary_weather);
    assert_relative_close(
        view.edge_risk(edges[3]),
        expected_primary,
        "primaryの辺（cなし）",
    );

    // 歩道は親の車道の密度 × 歩道の長さ（100m→50m）。歩道自身の事故は使わない。
    let expected_sidewalk = residential / 100.0 * 50.0;
    assert_relative_close(view.edge_risk(edges[1]), expected_sidewalk, "歩道");
    assert_relative_close(
        view.partial_edge_risk(edges[1], 0, 250),
        residential / 100.0 * 25.0,
        "歩道の部分",
    );

    // NO_RISK は0。
    assert_eq!(view.edge_risk(edges[2]), 0.0);
    assert_eq!(view.partial_edge_risk(edges[2], 0, 100), 0.0);

    // 境界：全長の部分は全長のリスクと一致し、分割しても合計が変わらない。
    assert_eq!(
        view.partial_edge_risk(edges[0], 0, graph.edge(edges[0]).length_dm),
        residential
    );
    let first = view.partial_edge_risk(edges[0], 0, 400);
    let second = view.partial_edge_risk(edges[0], 400, graph.edge(edges[0]).length_dm);
    assert_relative_close(first + second, residential, "部分の和");

    // クラスタ：μ=T·R=0.02、n=0.7+0.5=1.2、n^w=0.7、ρ=0.5。
    let cluster_r = 0.01_f64;
    let cluster_mu = t * cluster_r;
    let cluster_a = 0.2_f64;
    let cluster_n = f64::from(0.7_f32) + f64::from(0.5_f32);
    let cluster_n_weather = f64::from(0.7_f32);
    let cluster_w = cluster_mu / (cluster_mu + cluster_a);
    let cluster_y = (1.0 - cluster_w) * cluster_mu + cluster_w * cluster_n;
    let cluster_rho = 0.5_f64;
    let cluster_beta = model.params.beta_cluster.car;
    let cluster_w_weather = cluster_mu * cluster_rho / (cluster_mu * cluster_rho + cluster_beta);
    let cluster_y_weather = (1.0 - cluster_w_weather) * (cluster_mu * cluster_rho)
        + cluster_w_weather * cluster_n_weather;
    let expected_cluster =
        20.0 * (cluster_y + (model.params.weather_multiplier.rain - 1.0) * cluster_y_weather);
    assert_relative_close(view.cluster_risk(0), expected_cluster, "クラスタ");

    // 天候 all なら天候の項は1。
    let no_weather = ConditionFilter {
        period: Some(Period::EarlyMorning),
        day_type: Some(DayType::Weekday),
        weather: None,
    };
    let mut view = risk_view(&model, &graph, &topo, &index, &snap, Mode::Car, no_weather);
    assert_relative_close(
        view.cluster_risk(0),
        20.0 * cluster_y,
        "クラスタ（天候の項なし）",
    );
}

#[test]
fn cluster_entry_risk_is_positive_only_for_entering_dedges() {
    let (graph, topo, edges) = hand_graph();
    let specs = hand_specs(edges);
    let snap = snapshot(&specs);
    let index = accident_index(&specs, graph.edge_count(), 1);
    let model = hand_model();
    let filter = ConditionFilter {
        period: Some(Period::EarlyMorning),
        day_type: Some(DayType::Weekday),
        weather: Some(Weather::Rain),
    };
    let expected = {
        let mut view = risk_view(&model, &graph, &topo, &index, &snap, Mode::Car, filter);
        risk_to_fx(view.cluster_risk(0))
    };
    let mut view = risk_view(&model, &graph, &topo, &index, &snap, Mode::Car, filter);
    let mut cost = RiskArcCost::new(&mut view);
    // A→B と C→B は外からクラスタへ入る。
    assert_eq!(cost.cluster_entry_risk_fx(fwd(edges[0])), expected);
    assert_eq!(cost.cluster_entry_risk_fx(bwd(edges[1])), expected);
    assert!(expected > 0);
    // B→A、B→C、クラスタの外どうしは0。
    assert_eq!(cost.cluster_entry_risk_fx(bwd(edges[0])), 0);
    assert_eq!(cost.cluster_entry_risk_fx(fwd(edges[1])), 0);
    assert_eq!(cost.cluster_entry_risk_fx(fwd(edges[2])), 0);
}
