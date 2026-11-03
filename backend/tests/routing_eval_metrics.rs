//! `y-metrics` と `match-routes` の計算（作業書M5.4）の試験。
//!
//! 合成グラフ（東向きの3辺と、少し北にある並行の1辺）と、自分で作った小さな事故の列・
//! 割り当てを使い、Y1・Y2・Y3の数え方と、折れ線から有向辺の列への写し方を確かめる。

use accident_route_backend::contract::Mode;
use accident_route_backend::domain::routing::NONE;
use accident_route_backend::domain::routing::accident::{
    AssignTarget, Assignment, EdgeAccidentIndex, TargetKind,
};
use accident_route_backend::domain::routing::eval::matching::{class_lengths_m, match_route};
use accident_route_backend::domain::routing::eval::metrics::{
    EvalPath, Y2Assignments, y1_count, y2_count, y3_weight,
};
use accident_route_backend::domain::routing::graph::synthetic::{EdgeSpec, GraphBuilder};
use accident_route_backend::domain::routing::graph::{Coord, DirEdge, Graph, HighwayClass};
use accident_route_backend::domain::routing::prep::odindex::build_odindex;
use accident_route_backend::domain::routing::search::endpoints::path_distance_dm;
use accident_route_backend::domain::routing::snap::{OdIndex, SnapPoint};
use accident_route_backend::domain::routing::topology::{ClusterRecord, Topology};
use accident_route_backend::domain::snapshot::{AccidentColumns, AccidentSnapshot};
use serde_json::Value;

// ---------------------------------------------------------------------------------------------
// 組み立ての道具
// ---------------------------------------------------------------------------------------------

/// 東向きの3辺（0→1→2→3）と、0.0002度北にある並行の辺（4→5）。
/// 辺0は約91m、辺3は辺0と同じ長さで、辺0と同じ距離にある点を作れる。
fn graph() -> Graph {
    let mut builder = GraphBuilder::new();
    let n0 = builder.add_node_deg(35.0, 139.0);
    let n1 = builder.add_node_deg(35.0, 139.001);
    let n2 = builder.add_node_deg(35.0, 139.002);
    let n3 = builder.add_node_deg(35.0, 139.003);
    let n4 = builder.add_node_deg(35.0002, 139.0);
    let n5 = builder.add_node_deg(35.0002, 139.001);
    builder.add_edge(EdgeSpec::new(n0, n1).class(HighwayClass::Residential));
    builder.add_edge(EdgeSpec::new(n1, n2).class(HighwayClass::Residential));
    builder.add_edge(EdgeSpec::new(n2, n3).class(HighwayClass::Residential));
    builder.add_edge(EdgeSpec::new(n4, n5).class(HighwayClass::Residential));
    builder.build()
}

/// 全辺を全手段で吸着可にした位相。交差点クラスタと歩道の親を指定できる。
fn topology(
    graph: &Graph,
    node_cluster: Vec<u32>,
    clusters: Vec<ClusterRecord>,
    cluster_nodes: Vec<u32>,
    sidewalk_parents: Vec<(u32, u32)>,
) -> Topology {
    let edges = graph.edge_count();
    let mut bits = vec![u64::MAX; edges.div_ceil(64)];
    if edges & 63 != 0
        && let Some(last) = bits.last_mut()
    {
        *last = (1_u64 << (edges % 64)) - 1;
    }
    let topology = Topology {
        params: Value::Null,
        snap_car: bits.clone(),
        snap_bike: bits.clone(),
        snap_walk: bits,
        node_cluster,
        clusters,
        cluster_nodes,
        twins: Vec::new(),
        sidewalk_parents: sidewalk_parents
            .into_iter()
            .map(|(sidewalk, parent)| {
                accident_route_backend::domain::routing::topology::SidewalkParent {
                    sidewalk,
                    parent,
                }
            })
            .collect(),
    };
    topology.validate(graph).expect("位相が不正");
    topology
}

fn cluster(node_start: u32, node_count: u16) -> ClusterRecord {
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

/// 事故の列。`(緯度, 経度, 手段のビット, source_year)`。
fn snapshot(points: &[(f64, f64, u8, u16)]) -> AccidentSnapshot {
    let mut columns = AccidentColumns::default();
    for &(lat, lon, modes, source_year) in points {
        columns.lat.push(lat);
        columns.lon.push(lon);
        columns.modes.push(modes);
        columns.period.push(1);
        columns.weather.push(0);
        columns.day_type.push(0);
        columns.deaths.push(0);
        columns.injuries.push(0);
        columns.source_year.push(source_year);
        columns.date.push(20240101);
    }
    AccidentSnapshot::new(columns)
}

fn odindex(graph: &Graph) -> OdIndex {
    build_odindex(graph.parts())
}

fn full_path(graph: &Graph, from: u32, to: u32) -> (SnapPoint, SnapPoint, Vec<DirEdge>) {
    let dedges: Vec<DirEdge> = (from..=to).map(|e| DirEdge::new(e, false)).collect();
    let start = SnapPoint::on_edge(graph, from, 0);
    let end = SnapPoint::on_edge(graph, to, graph.edge(to).length_dm);
    (start, end, dedges)
}

fn eval_at<'a>(start: &'a SnapPoint, end: &'a SnapPoint, dedges: &'a [DirEdge]) -> EvalPath<'a> {
    EvalPath { start, end, dedges }
}

fn edge_target(edge: u32, pos_dm: u32, weight: f32) -> AssignTarget {
    AssignTarget {
        target: edge,
        pos_dm,
        weight,
        kind: TargetKind::Edge.code(),
        flags: 0,
        reserved: 0,
    }
}

fn cluster_target(cluster: u32, weight: f32) -> AssignTarget {
    AssignTarget {
        target: cluster,
        pos_dm: u32::MAX,
        weight,
        kind: TargetKind::Cluster.code(),
        flags: 0,
        reserved: 0,
    }
}

fn assignment(per_accident: Vec<Vec<AssignTarget>>) -> Assignment {
    let mut targets = Vec::new();
    let mut offsets = vec![0_u32];
    for list in per_accident {
        targets.extend(list);
        offsets.push(targets.len() as u32);
    }
    Assignment {
        params: Value::Null,
        offsets,
        targets,
        unassigned: Vec::new(),
    }
}

// ---------------------------------------------------------------------------------------------
// Y1
// ---------------------------------------------------------------------------------------------

#[test]
fn y1_counts_only_accidents_within_15m_of_the_travelled_polyline() {
    let graph = graph();
    let snapshot = snapshot(&[
        // 経路の上（東向きの辺0の途中）
        (35.0, 139.0005, Mode::Car.mask(), 2024),
        // 北へ約14.5m（15m以内）
        (35.00013, 139.0005, Mode::Car.mask(), 2024),
        // 北へ約16.7m（15m超）
        (35.00015, 139.0005, Mode::Car.mask(), 2024),
        // 並行する辺3の上（約22m北）
        (35.0002, 139.0005, Mode::Car.mask(), 2024),
        // 評価年・手段が違う（数えない）
        (35.0, 139.0005, Mode::Car.mask(), 2025),
        (35.0, 139.0005, Mode::Walk.mask(), 2024),
    ]);
    let (start, end, dedges) = full_path(&graph, 0, 2);
    let years = [2024_u16];
    assert_eq!(
        y1_count(
            &graph,
            &snapshot,
            &years,
            Mode::Car,
            eval_at(&start, &end, &dedges)
        ),
        Some(2)
    );
    assert_eq!(
        y1_count(
            &graph,
            &snapshot,
            &years,
            Mode::Walk,
            eval_at(&start, &end, &dedges)
        ),
        Some(1)
    );

    // 出発の部分辺の外（辺0の西側）にある事故は、経路の折れ線から離れるので数えない。
    let start = SnapPoint::on_edge(&graph, 0, 700);
    let end = SnapPoint::on_edge(&graph, 2, graph.edge(2).length_dm);
    let dedges = [
        DirEdge::new(0, false),
        DirEdge::new(1, false),
        DirEdge::new(2, false),
    ];
    assert_eq!(
        y1_count(
            &graph,
            &snapshot,
            &years,
            Mode::Car,
            eval_at(&start, &end, &dedges)
        ),
        Some(0)
    );
}

// ---------------------------------------------------------------------------------------------
// Y2
// ---------------------------------------------------------------------------------------------

#[test]
fn y2_assigns_the_nearest_edge_with_ties_and_the_35m_limit() {
    let graph = graph();
    let topology = topology(
        &graph,
        vec![NONE; graph.node_count()],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let snapshot = snapshot(&[
        // 辺0と辺3の中間（どちらも約11.1m）。同じ距離なら辺番号の小さい辺0。
        (35.0001, 139.0005, Mode::Car.mask(), 2024),
        // どの車道の辺からも35m超。
        (35.0006, 139.0005, Mode::Car.mask(), 2024),
        // 評価年が違うので割り当てない。
        (35.0001, 139.0005, Mode::Car.mask(), 2025),
    ]);
    let years = [2024_u16];
    let assignments = Y2Assignments::build(&graph, &topology, &odindex(&graph), &snapshot, &years);
    assert_eq!(assignments.total, 2);
    assert_eq!(assignments.assigned, 1);
    let target = assignments.targets[0].expect("辺0に割り当てる");
    assert_eq!(target.edge, 0, "同じ距離なら辺番号の小さい方");
    assert!(target.pos_dm > 400 && target.pos_dm < 500);
    assert_eq!(assignments.targets[1], None, "35m超は割り当てない");

    // 経路が辺0を通るときだけ数える。
    let (start, end, dedges) = full_path(&graph, 0, 0);
    assert_eq!(
        y2_count(
            &graph,
            &topology,
            &snapshot,
            &years,
            Mode::Car,
            &assignments,
            eval_at(&start, &end, &dedges)
        ),
        Some(1)
    );
    let (start, end, dedges) = full_path(&graph, 1, 1);
    assert_eq!(
        y2_count(
            &graph,
            &topology,
            &snapshot,
            &years,
            Mode::Car,
            &assignments,
            eval_at(&start, &end, &dedges)
        ),
        Some(0)
    );

    // 出発の部分辺の外にある事故は、辺0を通っても数えない。
    let start = SnapPoint::on_edge(&graph, 0, 700);
    let end = SnapPoint::on_edge(&graph, 0, graph.edge(0).length_dm);
    let dedges = [DirEdge::new(0, false)];
    assert_eq!(
        y2_count(
            &graph,
            &topology,
            &snapshot,
            &years,
            Mode::Car,
            &assignments,
            eval_at(&start, &end, &dedges)
        ),
        Some(0)
    );
    // 手段が違えば数えない。
    let (start, end, dedges) = full_path(&graph, 0, 0);
    assert_eq!(
        y2_count(
            &graph,
            &topology,
            &snapshot,
            &years,
            Mode::Walk,
            &assignments,
            eval_at(&start, &end, &dedges)
        ),
        Some(0)
    );
}

// ---------------------------------------------------------------------------------------------
// Y3
// ---------------------------------------------------------------------------------------------

#[test]
fn y3_sums_edges_and_clusters_with_the_path_risk_rules() {
    let graph = graph();
    // ノード1がクラスタ0、ノード2がクラスタ1。
    let topology = topology(
        &graph,
        vec![NONE, 0, 1, NONE, NONE, NONE],
        vec![cluster(0, 1), cluster(1, 1)],
        vec![1, 2],
        Vec::new(),
    );
    let accidents = assignment(vec![
        vec![edge_target(0, 100, 0.6)],
        vec![edge_target(0, 400, 0.3)],
        vec![edge_target(1, 200, 0.4)],
        vec![edge_target(1, 800, 0.2)],
        vec![cluster_target(0, 0.5)],
        vec![cluster_target(1, 0.7)],
        vec![edge_target(2, 100, 0.9)],
        vec![edge_target(0, 100, 0.9)], // 評価年が違う
        vec![edge_target(0, 100, 0.9)], // 手段が違う
    ]);
    let mut columns = AccidentColumns::default();
    for (i, &year) in [2024_u16, 2024, 2024, 2024, 2024, 2024, 2024, 2025, 2024]
        .iter()
        .enumerate()
    {
        columns.lat.push(35.0);
        columns.lon.push(139.0);
        columns.modes.push(if i == 8 {
            Mode::Walk.mask()
        } else {
            Mode::Car.mask()
        });
        columns.period.push(1);
        columns.weather.push(0);
        columns.day_type.push(0);
        columns.deaths.push(0);
        columns.injuries.push(0);
        columns.source_year.push(year);
        columns.date.push(20240101);
    }
    let snapshot = AccidentSnapshot::new(columns);
    accidents.validate(graph.edge_count(), 2).unwrap();
    let index = EdgeAccidentIndex::build(&accidents, graph.edge_count(), 2);
    let years = [2024_u16];

    // ノード0 → ノード2（辺0と辺1の全長）。辺0はクラスタ0へ、辺1はクラスタ1へ入る。
    let (start, end, dedges) = full_path(&graph, 0, 1);
    let y3 = y3_weight(
        &graph,
        &topology,
        &index,
        &snapshot,
        &years,
        Mode::Car,
        eval_at(&start, &end, &dedges),
    )
    .unwrap();
    assert!((y3 - 2.7).abs() < 1e-6, "y3={y3}");

    // 到着の辺を途中まで通る（辺1の200dmまで）。到着側のクラスタ1は数えない。
    let start = SnapPoint::on_edge(&graph, 0, 0);
    let end = SnapPoint::on_edge(&graph, 1, 200);
    let y3 = y3_weight(
        &graph,
        &topology,
        &index,
        &snapshot,
        &years,
        Mode::Car,
        eval_at(
            &start,
            &end,
            &[DirEdge::new(0, false), DirEdge::new(1, false)],
        ),
    )
    .unwrap();
    // 辺0 0.6+0.3、辺1は200dmまでなので0.4、クラスタ0 0.5。
    assert!((y3 - 1.8).abs() < 1e-6, "y3={y3}");

    // 出発の辺を途中から通る（辺0の300dmから）。位置100dmの割り当ては数えない。
    let start = SnapPoint::on_edge(&graph, 0, 300);
    let end = SnapPoint::on_edge(&graph, 0, graph.edge(0).length_dm);
    let y3 = y3_weight(
        &graph,
        &topology,
        &index,
        &snapshot,
        &years,
        Mode::Car,
        eval_at(&start, &end, &[DirEdge::new(0, false)]),
    )
    .unwrap();
    // 辺0の400dm（0.3）とクラスタ0（0.5）。到着はノード1なのでクラスタを数える。
    assert!((y3 - 0.8).abs() < 1e-6, "y3={y3}");

    // クラスタ0から出てクラスタ1へ入る辺1だけの経路。クラスタ0へは入らない。
    let start = SnapPoint::on_edge(&graph, 1, 0);
    let end = SnapPoint::on_edge(&graph, 1, graph.edge(1).length_dm);
    let y3 = y3_weight(
        &graph,
        &topology,
        &index,
        &snapshot,
        &years,
        Mode::Car,
        eval_at(&start, &end, &[DirEdge::new(1, false)]),
    )
    .unwrap();
    // 辺1（0.4+0.2）と、外から入るクラスタ1（0.7）。
    assert!((y3 - 1.3).abs() < 1e-6, "y3={y3}");
}

#[test]
fn y3_counts_a_sidewalk_through_its_parent_edge() {
    let graph = graph();
    // 辺0の歩道として辺3を使う（幾何は別でも、親の割り当てを数える）。
    let topology = topology(
        &graph,
        vec![NONE; graph.node_count()],
        Vec::new(),
        Vec::new(),
        vec![(3, 0)],
    );
    let accidents = assignment(vec![
        vec![edge_target(0, 100, 0.6)],
        vec![edge_target(0, 800, 0.4)],
    ]);
    let index = EdgeAccidentIndex::build(&accidents, graph.edge_count(), 0);
    let snapshot = snapshot(&[
        (35.0, 139.0, Mode::Walk.mask(), 2024),
        (35.0, 139.0, Mode::Walk.mask(), 2024),
    ]);
    let years = [2024_u16];
    // 歩道（辺3）の全長。親の辺0の全長に写るので、両方の割り当てを数える。
    let start = SnapPoint::on_edge(&graph, 3, 0);
    let end = SnapPoint::on_edge(&graph, 3, graph.edge(3).length_dm);
    let y3 = y3_weight(
        &graph,
        &topology,
        &index,
        &snapshot,
        &years,
        Mode::Walk,
        eval_at(&start, &end, &[DirEdge::new(3, false)]),
    )
    .unwrap();
    assert!((y3 - 1.0).abs() < 1e-6, "y3={y3}");
}

// ---------------------------------------------------------------------------------------------
// match-routes
// ---------------------------------------------------------------------------------------------

#[test]
fn match_routes_recovers_the_same_dedges_and_tolerates_a_5m_shift() {
    let graph = graph();
    let topology = topology(
        &graph,
        vec![NONE; graph.node_count()],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let index = odindex(&graph);
    let (start, end, dedges) = full_path(&graph, 0, 2);
    let coordinates = accident_route_backend::domain::routing::eval::geometry::path_polyline(
        &graph, &start, &end, &dedges,
    )
    .unwrap();
    let matched =
        match_route(&graph, &topology, &index, Mode::Car, &coordinates, 20.0).expect("写せる");
    assert_eq!(matched.path.dedges, dedges);
    assert_eq!(
        matched.path.time_ds,
        accident_route_backend::domain::routing::eval::matching::canonical_time_ds(
            &graph,
            Mode::Car,
            &matched.path
        )
        .unwrap()
    );
    let distance = matched.distance_m_for_test(&graph);
    assert!(
        (distance - path_distance_dm(&graph, &start, &end, &dedges).unwrap() as f64 / 10.0).abs()
            < 1e-6
    );
    let classes = class_lengths_m(&graph, &matched.path).unwrap();
    assert!(classes.contains_key(&HighwayClass::Residential.code()));

    // 5m北へずらしても同じ辺の列に写る。
    let shifted: Vec<Coord> = coordinates
        .iter()
        .map(|c| Coord::new(c.lat_e7 + 45, c.lon_e7))
        .collect();
    let matched =
        match_route(&graph, &topology, &index, Mode::Car, &shifted, 20.0).expect("写せる");
    assert_eq!(matched.path.dedges, dedges);
}

/// `MatchedRoute` の道のりの小さな補助（テストで読みやすくする）。
trait DistanceForTest {
    fn distance_m_for_test(&self, graph: &Graph) -> f64;
}

impl DistanceForTest for accident_route_backend::domain::routing::eval::matching::MatchedRoute {
    fn distance_m_for_test(&self, graph: &Graph) -> f64 {
        accident_route_backend::domain::routing::eval::matching::distance_m(graph, &self.path)
            .unwrap()
    }
}
