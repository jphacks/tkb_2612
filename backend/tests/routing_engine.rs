//! 新方式のエンジン（`RoutingEngine::evaluate`）の受け入れ試験（M4.3）。
//!
//! 合成グラフ・合成の割り当て・リスクモデルから `RoutingData` を組み立て、
//! behavior.md 5〜8章の形状・集合B・高リスク区間・選定・文面を確かめる。

use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use accident_route_backend::contract::geo::METERS_PER_DEGREE;
use accident_route_backend::contract::{
    Attribution, BoxFuture, CancelToken, CandidateSet, ConditionFilter, DatasetInfo, DayType,
    EngineError, Interrupt, LatLon, Maneuver, Mode, Period, ProviderRoute, RouteEngine, RouteQuery,
    Router, RoutingError, RoutingRequest, Weather,
};
use accident_route_backend::domain::routing::accident::{
    AssignTarget, Assignment, UnassignedReason, UnassignedRecord,
};
use accident_route_backend::domain::routing::evaluate::routing_risk_note;
use accident_route_backend::domain::routing::graph::synthetic::{EdgeSpec, GraphBuilder, grid};
use accident_route_backend::domain::routing::graph::{Coord, EdgeId, Graph, NodeId, access};
use accident_route_backend::domain::routing::prep::odindex::build_odindex;
use accident_route_backend::domain::routing::risk::{
    CELL_COUNT, CoefRecord, EDGE_K_NO_RISK, RiskModel, RiskParams, TABLE_LEN, WeatherMultiplier,
    table_index,
};
use accident_route_backend::domain::routing::search::alt::Landmarks;
use accident_route_backend::domain::routing::snap::point_on_edge;
use accident_route_backend::domain::routing::topology::{ClusterRecord, SidewalkParent, Topology};
use accident_route_backend::domain::routing::{
    EdgeAccidentIndex, NONE, RoutingData, RoutingEngine, UnassignedGrid,
};
use accident_route_backend::domain::snapshot::{AccidentColumns, AccidentSnapshot};
use accident_route_backend::domain::{candidates, engine as old_engine};
use accident_route_backend::testing::fixtures::check_outcome_invariants;
use serde_json::json;

// ---------------------------------------------------------------------------------------------
// 組み立ての道具
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

fn topology(graph: &Graph, clusters: &[Vec<NodeId>], parents: &[(EdgeId, EdgeId)]) -> Topology {
    let mut node_cluster = vec![NONE; graph.node_count()];
    let mut cluster_nodes = Vec::new();
    let mut records = Vec::new();
    for (i, nodes) in clusters.iter().enumerate() {
        let start = cluster_nodes.len() as u32;
        for &node in nodes {
            node_cluster[node as usize] = i as u32;
        }
        cluster_nodes.extend_from_slice(nodes);
        let lat = nodes
            .iter()
            .map(|&n| f64::from(graph.node_coord(n).lat_e7))
            .sum::<f64>()
            / nodes.len() as f64;
        let lon = nodes
            .iter()
            .map(|&n| f64::from(graph.node_coord(n).lon_e7))
            .sum::<f64>()
            / nodes.len() as f64;
        records.push(ClusterRecord {
            lat_e7: lat.round() as i32,
            lon_e7: lon.round() as i32,
            node_start: start,
            node_count: nodes.len() as u16,
            top_class: 4,
            branches: 3,
            flags: 0,
            pad: [0; 3],
        });
    }
    Topology {
        params: json!({}),
        snap_car: all_edges_bits(graph.edge_count()),
        snap_bike: all_edges_bits(graph.edge_count()),
        snap_walk: all_edges_bits(graph.edge_count()),
        node_cluster,
        clusters: records,
        cluster_nodes,
        twins: Vec::new(),
        sidewalk_parents: parents
            .iter()
            .map(|&(sidewalk, parent)| SidewalkParent { sidewalk, parent })
            .collect(),
    }
}

fn empty_topology(graph: &Graph) -> Topology {
    topology(graph, &[], &[])
}

fn base_params() -> RiskParams {
    RiskParams {
        train_source_years: vec![2019, 2020],
        years: 1,
        c: 1.0,
        c_groups: vec![5, 6, 7, 8],
        car_exposure: None,
        beta: accident_route_backend::domain::routing::graph::time::PerMode {
            walk: 0.2,
            bike: 0.3,
            car: 0.4,
        },
        beta_cluster: accident_route_backend::domain::routing::graph::time::PerMode {
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

/// すべてのリスクが0のモデル。
fn zero_risk(graph: &Graph, topology: &Topology) -> RiskModel {
    RiskModel {
        params: base_params(),
        raw_params: json!({}),
        edge_k: vec![0; graph.edge_count()],
        cluster_j: vec![0; topology.cluster_count()],
        k_table: vec![CoefRecord::default(); TABLE_LEN],
        j_table: vec![CoefRecord::default(); TABLE_LEN],
    }
}

#[derive(Debug, Clone, Default)]
struct Accident {
    lat: f64,
    lon: f64,
    modes: u8,
    period: u8,
    weather: u8,
    day_type: u8,
    deaths: u16,
    injuries: u16,
    source_year: u16,
}

impl Accident {
    fn at(lat: f64, lon: f64) -> Self {
        Self {
            lat,
            lon,
            modes: Mode::Car.mask(),
            period: Period::Daytime.code(),
            weather: Weather::Clear.code(),
            day_type: DayType::Weekday.code(),
            source_year: 2020,
            ..Self::default()
        }
    }

    fn injuries(mut self, injuries: u16) -> Self {
        self.injuries = injuries;
        self
    }
}

fn snapshot(accidents: &[Accident]) -> AccidentSnapshot {
    let mut columns = AccidentColumns::default();
    for accident in accidents {
        columns.lat.push(accident.lat);
        columns.lon.push(accident.lon);
        columns.modes.push(accident.modes);
        columns.period.push(accident.period);
        columns.weather.push(accident.weather);
        columns.day_type.push(accident.day_type);
        columns.deaths.push(accident.deaths);
        columns.injuries.push(accident.injuries);
        columns.source_year.push(accident.source_year);
        columns.date.push(20200101);
    }
    AccidentSnapshot::new(columns)
}

/// 1件の事故の割り当て。両方空なら帰属不明にする。
#[derive(Debug, Clone, Default)]
struct Spec {
    edge: Vec<(u32, u32, f32, bool)>,
    cluster: Vec<(u32, f32)>,
}

impl Spec {
    fn edge(edge: u32, pos_dm: u32, weight: f32) -> Self {
        Self {
            edge: vec![(edge, pos_dm, weight, false)],
            cluster: Vec::new(),
        }
    }

    fn cluster(cluster: u32, weight: f32) -> Self {
        Self {
            edge: Vec::new(),
            cluster: vec![(cluster, weight)],
        }
    }

    fn edge_and_cluster(edge: u32, pos_dm: u32, cluster: u32, weight: f32) -> Self {
        Self {
            edge: vec![(edge, pos_dm, weight, false)],
            cluster: vec![(cluster, weight)],
        }
    }
}

fn assignment(specs: &[Spec]) -> Assignment {
    let mut offsets = vec![0_u32];
    let mut targets = Vec::new();
    let mut unassigned = Vec::new();
    for (accident, spec) in specs.iter().enumerate() {
        if spec.edge.is_empty() && spec.cluster.is_empty() {
            unassigned.push(UnassignedRecord {
                accident: accident as u32,
                reason: UnassignedReason::Other,
            });
        } else {
            let mut current: Vec<AssignTarget> = spec
                .edge
                .iter()
                .map(|&(target, pos_dm, weight, mirror)| AssignTarget {
                    target,
                    pos_dm,
                    weight,
                    kind: 0,
                    flags: if mirror { 1 } else { 0 },
                    reserved: 0,
                })
                .chain(spec.cluster.iter().map(|&(target, weight)| AssignTarget {
                    target,
                    pos_dm: u32::MAX,
                    weight,
                    kind: 1,
                    flags: 0,
                    reserved: 0,
                }))
                .collect();
            current.sort_by_key(|t| (t.kind, t.target, t.pos_dm));
            targets.extend(current);
        }
        offsets.push(targets.len() as u32);
    }
    Assignment {
        params: json!({}),
        offsets,
        targets,
        unassigned,
    }
}

fn dataset() -> DatasetInfo {
    DatasetInfo {
        release_id: "r".repeat(64),
        dataset_id: "d".repeat(64),
        accidents: 0,
        data_years: vec![2020],
        load_seconds: 0.0,
        grid_cells: None,
    }
}

/// 指定した辺だけリスクを持つモデル（事故0件でも推定リスクが正になる）。
fn model_with_edge_risk(graph: &Graph, topology: &Topology, edge_k: Vec<u8>) -> RiskModel {
    assert_eq!(edge_k.len(), graph.edge_count());
    let mut model = zero_risk(graph, topology);
    model.edge_k = edge_k;
    for cell in 0..CELL_COUNT {
        model.k_table[table_index(Mode::Car, 1, cell)] = CoefRecord {
            r: 1e-4,
            s: 2.0,
            a: 1.0,
            rho: [0.2; 5],
            pad: 0.0,
        };
    }
    model
}

fn engine(
    builder: &GraphBuilder,
    topology: Topology,
    specs: &[Spec],
    snapshot: AccidentSnapshot,
) -> RoutingEngine {
    let graph = builder.build();
    let risk = zero_risk(&graph, &topology);
    engine_with_risk(builder, topology, specs, snapshot, risk)
}

fn engine_with_risk(
    builder: &GraphBuilder,
    topology: Topology,
    specs: &[Spec],
    snapshot: AccidentSnapshot,
    risk: RiskModel,
) -> RoutingEngine {
    let parts = builder.parts();
    let (graph, restrictions) = builder.build_all();
    let assignment = assignment(specs);
    assignment
        .validate(graph.edge_count(), topology.cluster_count())
        .unwrap_or_else(|e| panic!("合成の割り当てが不正です: {e}"));
    let accident_index =
        EdgeAccidentIndex::build(&assignment, graph.edge_count(), topology.cluster_count());
    let unassigned = UnassignedGrid::build(&assignment, &snapshot);
    let odindex = build_odindex(&parts);
    let data = RoutingData {
        graph,
        restrictions,
        topology,
        odindex,
        accidents: accident_index,
        unassigned,
        risk,
        landmarks: Landmarks::empty(),
    };
    RoutingEngine::new(Arc::new(data), Arc::new(snapshot), dataset())
}

fn query(
    origin: LatLon,
    dest: LatLon,
    mode: Mode,
    max_detour_min: f64,
    conditions: ConditionFilter,
) -> RouteQuery {
    RouteQuery::new(origin, dest, mode, max_detour_min, conditions).expect("入力は妥当")
}

fn evaluate(
    engine: &RoutingEngine,
    query: &RouteQuery,
) -> Result<accident_route_backend::contract::RouteOutcome, EngineError> {
    engine.evaluate(
        query,
        CandidateSet {
            routes: vec![],
            warnings: vec![],
        },
        &CancelToken::unbounded(),
    )
}

fn north(lat: f64, meters: f64) -> f64 {
    lat + meters / METERS_PER_DEGREE
}

/// 北緯35度・東経139度を基準に、東へ `east_m`、北へ `north_m` 移動したノード。
fn node_offset(builder: &mut GraphBuilder, east_m: f64, north_m: f64) -> NodeId {
    let lat = 35.0 + north_m / METERS_PER_DEGREE;
    let lon = 139.0 + east_m / (METERS_PER_DEGREE * 35.0_f64.to_radians().cos());
    builder.add_node_deg(lat, lon)
}

fn assert_no_duplicate_vertices(outcome: &accident_route_backend::contract::RouteOutcome) {
    for candidate in &outcome.candidates {
        assert!(candidate.path.coordinates.len() >= 2);
        for pair in candidate.path.coordinates.windows(2) {
            assert_ne!(pair[0], pair[1], "連続する重複頂点がある");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 正常な検索
// ---------------------------------------------------------------------------------------------

#[test]
fn normal_search_passes_invariants_with_fractional_detour() {
    let (builder, shape) = grid(4, 4, 120.0);
    let graph = builder.build();
    let engine = engine(&builder, empty_topology(&graph), &[], snapshot(&[]));
    let origin = graph.node_coord(shape.node(0, 0)).to_latlon();
    let dest = graph.node_coord(shape.node(3, 3)).to_latlon();
    let q = query(origin, dest, Mode::Car, 7.5, ConditionFilter::default());
    let outcome = evaluate(&engine, &q).expect("評価できる");
    check_outcome_invariants(&outcome).expect("相関制約");
    assert!(!outcome.candidates.is_empty());
    let fastest = &outcome.candidates[outcome.fastest];
    for candidate in &outcome.candidates {
        assert!(candidate.within_detour);
        assert!(
            candidate.path.duration_s <= fastest.path.duration_s + 7.5 * 60.0 + 1e-6,
            "候補の所要時間が上限を超えた"
        );
        assert!(candidate.path.distance_m <= 80_000.0);
    }
    assert_no_duplicate_vertices(&outcome);
    assert_eq!(outcome.mode, Mode::Car);
    assert_eq!(outcome.dataset_id, dataset().dataset_id);
}

#[test]
fn synthetic_crossroad_guidance_is_right_then_arrive() {
    let mut builder = GraphBuilder::new();
    let w = node_offset(&mut builder, -100.0, 0.0);
    let c = node_offset(&mut builder, 0.0, 0.0);
    let s = node_offset(&mut builder, 0.0, -100.0);
    let n = node_offset(&mut builder, 0.0, 100.0);
    let e = node_offset(&mut builder, 100.0, 0.0);
    builder.add_edge(EdgeSpec::new(w, c));
    builder.add_edge(EdgeSpec::new(c, s));
    builder.add_edge(EdgeSpec::new(c, n));
    builder.add_edge(EdgeSpec::new(c, e));
    let graph = builder.build();
    let engine = engine(&builder, empty_topology(&graph), &[], snapshot(&[]));
    let q = query(
        graph.node_coord(w).to_latlon(),
        graph.node_coord(s).to_latlon(),
        Mode::Car,
        10.0,
        ConditionFilter::default(),
    );
    let outcome = evaluate(&engine, &q).expect("評価できる");
    check_outcome_invariants(&outcome).expect("相関制約");
    let selected = &outcome.candidates[outcome.fastest];
    let nav = selected.navigation.as_ref().expect("選定候補");
    let maneuvers: Vec<Maneuver> = nav.guidance.iter().map(|step| step.maneuver).collect();
    assert_eq!(maneuvers, vec![Maneuver::Right, Maneuver::Arrive]);
    assert_eq!(nav.guidance.last().expect("案内あり").at_m, nav.length_m);
}

struct PanicRouter;

impl Router for PanicRouter {
    fn route<'a>(
        &'a self,
        _request: &'a RoutingRequest,
        _cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<Vec<ProviderRoute>, RoutingError>> {
        panic!("新方式の collect_candidates はルーターを呼んではいけません")
    }
}

#[tokio::test]
async fn collect_candidates_is_empty_and_does_not_call_router() {
    let (builder, shape) = grid(3, 3, 120.0);
    let graph = builder.build();
    let engine = engine(&builder, empty_topology(&graph), &[], snapshot(&[]));
    let origin = graph.node_coord(shape.node(0, 0)).to_latlon();
    let dest = graph.node_coord(shape.node(2, 2)).to_latlon();
    let q = query(origin, dest, Mode::Car, 10.0, ConditionFilter::default());
    let candidates = engine
        .collect_candidates(&PanicRouter, &q, &CancelToken::unbounded())
        .await
        .expect("候補取得");
    assert!(candidates.routes.is_empty());
    assert!(candidates.warnings.is_empty());
}

// ---------------------------------------------------------------------------------------------
// 集合B（behavior.md 6章）
// ---------------------------------------------------------------------------------------------

/// A→B→C の直線。各辺は100m。
fn line_abc() -> (GraphBuilder, NodeId, NodeId, NodeId, EdgeId, EdgeId) {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.6, 139.7);
    let b = builder.add_node_deg(35.6, 139.701);
    let c = builder.add_node_deg(35.6, 139.702);
    let e0 = builder.add_edge(EdgeSpec::new(a, b).length_dm(1000));
    let e1 = builder.add_edge(EdgeSpec::new(b, c).length_dm(1000));
    (builder, a, b, c, e0, e1)
}

#[test]
fn edge_positions_include_both_ends_of_the_travelled_range() {
    let (builder, a, _b, _c, e0, e1) = line_abc();
    let graph = builder.build();
    let specs = vec![
        Spec::edge(e0, 250, 0.5),
        Spec::edge(e0, 249, 0.5),
        Spec::edge(e0, 1000, 0.5),
        Spec::edge(e1, 0, 0.5),
        Spec::edge(e1, 750, 0.5),
        Spec::edge(e1, 751, 0.5),
    ];
    let snapshot = snapshot(&vec![Accident::at(35.6, 139.7); specs.len()]);
    let engine = engine(&builder, empty_topology(&graph), &specs, snapshot);
    let g = &engine.data().graph;
    let origin = point_on_edge(g, e0, 250).to_latlon();
    let dest = point_on_edge(g, e1, 750).to_latlon();
    let _ = a;
    let q = query(origin, dest, Mode::Car, 10.0, ConditionFilter::default());
    let outcome = evaluate(&engine, &q).expect("評価できる");
    let ids: Vec<u64> = outcome.candidates[0]
        .accident_points
        .iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(ids, vec![0, 2, 3, 4]);
    assert_eq!(outcome.candidates[0].totals.accident_count, 4);
}

#[test]
fn internal_node_cluster_counts_but_start_and_end_clusters_do_not() {
    let (builder, a, b, c, _e0, _e1) = line_abc();
    let graph = builder.build();
    let topology = topology(&graph, &[vec![a], vec![b], vec![c]], &[]);
    let specs = vec![
        Spec::cluster(0, 1.0),
        Spec::cluster(1, 1.0),
        Spec::cluster(2, 1.0),
    ];
    let snapshot = snapshot(&vec![Accident::at(35.6, 139.7); specs.len()]);
    let engine = engine(&builder, topology, &specs, snapshot);
    let q = query(
        graph.node_coord(a).to_latlon(),
        graph.node_coord(c).to_latlon(),
        Mode::Car,
        10.0,
        ConditionFilter::default(),
    );
    let outcome = evaluate(&engine, &q).expect("評価できる");
    let ids: Vec<u64> = outcome.candidates[0]
        .accident_points
        .iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(ids, vec![1]);
    assert_eq!(outcome.candidates[0].totals.accident_count, 1);
}

#[test]
fn sidewalk_parent_accidents_count_only_within_72m() {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.6, 139.7);
    let b = builder.add_node_deg(35.6, 139.701);
    let walk = builder.add_edge(
        EdgeSpec::new(a, b)
            .length_dm(1000)
            .access(access::WALK_FWD | access::WALK_BWD),
    );
    let parent = builder.add_edge(
        EdgeSpec::new(a, b)
            .length_dm(1000)
            .access(access::CAR_FWD | access::CAR_BWD),
    );
    let graph = builder.build();
    let topology = topology(&graph, &[], &[(walk, parent)]);
    let mid_lon = (graph.node_coord(a).to_latlon().lon + graph.node_coord(b).to_latlon().lon) / 2.0;
    let specs = vec![Spec::edge(parent, 500, 1.0), Spec::edge(parent, 500, 1.0)];
    let mut near = Accident::at(north(35.6, 10.0), mid_lon);
    near.modes = Mode::Walk.mask();
    let mut far = Accident::at(north(35.6, 80.0), mid_lon);
    far.modes = Mode::Walk.mask();
    let snapshot = snapshot(&[near, far]);
    let specs: Vec<Spec> = specs
        .into_iter()
        .enumerate()
        .map(|(i, spec)| {
            let mut spec = spec;
            spec.edge[0].0 = parent;
            spec.edge[0].1 = 500;
            let _ = i;
            spec
        })
        .collect();
    let engine = engine(&builder, topology, &specs, snapshot);
    let q = query(
        graph.node_coord(a).to_latlon(),
        graph.node_coord(b).to_latlon(),
        Mode::Walk,
        10.0,
        ConditionFilter::default(),
    );
    let outcome = evaluate(&engine, &q).expect("評価できる");
    let ids: Vec<u64> = outcome.candidates[0]
        .accident_points
        .iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(ids, vec![0]);
}

#[test]
fn unassigned_accidents_stop_at_the_72m_boundary() {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.6, 139.7);
    let b = builder.add_node_deg(35.6, 139.701);
    let e0 = builder.add_edge(EdgeSpec::new(a, b).length_dm(1000));
    let graph = builder.build();
    let mid_lon = (graph.node_coord(a).to_latlon().lon + graph.node_coord(b).to_latlon().lon) / 2.0;
    let specs = vec![Spec::default(), Spec::default()];
    let snapshot = snapshot(&[
        Accident::at(north(35.6, 71.9), mid_lon),
        Accident::at(north(35.6, 72.1), mid_lon),
    ]);
    let engine = engine(&builder, empty_topology(&graph), &specs, snapshot);
    let q = query(
        graph.node_coord(a).to_latlon(),
        graph.node_coord(b).to_latlon(),
        Mode::Car,
        10.0,
        ConditionFilter::default(),
    );
    let outcome = evaluate(&engine, &q).expect("評価できる");
    let ids: Vec<u64> = outcome.candidates[0]
        .accident_points
        .iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(ids, vec![0]);
    let _ = e0;
}

#[test]
fn attribution_reflects_how_each_accident_was_linked() {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.6, 139.7);
    let b = builder.add_node_deg(35.6, 139.701);
    let c = builder.add_node_deg(35.6, 139.702);
    let walk0 = builder.add_edge(
        EdgeSpec::new(a, b)
            .length_dm(1000)
            .access(access::WALK_FWD | access::WALK_BWD),
    );
    let walk1 = builder.add_edge(
        EdgeSpec::new(b, c)
            .length_dm(1000)
            .access(access::WALK_FWD | access::WALK_BWD),
    );
    let parent0 = builder.add_edge(
        EdgeSpec::new(a, b)
            .length_dm(1000)
            .access(access::CAR_FWD | access::CAR_BWD),
    );
    let parent1 = builder.add_edge(
        EdgeSpec::new(b, c)
            .length_dm(1000)
            .access(access::CAR_FWD | access::CAR_BWD),
    );
    let graph = builder.build();
    let topology = topology(&graph, &[vec![b]], &[(walk0, parent0), (walk1, parent1)]);
    let mid_lon = (graph.node_coord(a).to_latlon().lon + graph.node_coord(b).to_latlon().lon) / 2.0;
    let specs = vec![
        Spec::edge(walk0, 500, 1.0),
        Spec::cluster(0, 1.0),
        Spec::edge(parent0, 500, 1.0),
        Spec::default(),
    ];
    let mut accidents = vec![
        Accident::at(35.6, 139.7),
        Accident::at(35.6, 139.7),
        Accident::at(35.6, mid_lon),
        Accident::at(north(35.6, 10.0), mid_lon),
    ];
    for accident in &mut accidents {
        accident.modes = Mode::Walk.mask();
    }
    let engine = engine(&builder, topology, &specs, snapshot(&accidents));
    let q = query(
        graph.node_coord(a).to_latlon(),
        graph.node_coord(c).to_latlon(),
        Mode::Walk,
        10.0,
        ConditionFilter::default(),
    );
    let outcome = evaluate(&engine, &q).expect("評価できる");
    let points = &outcome.candidates[0].accident_points;
    assert_eq!(
        points.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    assert_eq!(
        points.iter().map(|p| p.attribution).collect::<Vec<_>>(),
        vec![
            Attribution::Edge,
            Attribution::Intersection,
            Attribution::Edge,
            Attribution::Unattributed,
        ]
    );
}

#[test]
fn sidewalk_parent_accident_is_not_reported_as_unattributed() {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.6, 139.7);
    let b = builder.add_node_deg(35.6, 139.701);
    let walk = builder.add_edge(
        EdgeSpec::new(a, b)
            .length_dm(1000)
            .access(access::WALK_FWD | access::WALK_BWD),
    );
    let parent = builder.add_edge(
        EdgeSpec::new(a, b)
            .length_dm(1000)
            .access(access::CAR_FWD | access::CAR_BWD),
    );
    let graph = builder.build();
    let topology = topology(&graph, &[], &[(walk, parent)]);
    let mid_lon = (graph.node_coord(a).to_latlon().lon + graph.node_coord(b).to_latlon().lon) / 2.0;
    let mut accident = Accident::at(north(35.6, 10.0), mid_lon);
    accident.modes = Mode::Walk.mask();
    let specs = vec![Spec::edge(parent, 500, 1.0)];
    let engine = engine(&builder, topology, &specs, snapshot(&[accident]));
    let q = query(
        graph.node_coord(a).to_latlon(),
        graph.node_coord(b).to_latlon(),
        Mode::Walk,
        10.0,
        ConditionFilter::default(),
    );
    let outcome = evaluate(&engine, &q).expect("評価できる");
    let point = &outcome.candidates[0].accident_points[0];
    assert_eq!(point.attribution, Attribution::Edge);
    assert_ne!(point.attribution, Attribution::Unattributed);
}

#[test]
fn conditions_filter_but_weather_does_not_reduce_the_set() {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.6, 139.7);
    let b = builder.add_node_deg(35.6, 139.701);
    let e0 = builder.add_edge(EdgeSpec::new(a, b).length_dm(1000));
    let graph = builder.build();
    let specs = (0..5).map(|_| Spec::edge(e0, 500, 0.2)).collect::<Vec<_>>();
    let mut accidents = vec![Accident::at(35.6, 139.7); 5];
    accidents[0].day_type = DayType::Weekday.code();
    accidents[1].weather = Weather::Rain.code();
    accidents[2].modes = Mode::Walk.mask();
    accidents[2].weather = Weather::Rain.code();
    accidents[3].period = Period::Night.code();
    accidents[3].weather = Weather::Rain.code();
    accidents[4].weather = Weather::Clear.code();
    for accident in &mut accidents[1..4] {
        accident.day_type = DayType::Holiday.code();
    }
    accidents[4].day_type = DayType::Holiday.code();
    let engine = engine(
        &builder,
        empty_topology(&graph),
        &specs,
        snapshot(&accidents),
    );
    let conditions = ConditionFilter {
        period: Some(Period::Daytime),
        day_type: Some(DayType::Holiday),
        weather: Some(Weather::Rain),
    };
    let q = query(
        graph.node_coord(a).to_latlon(),
        graph.node_coord(b).to_latlon(),
        Mode::Car,
        10.0,
        conditions,
    );
    let outcome = evaluate(&engine, &q).expect("評価できる");
    let candidate = &outcome.candidates[0];
    assert_eq!(candidate.totals.accident_count, 2);
    assert_eq!(candidate.weather_match.expect("天候指定").accident_count, 1);
    let ids: Vec<u64> = candidate.accident_points.iter().map(|p| p.id).collect();
    assert_eq!(ids, vec![1, 4]);
}

#[test]
fn total_accident_count_is_counted_before_condition_filter() {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.6, 139.7);
    let b = builder.add_node_deg(35.6, 139.701);
    let e0 = builder.add_edge(EdgeSpec::new(a, b).length_dm(1000));
    let graph = builder.build();
    let specs = (0..5).map(|_| Spec::edge(e0, 500, 0.2)).collect::<Vec<_>>();
    let mut accidents = vec![Accident::at(35.6, 139.7); 5];
    // 0は曜日区分、2は移動手段、3は時間帯でBから外れる。1と4は残る。
    accidents[0].day_type = DayType::Weekday.code();
    accidents[2].modes = Mode::Walk.mask();
    accidents[3].period = Period::Night.code();
    for accident in &mut accidents[1..] {
        accident.day_type = DayType::Holiday.code();
    }
    let engine = engine(
        &builder,
        empty_topology(&graph),
        &specs,
        snapshot(&accidents),
    );
    let conditions = ConditionFilter {
        period: Some(Period::Daytime),
        day_type: Some(DayType::Holiday),
        weather: Some(Weather::Rain),
    };
    let q = query(
        graph.node_coord(a).to_latlon(),
        graph.node_coord(b).to_latlon(),
        Mode::Car,
        10.0,
        conditions,
    );
    let outcome = evaluate(&engine, &q).expect("評価できる");
    let candidate = &outcome.candidates[0];
    assert_eq!(candidate.totals.accident_count, 2);
    assert_eq!(
        candidate.total_accident_count,
        Some(5),
        "条件で絞る前の事故IDの数を数える"
    );
}

#[test]
fn one_accident_with_multiple_assignments_is_counted_once() {
    let (builder, a, b, c, e0, _e1) = line_abc();
    let graph = builder.build();
    let topology = topology(&graph, &[vec![b]], &[]);
    let specs = vec![Spec::edge_and_cluster(e0, 500, 0, 0.5)];
    let engine = engine(
        &builder,
        topology,
        &specs,
        snapshot(&[Accident::at(35.6, 139.7)]),
    );
    let q = query(
        graph.node_coord(a).to_latlon(),
        graph.node_coord(c).to_latlon(),
        Mode::Car,
        10.0,
        ConditionFilter::default(),
    );
    let outcome = evaluate(&engine, &q).expect("評価できる");
    assert_eq!(outcome.candidates[0].totals.accident_count, 1);
    assert_eq!(outcome.candidates[0].accident_points.len(), 1);
}

// ---------------------------------------------------------------------------------------------
// 高リスク区間（behavior.md 7章）
// ---------------------------------------------------------------------------------------------

/// A→B の直線1本（100m）。
fn line_ab() -> (GraphBuilder, NodeId, NodeId, EdgeId) {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.6, 139.7);
    let b = builder.add_node_deg(35.6, 139.701);
    let e0 = builder.add_edge(EdgeSpec::new(a, b).length_dm(1000));
    (builder, a, b, e0)
}

fn run_line_ab(
    specs: &[Spec],
    accidents: Vec<Accident>,
) -> accident_route_backend::contract::RouteOutcome {
    let (builder, a, b, _e0) = line_ab();
    let graph = builder.build();
    let engine = engine(
        &builder,
        empty_topology(&graph),
        specs,
        snapshot(&accidents),
    );
    let q = query(
        graph.node_coord(a).to_latlon(),
        graph.node_coord(b).to_latlon(),
        Mode::Car,
        10.0,
        ConditionFilter::default(),
    );
    evaluate(&engine, &q).expect("評価できる")
}

#[test]
fn high_risk_threshold_is_three_accidents() {
    let two = run_line_ab(
        &[Spec::edge(0, 100, 0.5), Spec::edge(0, 200, 0.5)],
        vec![Accident::at(35.6, 139.7).injuries(1); 2],
    );
    assert_eq!(two.candidates[0].high_risk_total, 0);
    assert!(two.candidates[0].high_risk_sections.is_empty());
    assert!(
        two.candidates[0]
            .navigation
            .as_ref()
            .expect("選定候補")
            .sections
            .is_empty()
    );

    let three = run_line_ab(
        &[
            Spec::edge(0, 100, 0.5),
            Spec::edge(0, 200, 0.5),
            Spec::edge(0, 300, 0.5),
        ],
        vec![Accident::at(35.6, 139.7).injuries(1); 3],
    );
    assert_eq!(three.candidates[0].high_risk_total, 1);
    assert_eq!(three.candidates[0].high_risk_sections.len(), 1);
    assert_eq!(three.candidates[0].high_risk_sections[0].severity_weight, 3);
    let nav = three.candidates[0].navigation.as_ref().expect("選定候補");
    let covered: u64 = nav
        .sections
        .iter()
        .map(|s| u64::from(s.end_segment_index - s.start_segment_index + 1))
        .sum();
    assert_eq!(covered, three.candidates[0].high_risk_total);
}

#[test]
fn the_same_weight_prefers_an_edge_over_a_cluster() {
    let (builder, a, b, c, e0, _e1) = line_abc();
    let graph = builder.build();
    let topology = topology(&graph, &[vec![b]], &[]);
    let specs: Vec<Spec> = (0..3)
        .map(|_| Spec::edge_and_cluster(e0, 100, 0, 0.5))
        .collect();
    let engine = engine(
        &builder,
        topology,
        &specs,
        snapshot(&vec![Accident::at(35.6, 139.7).injuries(1); 3]),
    );
    let q = query(
        graph.node_coord(a).to_latlon(),
        graph.node_coord(c).to_latlon(),
        Mode::Car,
        10.0,
        ConditionFilter::default(),
    );
    let outcome = evaluate(&engine, &q).expect("評価できる");
    let sections = &outcome.candidates[0].high_risk_sections;
    assert_eq!(sections.len(), 1);
    // 辺 e0 の10mの位置は先頭の小線分（sid 0）。クラスタBなら別のsidになる。
    assert_eq!(sections[0].segment_index, 0);
    assert_eq!(sections[0].severity_weight, 3);
}

#[test]
fn accidents_on_the_same_cluster_share_a_sid() {
    let (builder, a, b, c, _e0, _e1) = line_abc();
    let graph = builder.build();
    let topology = topology(&graph, &[vec![b]], &[]);
    let specs: Vec<Spec> = (0..3).map(|_| Spec::cluster(0, 1.0)).collect();
    let engine = engine(
        &builder,
        topology,
        &specs,
        snapshot(&vec![Accident::at(35.6, 139.7).injuries(1); 3]),
    );
    let q = query(
        graph.node_coord(a).to_latlon(),
        graph.node_coord(c).to_latlon(),
        Mode::Car,
        10.0,
        ConditionFilter::default(),
    );
    let outcome = evaluate(&engine, &q).expect("評価できる");
    let sections = &outcome.candidates[0].high_risk_sections;
    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0].severity_weight, 3);
}

#[test]
fn high_risk_total_equals_navigation_coverage() {
    let specs: Vec<Spec> = [100, 200, 300, 600, 700, 800]
        .into_iter()
        .map(|pos| Spec::edge(0, pos, 0.5))
        .collect();
    let outcome = run_line_ab(&specs, vec![Accident::at(35.6, 139.7).injuries(1); 6]);
    let candidate = &outcome.candidates[0];
    assert_eq!(candidate.high_risk_total, 2);
    assert_eq!(candidate.high_risk_sections.len(), 2);
    let nav = candidate.navigation.as_ref().expect("選定候補");
    let covered: u64 = nav
        .sections
        .iter()
        .map(|s| u64::from(s.end_segment_index - s.start_segment_index + 1))
        .sum();
    assert_eq!(covered, candidate.high_risk_total);
    check_outcome_invariants(&outcome).expect("相関制約");
}

// ---------------------------------------------------------------------------------------------
// 形状（behavior.md 5章）
// ---------------------------------------------------------------------------------------------

#[test]
fn long_path_is_simplified_and_keeps_both_ends() {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.6, 139.7);
    let b = builder.add_node_deg(35.6, 139.71);
    let (lat, lon_start, lon_end) = (
        builder.node_coord(a).lat_e7,
        builder.node_coord(a).lon_e7,
        builder.node_coord(b).lon_e7,
    );
    let shape: Vec<Coord> = (1..15_000)
        .map(|i| {
            let lon = lon_start
                + ((i64::from(lon_end) - i64::from(lon_start)) * i64::from(i) / 15_000) as i32;
            Coord::new(lat, lon)
        })
        .collect();
    let edge = builder.add_edge(EdgeSpec::new(a, b).shape(&shape));
    let graph = builder.build();
    let engine = engine(&builder, empty_topology(&graph), &[], snapshot(&[]));
    let q = query(
        graph.node_coord(a).to_latlon(),
        graph.node_coord(b).to_latlon(),
        Mode::Car,
        10.0,
        ConditionFilter::default(),
    );
    let outcome = evaluate(&engine, &q).expect("評価できる");
    let coordinates = &outcome.candidates[0].path.coordinates;
    assert!(
        coordinates.len() <= 15_000,
        "間引かれていない: {}",
        coordinates.len()
    );
    assert_eq!(
        coordinates.first(),
        Some(&graph.node_coord(a).to_latlon().into())
    );
    assert_eq!(
        coordinates.last(),
        Some(&graph.node_coord(b).to_latlon().into())
    );
    assert_no_duplicate_vertices(&outcome);
    let _ = edge;
}

// ---------------------------------------------------------------------------------------------
// エラー
// ---------------------------------------------------------------------------------------------

#[test]
fn a_fastest_path_over_80km_is_route_too_long() {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.6, 139.7);
    let b = builder.add_node_deg(35.6, 139.701);
    builder.add_edge(EdgeSpec::new(a, b).length_dm(800_001));
    let graph = builder.build();
    let engine = engine(&builder, empty_topology(&graph), &[], snapshot(&[]));
    let q = query(
        graph.node_coord(a).to_latlon(),
        graph.node_coord(b).to_latlon(),
        Mode::Car,
        10.0,
        ConditionFilter::default(),
    );
    assert_eq!(evaluate(&engine, &q), Err(EngineError::RouteTooLong));
}

#[test]
fn an_unreachable_origin_is_no_route() {
    let (builder, _a, b, _e0) = line_ab();
    let graph = builder.build();
    let engine = engine(&builder, empty_topology(&graph), &[], snapshot(&[]));
    let q = query(
        LatLon::new(35.7, 139.7),
        graph.node_coord(b).to_latlon(),
        Mode::Car,
        10.0,
        ConditionFilter::default(),
    );
    assert_eq!(evaluate(&engine, &q), Err(EngineError::NoRoute));
}

#[test]
fn an_expired_token_keeps_the_deadline_exceeded_kind() {
    let (builder, a, b, _e0) = line_ab();
    let graph = builder.build();
    let engine = engine(&builder, empty_topology(&graph), &[], snapshot(&[]));
    let q = query(
        graph.node_coord(a).to_latlon(),
        graph.node_coord(b).to_latlon(),
        Mode::Car,
        10.0,
        ConditionFilter::default(),
    );
    let cancel = CancelToken::with_timeout(Duration::from_millis(0));
    let result = engine.evaluate(
        &q,
        CandidateSet {
            routes: vec![],
            warnings: vec![],
        },
        &cancel,
    );
    assert_eq!(
        result,
        Err(EngineError::Interrupted(Interrupt::DeadlineExceeded))
    );
}

#[test]
fn cancellation_during_search_returns_interrupted_quickly() {
    let (builder, shape) = grid(300, 300, 70.0);
    let graph = builder.build();
    let engine = engine(&builder, empty_topology(&graph), &[], snapshot(&[]));
    let origin = graph.node_coord(shape.node(0, 0)).to_latlon();
    let dest = graph.node_coord(shape.node(299, 299)).to_latlon();
    let q = query(origin, dest, Mode::Car, 10.0, ConditionFilter::default());
    let cancel = CancelToken::unbounded();
    let token = cancel.clone();
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(5));
        let at = Instant::now();
        token.cancel();
        tx.send(at).expect("時刻を送る");
    });
    let result = engine.evaluate(
        &q,
        CandidateSet {
            routes: vec![],
            warnings: vec![],
        },
        &cancel,
    );
    let cancel_at = rx.recv().expect("取消し時刻");
    let elapsed = Instant::now().duration_since(cancel_at);
    handle.join().expect("取り消しスレッド");
    assert!(
        matches!(result, Err(EngineError::Interrupted(_))),
        "中断にならなかった: {result:?}"
    );
    assert!(
        elapsed <= Duration::from_millis(100),
        "取り消しから{}msかかった",
        elapsed.as_millis()
    );
}

// ---------------------------------------------------------------------------------------------
// 選定・文面
// ---------------------------------------------------------------------------------------------

/// 事故の有無にかかわらず2本の候補ができるグラフ（A→B→D が最短、A→C→D が遠回り）。
struct TwoRouteGraph {
    builder: GraphBuilder,
    graph: Graph,
    origin: NodeId,
    dest: NodeId,
    fast_edges: [EdgeId; 2],
}

fn two_route_graph() -> TwoRouteGraph {
    let mut builder = GraphBuilder::new();
    let a = builder.add_node_deg(35.6, 139.7);
    let b = builder.add_node_deg(35.6, 139.701);
    let c = builder.add_node_deg(35.601, 139.7005);
    let d = builder.add_node_deg(35.6, 139.702);
    let fast1 = builder.add_edge(EdgeSpec::new(a, b).length_dm(1000));
    let fast2 = builder.add_edge(EdgeSpec::new(b, d).length_dm(1000));
    builder.add_edge(EdgeSpec::new(a, c).length_dm(1100));
    builder.add_edge(EdgeSpec::new(c, d).length_dm(1100));
    let graph = builder.build();
    TwoRouteGraph {
        builder,
        graph,
        origin: a,
        dest: d,
        fast_edges: [fast1, fast2],
    }
}

fn evaluate_two_routes(
    two: &TwoRouteGraph,
    specs: &[Spec],
    snapshot: AccidentSnapshot,
    risk: Option<RiskModel>,
) -> accident_route_backend::contract::RouteOutcome {
    let topology = empty_topology(&two.graph);
    let engine = match risk {
        Some(risk) => engine_with_risk(&two.builder, topology, specs, snapshot, risk),
        None => engine(&two.builder, topology, specs, snapshot),
    };
    let q = query(
        two.graph.node_coord(two.origin).to_latlon(),
        two.graph.node_coord(two.dest).to_latlon(),
        Mode::Car,
        10.0,
        ConditionFilter::default(),
    );
    evaluate(&engine, &q).expect("評価できる")
}

#[test]
fn all_zero_accidents_still_selects_a_safest_route() {
    let (builder, a, b, _e0) = line_ab();
    let graph = builder.build();
    let engine = engine(&builder, empty_topology(&graph), &[], snapshot(&[]));
    let q = query(
        graph.node_coord(a).to_latlon(),
        graph.node_coord(b).to_latlon(),
        Mode::Car,
        10.0,
        ConditionFilter::default(),
    );
    let outcome = evaluate(&engine, &q).expect("評価できる");
    assert_eq!(outcome.safest, Some(0));
    assert_eq!(outcome.extra_minutes, Some(0.0));
    assert_eq!(outcome.risk_reduction_percent, None);
    assert!(outcome.reason.starts_with(old_engine::SAME_ROUTE_REASON));
    assert_eq!(
        outcome.reason,
        format!(
            "{}{}",
            old_engine::SAME_ROUTE_REASON,
            old_engine::ZERO_BASELINE_REASON
        )
    );
    assert_eq!(
        outcome.warnings,
        vec![
            candidates::SINGLE_CANDIDATE_WARNING.to_owned(),
            old_engine::NO_ACCIDENT_WARNING.to_owned(),
        ]
    );
    check_outcome_invariants(&outcome).expect("相関制約");
}

#[test]
fn all_zero_accidents_selects_the_lowest_model_risk_route() {
    let two = two_route_graph();
    let topology = empty_topology(&two.graph);
    let mut edge_k = vec![EDGE_K_NO_RISK; two.graph.edge_count()];
    for edge in two.fast_edges {
        edge_k[edge as usize] = 1;
    }
    let risk = model_with_edge_risk(&two.graph, &topology, edge_k);
    let outcome = evaluate_two_routes(&two, &[], snapshot(&[]), Some(risk));

    assert!(outcome.candidates.len() >= 2);
    assert_ne!(outcome.safest, Some(outcome.fastest));
    let s = outcome.safest.expect("全候補0件でもsafestを選ぶ");
    assert!(
        outcome
            .candidates
            .iter()
            .all(|c| c.totals.accident_count == 0)
    );
    assert!(outcome.candidates.iter().any(|c| c.weighted_risk > 0.0));
    assert!(outcome.candidates[s].navigation.is_some());
    assert!(
        outcome
            .warnings
            .iter()
            .any(|w| w == old_engine::NO_ACCIDENT_WARNING)
    );
    let extra = outcome.extra_minutes.expect("追加時間");
    let reduction = outcome.risk_reduction_percent.expect("削減率");
    assert_eq!(outcome.candidates[s].extra_minutes, extra);
    assert_eq!(
        outcome.reason,
        format!(
            "全候補で該当する過去の事故はありません。道路の種類ごとの平均から推定したリスクが最小の経路を選びました。追加{extra:.1}分、リスク{reduction:.1}%減です。"
        )
    );
    check_outcome_invariants(&outcome).expect("相関制約");
}

#[test]
fn accident_candidates_use_the_network_search_reason() {
    let two = two_route_graph();
    let topology = empty_topology(&two.graph);
    let mut edge_k = vec![EDGE_K_NO_RISK; two.graph.edge_count()];
    for edge in two.fast_edges {
        edge_k[edge as usize] = 1;
    }
    let risk = model_with_edge_risk(&two.graph, &topology, edge_k);
    let specs = [Spec::edge(two.fast_edges[0], 500, 1.0)];
    let outcome = evaluate_two_routes(
        &two,
        &specs,
        snapshot(&[Accident::at(35.6, 139.7).injuries(1)]),
        Some(risk),
    );

    assert!(outcome.candidates.len() >= 2);
    assert_ne!(outcome.safest, Some(outcome.fastest));
    let s = outcome.safest.expect("事故があるのでsafestを選ぶ");
    assert_eq!(outcome.candidates[s].totals.accident_count, 0);
    let extra = outcome.extra_minutes.expect("追加時間");
    let reduction = outcome.risk_reduction_percent.expect("削減率");
    assert_eq!(
        outcome.reason,
        format!(
            "道路網を探索して得た{}候補のうち、最短時間経路から+{}分以内で、推定リスクが最小の経路を選びました。追加{extra:.1}分、リスク{reduction:.1}%減です。",
            outcome.candidates.len(),
            old_engine::format_minutes(10.0),
        )
    );
    check_outcome_invariants(&outcome).expect("相関制約");
}

#[test]
fn risk_note_has_the_new_sentences_in_order() {
    let note = routing_risk_note(Mode::Car, None);
    let car = note.find("生活道路").expect("自動車の文");
    let tail = note.find("交通量や通行回数").expect("末尾の文");
    assert!(car < tail);
    assert!(!note.contains("雨・霧・雪を指定した場合"));

    let note = routing_risk_note(Mode::Car, Some(Weather::Clear));
    assert!(!note.contains("雨・霧・雪を指定した場合"));
    for weather in [Weather::Rain, Weather::Fog, Weather::Snow] {
        let note = routing_risk_note(Mode::Car, Some(weather));
        let car = note.find("生活道路").expect("自動車の文");
        let weather_sentence = note.find("雨・霧・雪を指定した場合").expect("天候の文");
        let tail = note.find("交通量や通行回数").expect("末尾の文");
        assert!(car < weather_sentence && weather_sentence < tail);
    }

    let note = routing_risk_note(Mode::Walk, Some(Weather::Rain));
    assert!(!note.contains("生活道路"));
    let weather_sentence = note.find("雨・霧・雪を指定した場合").expect("天候の文");
    let tail = note.find("交通量や通行回数").expect("末尾の文");
    assert!(weather_sentence < tail);
}
