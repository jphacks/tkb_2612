//! 進行方向の案内地点（計画書4章、T4）の受け入れ試験。
//!
//! 合成グラフの座標で、分岐・連・角度・距離の規則を確かめる。角度が定数の境界に
//! ならない座標を選んでいる。

use accident_route_backend::contract::geo::METERS_PER_DEGREE;
use accident_route_backend::contract::{CancelToken, GuidanceStep, Interrupt, Maneuver, Mode};
use accident_route_backend::domain::corridor::Subdivision;
use accident_route_backend::domain::navigation::vertex_distances;
use accident_route_backend::domain::routing::NONE;
use accident_route_backend::domain::routing::evaluate::geometry::{RouteShape, build_shape};
use accident_route_backend::domain::routing::evaluate::guidance::build_guidance;
use accident_route_backend::domain::routing::graph::synthetic::{EdgeSpec, GraphBuilder};
use accident_route_backend::domain::routing::graph::{DirEdge, EdgeId, Graph, NodeId, edge_flags};
use accident_route_backend::domain::routing::search::Path;
use accident_route_backend::domain::routing::snap::SnapPoint;
use accident_route_backend::domain::routing::topology::{ClusterRecord, SidewalkParent, Topology};
use serde_json::json;

const LAT: f64 = 35.0;
const LON: f64 = 139.0;

// ---------------------------------------------------------------------------------------------
// 組み立ての道具
// ---------------------------------------------------------------------------------------------

fn offset(east_m: f64, north_m: f64) -> (f64, f64) {
    let lat = LAT + north_m / METERS_PER_DEGREE;
    let lon = LON + east_m / (METERS_PER_DEGREE * LAT.to_radians().cos());
    (lat, lon)
}

fn add(builder: &mut GraphBuilder, east_m: f64, north_m: f64) -> NodeId {
    let (lat, lon) = offset(east_m, north_m);
    builder.add_node_deg(lat, lon)
}

/// 東から時計回りに `angle_deg` の向きへ `radius_m` 進んだノード。
fn add_polar(builder: &mut GraphBuilder, angle_deg: f64, radius_m: f64) -> NodeId {
    let angle = angle_deg.to_radians();
    add(builder, radius_m * angle.cos(), -radius_m * angle.sin())
}

/// `from` から東を0として時計回りに `angle_deg` の向きへ `radius_m` 進んだノード。
fn add_from(builder: &mut GraphBuilder, from: NodeId, angle_deg: f64, radius_m: f64) -> NodeId {
    let base = builder.node_coord(from).to_latlon();
    let angle = angle_deg.to_radians();
    let east = radius_m * angle.cos();
    let north = -radius_m * angle.sin();
    let lat = base.lat + north / METERS_PER_DEGREE;
    let lon = base.lon + east / (METERS_PER_DEGREE * base.lat.to_radians().cos());
    builder.add_node_deg(lat, lon)
}

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

fn topology(graph: &Graph, clusters: &[Vec<NodeId>]) -> Topology {
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
            top_class: 11,
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
        sidewalk_parents: Vec::<SidewalkParent>::new(),
    }
}

fn start_point(graph: &Graph, d: DirEdge) -> SnapPoint {
    let offset_dm = if d.is_reverse() {
        graph.edge(d.edge()).length_dm
    } else {
        0
    };
    SnapPoint::on_edge(graph, d.edge(), offset_dm)
}

fn end_point(graph: &Graph, d: DirEdge) -> SnapPoint {
    let offset_dm = if d.is_reverse() {
        0
    } else {
        graph.edge(d.edge()).length_dm
    };
    SnapPoint::on_edge(graph, d.edge(), offset_dm)
}

struct Prepared {
    graph: Graph,
    topology: Topology,
    shape: RouteShape,
    distances: Vec<f64>,
    path: Path,
}

fn prepare(builder: &GraphBuilder, nodes: &[NodeId], clusters: &[Vec<NodeId>]) -> Prepared {
    let graph = builder.build();
    let topology = topology(&graph, clusters);
    let dedges = builder.dedges(nodes);
    let path = Path {
        start: start_point(&graph, dedges[0]),
        end: end_point(&graph, *dedges.last().expect("2ノード以上")),
        dedges,
        time_ds: 0,
        risk_fx: 0,
    };
    let cancel = CancelToken::unbounded();
    let shape = build_shape(&graph, &path, &cancel).expect("形状を作れる");
    let subdivision = Subdivision::build(&shape.display, &cancel).expect("小線分を作れる");
    let distances = vertex_distances(&subdivision);
    Prepared {
        graph,
        topology,
        shape,
        distances,
        path,
    }
}

fn guidance(
    builder: &GraphBuilder,
    nodes: &[NodeId],
    mode: Mode,
    clusters: &[Vec<NodeId>],
) -> (Prepared, Vec<GuidanceStep>) {
    let prepared = prepare(builder, nodes, clusters);
    let steps = build_guidance(
        &prepared.graph,
        &prepared.topology,
        &prepared.path,
        &prepared.shape,
        mode,
        &prepared.distances,
        &CancelToken::unbounded(),
    )
    .expect("案内を作れる");
    (prepared, steps)
}

fn assert_only_arrive(prepared: &Prepared, steps: &[GuidanceStep]) {
    assert_eq!(steps.len(), 1, "{steps:?}");
    assert_eq!(steps[0].maneuver, Maneuver::Arrive);
    assert_eq!(steps[0].at_m, *prepared.distances.last().expect("頂点あり"));
}

fn node_distance(prepared: &Prepared, dedge_index: usize) -> f64 {
    let vertex = prepared
        .shape
        .internal_node_vertex(dedge_index)
        .expect("内部ノード");
    prepared.distances[vertex]
}

fn edge(builder: &mut GraphBuilder, tail: NodeId, head: NodeId) -> EdgeId {
    builder.add_edge(EdgeSpec::new(tail, head))
}

// ---------------------------------------------------------------------------------------------
// 1〜3：十字路
// ---------------------------------------------------------------------------------------------

#[test]
fn t4_01_crossroad_east_to_south_is_right() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let c = add(&mut b, 0.0, 0.0);
    let s = add(&mut b, 0.0, -100.0);
    let n = add(&mut b, 0.0, 100.0);
    let e = add(&mut b, 100.0, 0.0);
    edge(&mut b, w, c);
    edge(&mut b, c, s);
    edge(&mut b, c, n);
    edge(&mut b, c, e);
    let (prepared, steps) = guidance(&b, &[w, c, s], Mode::Car, &[]);
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].maneuver, Maneuver::Right);
    assert_eq!(steps[0].at_m, node_distance(&prepared, 0));
    assert_eq!(steps[1].maneuver, Maneuver::Arrive);
    assert_eq!(steps[1].at_m, *prepared.distances.last().expect("頂点あり"));
}

#[test]
fn t4_02_crossroad_east_to_north_is_left() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let c = add(&mut b, 0.0, 0.0);
    let s = add(&mut b, 0.0, -100.0);
    let n = add(&mut b, 0.0, 100.0);
    let e = add(&mut b, 100.0, 0.0);
    edge(&mut b, w, c);
    edge(&mut b, c, n);
    edge(&mut b, c, s);
    edge(&mut b, c, e);
    let (_, steps) = guidance(&b, &[w, c, n], Mode::Car, &[]);
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].maneuver, Maneuver::Left);
    assert_eq!(steps[1].maneuver, Maneuver::Arrive);
}

#[test]
fn t4_03_crossroad_straight_is_arrive_only() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let c = add(&mut b, 0.0, 0.0);
    let s = add(&mut b, 0.0, -100.0);
    let n = add(&mut b, 0.0, 100.0);
    let e = add(&mut b, 100.0, 0.0);
    edge(&mut b, w, c);
    edge(&mut b, c, e);
    edge(&mut b, c, n);
    edge(&mut b, c, s);
    let (prepared, steps) = guidance(&b, &[w, c, e], Mode::Car, &[]);
    assert_only_arrive(&prepared, &steps);
}

// ---------------------------------------------------------------------------------------------
// 4〜6：分岐の有無と道路区分
// ---------------------------------------------------------------------------------------------

#[test]
fn t4_04_curve_without_branch_is_arrive_only() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let c = add(&mut b, 0.0, 0.0);
    let s = add(&mut b, 0.0, -100.0);
    edge(&mut b, w, c);
    edge(&mut b, c, s);
    let (prepared, steps) = guidance(&b, &[w, c, s], Mode::Car, &[]);
    assert_only_arrive(&prepared, &steps);
}

#[test]
fn t4_05_fifty_degree_bend_with_lower_side_road_is_arrive_only() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let c = add(&mut b, 0.0, 0.0);
    let out = add_polar(&mut b, 50.0, 100.0);
    let lower = add_polar(&mut b, -20.0, 100.0);
    edge(&mut b, w, c);
    edge(&mut b, c, out);
    b.add_edge(
        EdgeSpec::new(c, lower)
            .class(accident_route_backend::domain::routing::graph::HighwayClass::Service),
    );
    let (prepared, steps) = guidance(&b, &[w, c, out], Mode::Car, &[]);
    assert_only_arrive(&prepared, &steps);
}

#[test]
fn t4_06_fifty_degree_bend_with_higher_side_road_is_right() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let c = add(&mut b, 0.0, 0.0);
    let out = add_polar(&mut b, 50.0, 100.0);
    let higher = add_polar(&mut b, -20.0, 100.0);
    edge(&mut b, w, c);
    edge(&mut b, c, out);
    b.add_edge(
        EdgeSpec::new(c, higher)
            .class(accident_route_backend::domain::routing::graph::HighwayClass::Primary),
    );
    let (_, steps) = guidance(&b, &[w, c, out], Mode::Car, &[]);
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].maneuver, Maneuver::Right);
}

// ---------------------------------------------------------------------------------------------
// 7〜8：斜めの分岐と手段
// ---------------------------------------------------------------------------------------------

#[test]
fn t4_07_y_junction_right_branch_is_slight_right() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let c = add(&mut b, 0.0, 0.0);
    let right = add_polar(&mut b, 20.0, 100.0);
    let left = add_polar(&mut b, -20.0, 100.0);
    edge(&mut b, w, c);
    edge(&mut b, c, right);
    edge(&mut b, c, left);
    let (_, steps) = guidance(&b, &[w, c, right], Mode::Car, &[]);
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].maneuver, Maneuver::SlightRight);
}

#[test]
fn t4_08_footway_branch_is_ignored_for_car_but_right_for_walk() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let c = add(&mut b, 0.0, 0.0);
    let s = add(&mut b, 0.0, -100.0);
    let n = add(&mut b, 0.0, 100.0);
    edge(&mut b, w, c);
    edge(&mut b, c, s);
    b.add_edge(
        EdgeSpec::new(c, n)
            .class(accident_route_backend::domain::routing::graph::HighwayClass::Footway)
            .deny(Mode::Car),
    );
    let (car_prepared, car_steps) = guidance(&b, &[w, c, s], Mode::Car, &[]);
    assert_only_arrive(&car_prepared, &car_steps);
    let (_, walk_steps) = guidance(&b, &[w, c, s], Mode::Walk, &[]);
    assert_eq!(walk_steps.len(), 2, "{walk_steps:?}");
    assert_eq!(walk_steps[0].maneuver, Maneuver::Right);
}

// ---------------------------------------------------------------------------------------------
// 9〜12：連のまとめ方
// ---------------------------------------------------------------------------------------------

#[test]
fn t4_09_same_cluster_two_nodes_make_one_right() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let a = add(&mut b, 0.0, 0.0);
    let second = add(&mut b, 12.0, 0.0);
    let s = add(&mut b, 12.0, -100.0);
    let n = add(&mut b, 0.0, 100.0);
    let e = add(&mut b, 112.0, 0.0);
    edge(&mut b, w, a);
    edge(&mut b, a, second);
    edge(&mut b, second, s);
    edge(&mut b, a, n);
    edge(&mut b, second, e);
    let (prepared, steps) = guidance(&b, &[w, a, second, s], Mode::Car, &[vec![a, second]]);
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].maneuver, Maneuver::Right);
    assert_eq!(steps[0].at_m, node_distance(&prepared, 1));
    assert_eq!(steps[1].maneuver, Maneuver::Arrive);
}

#[test]
fn t4_10_two_right_turns_make_one_uturn() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let a = add(&mut b, 0.0, 0.0);
    let first = add_from(&mut b, a, 80.0, 10.0);
    let second = add_from(&mut b, first, 160.0, 100.0);
    let a_branch = add(&mut b, 0.0, 100.0);
    let first_branch = add_from(&mut b, first, 0.0, 100.0);
    edge(&mut b, w, a);
    edge(&mut b, a, first);
    edge(&mut b, first, second);
    edge(&mut b, a, a_branch);
    edge(&mut b, first, first_branch);
    let (_, steps) = guidance(&b, &[w, a, first, second], Mode::Car, &[]);
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].maneuver, Maneuver::Uturn);
    assert_eq!(steps[1].maneuver, Maneuver::Arrive);
}

#[test]
fn t4_11_right_then_left_within_fifteen_metres_is_arrive_only() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let a = add(&mut b, 0.0, 0.0);
    let second = add(&mut b, 0.0, -10.0);
    let c = add(&mut b, 100.0, -10.0);
    let a_branch = add(&mut b, 0.0, 100.0);
    let second_branch = add(&mut b, -100.0, -10.0);
    edge(&mut b, w, a);
    edge(&mut b, a, second);
    edge(&mut b, second, c);
    edge(&mut b, a, a_branch);
    edge(&mut b, second, second_branch);
    let (prepared, steps) = guidance(&b, &[w, a, second, c], Mode::Walk, &[]);
    assert_only_arrive(&prepared, &steps);
}

#[test]
fn t4_12_diagonal_last_three_metres_is_arrive_only() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let node = add(&mut b, 0.0, 0.0);
    let c = add(&mut b, 100.0, 0.0);
    let p = add(&mut b, -3.0, 3.0);
    let branch = add(&mut b, 0.0, 100.0);
    b.add_edge(EdgeSpec::new(w, node).shape(&[b.node_coord(p)]));
    edge(&mut b, node, c);
    edge(&mut b, node, branch);
    let (prepared, steps) = guidance(&b, &[w, node, c], Mode::Car, &[]);
    assert_only_arrive(&prepared, &steps);
}

// ---------------------------------------------------------------------------------------------
// 13〜17：Uターン、環状交差点、端の扱い
// ---------------------------------------------------------------------------------------------

#[test]
fn t4_13_returning_on_the_same_edge_is_uturn() {
    let mut b = GraphBuilder::new();
    let a = add(&mut b, 0.0, 0.0);
    let node = add(&mut b, 100.0, 0.0);
    let branch = add(&mut b, 100.0, 100.0);
    edge(&mut b, a, node);
    edge(&mut b, node, branch);
    let (_, steps) = guidance(&b, &[a, node, a], Mode::Car, &[]);
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].maneuver, Maneuver::Uturn);
    assert_eq!(steps[1].maneuver, Maneuver::Arrive);
}

#[test]
fn t4_13b_returning_at_a_dead_end_is_uturn() {
    let mut b = GraphBuilder::new();
    let a = add(&mut b, 0.0, 0.0);
    let node = add(&mut b, 100.0, 0.0);
    edge(&mut b, a, node);
    let (prepared, steps) = guidance(&b, &[a, node, a], Mode::Car, &[]);
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].maneuver, Maneuver::Uturn);
    assert_eq!(steps[0].at_m, node_distance(&prepared, 0));
    assert_eq!(steps[1].maneuver, Maneuver::Arrive);
}

#[test]
fn t4_12b_cluster_away_from_the_run_does_not_extend_it() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let turn = add(&mut b, 0.0, 0.0);
    let q = add(&mut b, 0.0, -40.0);
    let r = add(&mut b, 0.0, -45.0);
    let x = add(&mut b, 100.0, -45.0);
    let branch = add(&mut b, 100.0, 0.0);
    edge(&mut b, w, turn);
    edge(&mut b, turn, q);
    edge(&mut b, q, r);
    edge(&mut b, r, x);
    edge(&mut b, turn, branch);
    let (prepared, steps) = guidance(&b, &[w, turn, q, r, x], Mode::Car, &[vec![q, r]]);
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].maneuver, Maneuver::Right);
    assert_eq!(steps[0].at_m, node_distance(&prepared, 0));
    assert_eq!(steps[1].maneuver, Maneuver::Arrive);
}

#[test]
fn t4_14_roundabout_second_exit_is_right_at_exit_node() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let r0 = add(&mut b, 0.0, 0.0);
    let r1 = add(&mut b, 20.0, 0.0);
    let r2 = add(&mut b, 40.0, 0.0);
    let r3 = add(&mut b, 60.0, 0.0);
    let first_exit = add(&mut b, 20.0, 100.0);
    let second_exit = add(&mut b, 40.0, -100.0);
    edge(&mut b, w, r0);
    b.add_edge(EdgeSpec::new(r0, r1).flags(edge_flags::ROUNDABOUT));
    b.add_edge(EdgeSpec::new(r1, r2).flags(edge_flags::ROUNDABOUT));
    b.add_edge(EdgeSpec::new(r2, r3).flags(edge_flags::ROUNDABOUT));
    edge(&mut b, r1, first_exit);
    edge(&mut b, r2, second_exit);
    let (prepared, steps) = guidance(&b, &[w, r0, r1, r2, second_exit], Mode::Car, &[]);
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].maneuver, Maneuver::Right);
    assert_eq!(steps[0].at_m, node_distance(&prepared, 2));
    assert_eq!(steps[1].maneuver, Maneuver::Arrive);
}

/// 60mを超える環の出口の10m先で脇道を直進しても、案内地点は環の出口のまま。
#[test]
fn long_roundabout_keeps_the_step_at_its_exit_past_a_nearby_side_road() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let r0 = add(&mut b, 0.0, 0.0);
    let r1 = add(&mut b, 40.0, 0.0);
    let r2 = add(&mut b, 80.0, 0.0);
    let r3 = add(&mut b, 120.0, 0.0);
    let r4 = add(&mut b, 160.0, 0.0);
    let after_exit = add(&mut b, 120.0, -10.0);
    let side = add(&mut b, 220.0, -10.0);
    let end = add(&mut b, 120.0, -110.0);
    edge(&mut b, w, r0);
    b.add_edge(EdgeSpec::new(r0, r1).flags(edge_flags::ROUNDABOUT));
    b.add_edge(EdgeSpec::new(r1, r2).flags(edge_flags::ROUNDABOUT));
    b.add_edge(EdgeSpec::new(r2, r3).flags(edge_flags::ROUNDABOUT));
    b.add_edge(EdgeSpec::new(r3, r4).flags(edge_flags::ROUNDABOUT));
    edge(&mut b, r3, after_exit);
    edge(&mut b, after_exit, side);
    edge(&mut b, after_exit, end);
    let (prepared, steps) = guidance(&b, &[w, r0, r1, r2, r3, after_exit, end], Mode::Car, &[]);
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].maneuver, Maneuver::Right);
    assert_eq!(steps[0].at_m, node_distance(&prepared, 3));
    assert_eq!(steps[1].maneuver, Maneuver::Arrive);
}

/// 60m未満の環でも、出口の14m先の曲がりを環の案内に合成しない。
#[test]
fn short_roundabout_does_not_merge_a_turn_just_after_its_exit() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let r0 = add(&mut b, 0.0, 0.0);
    let r1 = add(&mut b, 20.0, 0.0);
    let r2 = add(&mut b, 40.0, 0.0);
    let r3 = add(&mut b, 60.0, 0.0);
    let after_exit = add(&mut b, 40.0, -14.0);
    let south = add(&mut b, 40.0, -114.0);
    let east = add(&mut b, 140.0, -14.0);
    edge(&mut b, w, r0);
    b.add_edge(EdgeSpec::new(r0, r1).flags(edge_flags::ROUNDABOUT));
    b.add_edge(EdgeSpec::new(r1, r2).flags(edge_flags::ROUNDABOUT));
    b.add_edge(EdgeSpec::new(r2, r3).flags(edge_flags::ROUNDABOUT));
    edge(&mut b, r2, after_exit);
    edge(&mut b, after_exit, south);
    edge(&mut b, after_exit, east);
    let (prepared, steps) = guidance(&b, &[w, r0, r1, r2, after_exit, east], Mode::Car, &[]);
    assert_eq!(steps.len(), 3, "{steps:?}");
    assert_eq!(steps[0].maneuver, Maneuver::Right);
    assert_eq!(steps[0].at_m, node_distance(&prepared, 2));
    assert_eq!(steps[1].maneuver, Maneuver::Left);
    assert_eq!(steps[1].at_m, node_distance(&prepared, 3));
    assert_eq!(steps[2].maneuver, Maneuver::Arrive);
}

/// 環の出口の5m先で曲がっても、環の案内の向きにその曲がりを合成しない。
#[test]
fn roundabout_direction_is_independent_of_a_turn_five_metres_after_its_exit() {
    for (turn_east_m, expected) in [(-100.0, Maneuver::Right), (100.0, Maneuver::Left)] {
        let mut b = GraphBuilder::new();
        let w = add(&mut b, -100.0, 0.0);
        let r0 = add(&mut b, 0.0, 0.0);
        let r1 = add(&mut b, 20.0, 0.0);
        let r2 = add(&mut b, 40.0, 0.0);
        let r3 = add(&mut b, 60.0, 0.0);
        let after_exit = add(&mut b, 40.0, -5.0);
        let south = add(&mut b, 40.0, -105.0);
        let turned = add(&mut b, 40.0 + turn_east_m, -5.0);
        edge(&mut b, w, r0);
        b.add_edge(EdgeSpec::new(r0, r1).flags(edge_flags::ROUNDABOUT));
        b.add_edge(EdgeSpec::new(r1, r2).flags(edge_flags::ROUNDABOUT));
        b.add_edge(EdgeSpec::new(r2, r3).flags(edge_flags::ROUNDABOUT));
        edge(&mut b, r2, after_exit);
        edge(&mut b, after_exit, south);
        edge(&mut b, after_exit, turned);
        let (prepared, steps) = guidance(&b, &[w, r0, r1, r2, after_exit, turned], Mode::Car, &[]);
        assert_eq!(steps.len(), 3, "{steps:?}");
        assert_eq!(steps[0].maneuver, Maneuver::Right);
        assert_eq!(steps[0].at_m, node_distance(&prepared, 2));
        assert_eq!(steps[1].maneuver, expected);
        assert_eq!(steps[1].at_m, node_distance(&prepared, 3));
        assert_eq!(steps[2].maneuver, Maneuver::Arrive);
    }
}

/// 環の入口の5m手前で曲がる案内の向きに、環の曲線を合成しない。
#[test]
fn turn_five_metres_before_a_roundabout_ignores_the_roundabout_curve() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let a = add(&mut b, 0.0, 0.0);
    let east = add(&mut b, 100.0, 0.0);
    let entrance = add(&mut b, 0.0, -5.0);
    let r1 = add(&mut b, -20.0, -5.0);
    let r2 = add(&mut b, -40.0, -5.0);
    let r3 = add(&mut b, -40.0, -25.0);
    let exit = add(&mut b, -140.0, -5.0);
    edge(&mut b, w, a);
    edge(&mut b, a, east);
    edge(&mut b, a, entrance);
    b.add_edge(EdgeSpec::new(entrance, r1).flags(edge_flags::ROUNDABOUT));
    b.add_edge(EdgeSpec::new(r1, r2).flags(edge_flags::ROUNDABOUT));
    b.add_edge(EdgeSpec::new(r2, r3).flags(edge_flags::ROUNDABOUT));
    edge(&mut b, r2, exit);
    let (prepared, steps) = guidance(&b, &[w, a, entrance, r1, r2, exit], Mode::Car, &[]);
    assert_eq!(steps.len(), 3, "{steps:?}");
    assert_eq!(steps[0].maneuver, Maneuver::Right);
    assert_eq!(steps[0].at_m, node_distance(&prepared, 0));
    assert_eq!(steps[1].maneuver, Maneuver::Right);
    assert_eq!(steps[1].at_m, node_distance(&prepared, 3));
    assert_eq!(steps[2].maneuver, Maneuver::Arrive);
}

#[test]
fn t4_15_single_directed_edge_is_arrive_only_at_length() {
    let mut b = GraphBuilder::new();
    let a = add(&mut b, 0.0, 0.0);
    let c = add(&mut b, 100.0, 0.0);
    edge(&mut b, a, c);
    let (prepared, steps) = guidance(&b, &[a, c], Mode::Car, &[]);
    assert_only_arrive(&prepared, &steps);
    assert_eq!(steps[0].at_m, *prepared.distances.last().expect("頂点あり"));
}

#[test]
fn t4_16_right_turn_eight_metres_before_arrival_is_suppressed() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let a = add(&mut b, 0.0, 0.0);
    let c = add(&mut b, 0.0, -8.0);
    let branch = add(&mut b, 100.0, 0.0);
    edge(&mut b, w, a);
    edge(&mut b, a, c);
    edge(&mut b, a, branch);
    let (prepared, steps) = guidance(&b, &[w, a, c], Mode::Car, &[]);
    assert_only_arrive(&prepared, &steps);
}

#[test]
fn t7_t_junction_onto_a_one_way_road_is_right() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let c = add(&mut b, 0.0, 0.0);
    let s = add(&mut b, 0.0, -100.0);
    let n = add(&mut b, 0.0, 100.0);
    edge(&mut b, w, c);
    b.add_edge(EdgeSpec::new(n, c).oneway());
    b.add_edge(EdgeSpec::new(c, s).oneway());
    let (_, steps) = guidance(&b, &[w, c, s], Mode::Car, &[]);
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].maneuver, Maneuver::Right);
}

#[test]
fn t7_one_way_road_merging_at_a_small_angle_is_not_a_fork() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let c = add(&mut b, 0.0, 0.0);
    let e = add(&mut b, 100.0, 0.0);
    let ramp = add_polar(&mut b, 20.0, 100.0);
    edge(&mut b, w, c);
    edge(&mut b, c, e);
    b.add_edge(EdgeSpec::new(ramp, c).oneway());
    let (prepared, steps) = guidance(&b, &[w, c, e], Mode::Car, &[]);
    assert_only_arrive(&prepared, &steps);
}

#[test]
fn t7_turn_right_after_the_start_is_arrive_only() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -8.0, 0.0);
    let c = add(&mut b, 0.0, 0.0);
    let s = add(&mut b, 0.0, -100.0);
    let e = add(&mut b, 100.0, 0.0);
    edge(&mut b, w, c);
    edge(&mut b, c, s);
    edge(&mut b, c, e);
    let (prepared, steps) = guidance(&b, &[w, c, s], Mode::Car, &[]);
    assert_only_arrive(&prepared, &steps);
}

#[test]
fn t7_passing_a_ramp_on_the_main_road_is_arrive_only() {
    use accident_route_backend::domain::routing::graph::HighwayClass;
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let c = add(&mut b, 0.0, 0.0);
    let e = add(&mut b, 100.0, 0.0);
    let ramp = add_polar(&mut b, 8.0, 100.0);
    b.add_edge(EdgeSpec::new(w, c).class(HighwayClass::Motorway));
    b.add_edge(EdgeSpec::new(c, e).class(HighwayClass::Motorway));
    b.add_edge(EdgeSpec::new(c, ramp).class(HighwayClass::MotorwayLink));
    let (prepared, steps) = guidance(&b, &[w, c, e], Mode::Car, &[]);
    assert_only_arrive(&prepared, &steps);
}

#[test]
fn t7_leaving_on_a_ramp_keeps_the_side_of_the_ramp() {
    use accident_route_backend::domain::routing::graph::HighwayClass;
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let c = add(&mut b, 0.0, 0.0);
    let e = add(&mut b, 100.0, 0.0);
    let ramp = add_polar(&mut b, 8.0, 100.0);
    b.add_edge(EdgeSpec::new(w, c).class(HighwayClass::Motorway));
    b.add_edge(EdgeSpec::new(c, e).class(HighwayClass::Motorway));
    b.add_edge(EdgeSpec::new(c, ramp).class(HighwayClass::MotorwayLink));
    let (_, steps) = guidance(&b, &[w, c, ramp], Mode::Car, &[]);
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].maneuver, Maneuver::SlightRight);
}

#[test]
fn t7_straight_past_a_side_road_at_thirty_five_degrees_is_arrive_only() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let c = add(&mut b, 0.0, 0.0);
    let e = add(&mut b, 100.0, 0.0);
    let side = add_polar(&mut b, 35.0, 100.0);
    edge(&mut b, w, c);
    edge(&mut b, c, e);
    edge(&mut b, c, side);
    let (prepared, steps) = guidance(&b, &[w, c, e], Mode::Car, &[]);
    assert_only_arrive(&prepared, &steps);
}

#[test]
fn t7_fork_direction_follows_the_whole_run_at_a_jog() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let first = add(&mut b, 0.0, 0.0);
    let second = add(&mut b, 10.0, -10.0);
    let out = add_from(&mut b, second, 20.0, 100.0);
    let first_branch = add(&mut b, 0.0, 100.0);
    let straight = add_from(&mut b, second, 0.0, 100.0);
    edge(&mut b, w, first);
    edge(&mut b, first, second);
    edge(&mut b, second, out);
    edge(&mut b, first, first_branch);
    edge(&mut b, second, straight);
    let (_, steps) = guidance(&b, &[w, first, second, out], Mode::Car, &[]);
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].maneuver, Maneuver::SlightRight);
}

#[test]
fn t4_17_cancelled_token_is_interrupted() {
    let mut b = GraphBuilder::new();
    let w = add(&mut b, -100.0, 0.0);
    let c = add(&mut b, 0.0, 0.0);
    let e = add(&mut b, 100.0, 0.0);
    edge(&mut b, w, c);
    edge(&mut b, c, e);
    let prepared = prepare(&b, &[w, c, e], &[]);
    let cancel = CancelToken::unbounded();
    cancel.cancel();
    let error = build_guidance(
        &prepared.graph,
        &prepared.topology,
        &prepared.path,
        &prepared.shape,
        Mode::Car,
        &prepared.distances,
        &cancel,
    )
    .expect_err("取り消し済み");
    assert!(matches!(error, Interrupt::Cancelled), "{error:?}");
}
