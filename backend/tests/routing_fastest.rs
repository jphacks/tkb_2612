//! 吸着（形式書11章）、出発・到着の扱い（`search::endpoints`）、合法な最短時間経路（`search::fastest`）の
//! 結合試験（作業書M1.4・M1.5、behavior.md 11章「右左折禁止」「中断」）。
//!
//! 最短時間経路は、次の3つで確かめる。
//! - `legality::check_path`：出発・到着の部分有向辺を含めた並びの違反が0件。
//! - `endpoints::path_time_ds`：返す時間が正本の計算と一致。
//! - 参照実装：全体グラフで「有向辺＋照合器の状態」を状態にした素朴なDijkstra（枝刈りなし）の最短時間と一致。
//!   ノード6個前後の小さなグラフでは、有向辺の並びの全探索（`check_path` と `path_time_ds` だけで評価）とも比べる。
//!
//! 乱数を使う試験はシードを固定している（`random_graph` と `Rng` は決定的）。

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::time::{Duration, Instant};

use accident_route_backend::contract::geo::METERS_PER_DEGREE;
use accident_route_backend::contract::{CancelToken, Interrupt, LatLon, Mode};
use accident_route_backend::domain::routing::graph::restrictions::{Restrictions, TurnAutomaton};
use accident_route_backend::domain::routing::graph::synthetic::{
    ALL_MODES_MASK, EdgeSpec, GraphBuilder, RandomGraphConfig, Rng, bwd, fwd, grid, random_graph,
};
use accident_route_backend::domain::routing::graph::time::{
    PerMode, TimeModel, TurnParams, edge_time_ds, node_penalty_ds, partial_edge_time_ds,
    turn_cost_ds,
};
use accident_route_backend::domain::routing::graph::{Coord, DirEdge, Graph, NodeId, node_flags};
use accident_route_backend::domain::routing::legality::check_path;
use accident_route_backend::domain::routing::search::endpoints::{
    Endpoints, Placement, arrival_arcs, departure_arcs, direct_arc, is_same_position,
    path_distance_dm, path_time_ds, placement,
};
use accident_route_backend::domain::routing::search::fastest::{
    FastestWork, fastest_path, fastest_path_with,
};
use accident_route_backend::domain::routing::search::turn_dijkstra::{
    Leg, Pricing, TimeOnly, Transition, TurnSearch, TurnWork, turn_dijkstra,
};
use accident_route_backend::domain::routing::search::{Path, SearchContext, SearchParams};
use accident_route_backend::domain::routing::snap::{
    OdIndex, OdIndexParams, SnapPoint, point_on_edge, snap,
};
use accident_route_backend::domain::routing::topology::Topology;
use accident_route_backend::domain::routing::{Landmarks, NONE, SNAP_MAX_DISTANCE_M};

// ---------------------------------------------------------------------------------------------
// 試験用の世界：グラフ、右左折禁止、吸着先の位相、格子の索引
// ---------------------------------------------------------------------------------------------

/// 全辺を全手段で吸着可にした位相（余りのビットは0）。
fn all_snappable(graph: &Graph) -> Topology {
    let edges = graph.edge_count();
    let mut bits = vec![u64::MAX; edges.div_ceil(64)];
    if edges & 63 != 0
        && let Some(last) = bits.last_mut()
    {
        *last = (1_u64 << (edges % 64)) - 1;
    }
    let topology = Topology {
        params: serde_json::Value::Null,
        snap_car: bits.clone(),
        snap_bike: bits.clone(),
        snap_walk: bits,
        node_cluster: vec![NONE; graph.node_count()],
        clusters: vec![],
        cluster_nodes: vec![],
        twins: vec![],
        sidewalk_parents: vec![],
    };
    topology.validate(graph).expect("位相が不正");
    topology
}

fn clear_snap_bit(bits: &mut [u64], edge: u32) {
    bits[edge as usize / 64] &= !(1_u64 << (edge % 64));
}

/// 吸着用の格子。各辺を、折れ線の外接矩形に重なるセルすべてに登録した上位集合
/// （形式書11章の「線分が通るセル」の上位集合。`snap` は距離で絞るので結果は同じ）。
fn build_index(graph: &Graph) -> OdIndex {
    let params = OdIndexParams {
        lat0_e7: 200_000_000,
        lon0_e7: 1_220_000_000,
        cell_lat_e7: 25_000,
        cell_lon_e7: 25_000,
        rows: 10_401,
        cols: 12_801,
    };
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    for e in 0..graph.edge_count() as u32 {
        let line = graph.edge_polyline(e);
        let lat = line.iter().map(|c| c.lat_e7);
        let lon = line.iter().map(|c| c.lon_e7);
        let (lat_lo, lat_hi) = (lat.clone().min().unwrap(), lat.max().unwrap());
        let (lon_lo, lon_hi) = (lon.clone().min().unwrap(), lon.max().unwrap());
        let row = |x: i32| ((x - params.lat0_e7) / params.cell_lat_e7) as u32;
        let col = |x: i32| ((x - params.lon0_e7) / params.cell_lon_e7) as u32;
        for r in row(lat_lo)..=row(lat_hi) {
            for c in col(lon_lo)..=col(lon_hi) {
                pairs.push((r * params.cols + c, e));
            }
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    let mut cell_keys = Vec::new();
    let mut cell_ofs = Vec::new();
    let mut cell_edges = Vec::new();
    for (key, e) in pairs {
        if cell_keys.last() != Some(&key) {
            cell_keys.push(key);
            cell_ofs.push(cell_edges.len() as u32);
        }
        cell_edges.push(e);
    }
    cell_ofs.push(cell_edges.len() as u32);
    OdIndex {
        params,
        raw_params: serde_json::Value::Null,
        cell_keys,
        cell_ofs,
        cell_edges,
    }
}

struct World {
    graph: Graph,
    restrictions: Restrictions,
    topology: Topology,
    index: OdIndex,
    landmarks: Landmarks,
    automata: Vec<(Mode, TurnAutomaton)>,
}

impl World {
    fn new(builder: &GraphBuilder) -> World {
        let (graph, restrictions) = builder.build_all();
        let topology = all_snappable(&graph);
        let index = build_index(&graph);
        let automata = Mode::ALL
            .into_iter()
            .map(|m| (m, TurnAutomaton::build(&restrictions, m)))
            .collect();
        World {
            graph,
            restrictions,
            topology,
            index,
            landmarks: Landmarks::empty(),
            automata,
        }
    }

    fn automaton(&self, mode: Mode) -> &TurnAutomaton {
        &self.automata.iter().find(|(m, _)| *m == mode).unwrap().1
    }

    fn ctx(&self, mode: Mode) -> SearchContext<'_> {
        self.ctx_with(mode, SearchParams::DEFAULT)
    }

    fn ctx_with(&self, mode: Mode, params: SearchParams) -> SearchContext<'_> {
        SearchContext {
            graph: &self.graph,
            topology: &self.topology,
            automaton: self.automaton(mode),
            landmarks: &self.landmarks,
            mode,
            params,
        }
    }

    fn fastest(&self, mode: Mode, s: &SnapPoint, e: &SnapPoint) -> Option<Path> {
        fastest_path(&self.ctx(mode), s, e, &CancelToken::unbounded()).unwrap()
    }

    fn point(&self, edge: u32, offset_dm: u32) -> SnapPoint {
        SnapPoint::on_edge(&self.graph, edge, offset_dm)
    }
}

/// 右左折・信号などのコストを0でない値にした時間のモデル（`TimeModel::synthetic` は全部0）。
fn penalized_model() -> TimeModel {
    let mut model = TimeModel::synthetic();
    let turn = |right_s, left_s, u_turn_s| TurnParams {
        right_s,
        left_s,
        u_turn_s,
        straight_max_deg: 30.0,
        u_turn_min_deg: 150.0,
    };
    model.signal_penalty_s = PerMode {
        walk: 20.0,
        bike: 15.0,
        car: 30.0,
    };
    model.stop_penalty_s = PerMode {
        walk: 0.0,
        bike: 5.0,
        car: 8.0,
    };
    model.level_crossing_penalty_s = PerMode {
        walk: 10.0,
        bike: 20.0,
        car: 40.0,
    };
    model.turn = PerMode {
        walk: turn(2.0, 1.0, 5.0),
        bike: turn(4.0, 2.0, 10.0),
        car: turn(6.0, 3.0, 20.0),
    };
    model
}

/// 点を、基準の座標から北・東へ何mか動かした位置。
fn moved(from: Coord, north_m: f64, east_m: f64) -> LatLon {
    let origin = from.to_latlon();
    LatLon::new(
        origin.lat + north_m / METERS_PER_DEGREE,
        origin.lon + east_m / (METERS_PER_DEGREE * origin.lat.to_radians().cos()),
    )
}

// ---------------------------------------------------------------------------------------------
// 吸着（形式書11章）
// ---------------------------------------------------------------------------------------------

#[test]
fn snap_picks_the_nearest_edge_and_reports_the_position() {
    let (b, shape) = grid(3, 3, 100.0);
    let w = World::new(&b);
    let g = &w.graph;
    let west = shape.node(1, 0);
    let east = shape.node(1, 1);
    let expected = b.dedge(west, east).edge();
    // 辺の真ん中の30m北：他の辺は50m以上離れている。
    let average = |x: i32, y: i32| ((i64::from(x) + i64::from(y)) / 2) as i32;
    let mid = Coord::new(
        average(g.node_coord(west).lat_e7, g.node_coord(east).lat_e7),
        average(g.node_coord(west).lon_e7, g.node_coord(east).lon_e7),
    );
    let p = snap(g, &w.topology, &w.index, moved(mid, 30.0, 0.0), Mode::Walk).expect("吸着できる");
    assert_eq!(p.edge, expected);
    assert!(
        (p.offset_dm as i64 - 500).abs() <= 2,
        "辺の真ん中のはず: {}",
        p.offset_dm
    );
    assert!((p.distance_m - 30.0).abs() < 0.1, "{}", p.distance_m);
    // 吸着した点は辺上にあり、中点に近い。
    assert!((p.position.lat_e7 - mid.lat_e7).abs() <= 2);
    assert!((p.position.lon_e7 - mid.lon_e7).abs() <= 2);

    // 辺の始点側の端より手前の点は、端のノードに吸着する（offset 0 か length_dm）。
    let node = g.node_coord(shape.node(0, 0));
    let p = snap(
        g,
        &w.topology,
        &w.index,
        moved(node, -20.0, -20.0),
        Mode::Car,
    )
    .expect("角のノードの外側");
    assert_eq!(placement(g, &p), Some(Placement::AtNode(shape.node(0, 0))));
    assert!((p.distance_m - 28.28).abs() < 0.1, "{}", p.distance_m);
}

#[test]
fn snap_breaks_ties_by_the_smaller_edge_number() {
    let (b, shape) = grid(3, 3, 100.0);
    let w = World::new(&b);
    let g = &w.graph;
    // ちょうどノードの上：そのノードに接する辺すべてが距離0。
    let center = shape.node(1, 1);
    let smallest = (0..g.edge_count() as u32)
        .filter(|&e| g.edge(e).tail == center || g.edge(e).head == center)
        .min()
        .unwrap();
    let p = snap(
        g,
        &w.topology,
        &w.index,
        g.node_coord(center).to_latlon(),
        Mode::Car,
    )
    .unwrap();
    assert_eq!(p.edge, smallest);
    assert_eq!(p.distance_m, 0.0);

    // 向かい合う2本の平行な辺の真ん中：辺の位置によらず、番号の小さい方。
    for swap in [false, true] {
        let mut b = GraphBuilder::new();
        let lat = 356_000_000;
        let lon = 1_397_000_000;
        let up = |b: &mut GraphBuilder| {
            let x = b.add_node(Coord::new(lat + 18_000, lon));
            let y = b.add_node(Coord::new(lat + 18_000, lon + 20_000));
            b.add_road(x, y)
        };
        let down = |b: &mut GraphBuilder| {
            let x = b.add_node(Coord::new(lat, lon));
            let y = b.add_node(Coord::new(lat, lon + 20_000));
            b.add_road(x, y)
        };
        let (first, second) = if swap {
            (down(&mut b), up(&mut b))
        } else {
            (up(&mut b), down(&mut b))
        };
        assert_eq!((first, second), (0, 1));
        let w = World::new(&b);
        let mid = LatLon::new(f64::from(lat + 9_000) / 1e7, f64::from(lon + 10_000) / 1e7);
        let p = snap(&w.graph, &w.topology, &w.index, mid, Mode::Walk).unwrap();
        assert_eq!(p.edge, 0, "同じ距離なら辺番号の小さい方（swap={swap}）");
    }
}

#[test]
fn snap_fails_beyond_the_distance_limit() {
    assert_eq!(SNAP_MAX_DISTANCE_M, 300.0);
    let mut b = GraphBuilder::new();
    let a = b.add_node_deg(35.6, 139.7);
    let c = b.add_node_deg(35.6, 139.702);
    b.add_road(a, c);
    let w = World::new(&b);
    let mid = Coord::new(356_000_000, 1_397_010_000);
    for (north, ok) in [(250.0, true), (299.0, true), (301.0, false), (350.0, false)] {
        let p = snap(
            &w.graph,
            &w.topology,
            &w.index,
            moved(mid, north, 0.0),
            Mode::Bike,
        );
        assert_eq!(p.is_some(), ok, "{north}m");
        if let Some(p) = p {
            assert!(p.distance_m <= SNAP_MAX_DISTANCE_M);
            assert!((p.distance_m - north).abs() < 0.2);
        }
    }
    // 辺から遠いだけでなく、格子の外（索引の範囲外）も失敗する。
    let far = LatLon::new(19.0, 121.0);
    assert!(snap(&w.graph, &w.topology, &w.index, far, Mode::Car).is_none());
    assert!(
        snap(
            &w.graph,
            &w.topology,
            &w.index,
            LatLon::new(f64::NAN, 139.7),
            Mode::Car
        )
        .is_none()
    );
}

#[test]
fn snap_skips_edges_that_are_not_snappable_or_not_passable() {
    // 3本の平行な辺：1本目が最寄り（20m）、2本目が次（80m）、3本目が遠い（150m）。
    let mut b = GraphBuilder::new();
    let lon = 1_397_000_000;
    for north_e7 in [0, 7_000, 13_500] {
        let x = b.add_node(Coord::new(356_000_000 + north_e7, lon));
        let y = b.add_node(Coord::new(356_000_000 + north_e7, lon + 20_000));
        b.add_road(x, y);
    }
    // 点は1本目の2m南。1本目が最寄りで、2本目（約78m）、3本目（約150m）の順。
    let point = LatLon::new(f64::from(356_000_000 - 180) / 1e7, 139.7 + 0.001);
    let near = |w: &World, mode| snap(&w.graph, &w.topology, &w.index, point, mode);

    let w = World::new(&b);
    for mode in Mode::ALL {
        assert_eq!(near(&w, mode).unwrap().edge, 0);
    }

    // SNAPのビットを手段ごとに落とす：その手段だけ次の辺に移る。
    let mut w = World::new(&b);
    clear_snap_bit(&mut w.topology.snap_car, 0);
    assert_eq!(near(&w, Mode::Car).unwrap().edge, 1);
    assert_eq!(near(&w, Mode::Bike).unwrap().edge, 0);
    assert_eq!(near(&w, Mode::Walk).unwrap().edge, 0);
    clear_snap_bit(&mut w.topology.snap_car, 1);
    assert_eq!(near(&w, Mode::Car).unwrap().edge, 2);
    clear_snap_bit(&mut w.topology.snap_car, 2);
    assert!(near(&w, Mode::Car).is_none(), "吸着できる辺が1本もない");

    // その手段で通れない辺（ビットが立っていても）は選ばない。
    let mut b2 = b.clone();
    *b2.edge_spec_mut(0) = EdgeSpec::new(0, 1).deny(Mode::Car).deny(Mode::Walk);
    *b2.edge_spec_mut(1) = EdgeSpec::new(2, 3).deny(Mode::Walk);
    let w = World::new(&b2);
    assert_eq!(near(&w, Mode::Car).unwrap().edge, 1, "carは0を通れない");
    assert_eq!(
        near(&w, Mode::Walk).unwrap().edge,
        2,
        "walkは0も1も通れない"
    );
    assert_eq!(near(&w, Mode::Bike).unwrap().edge, 0);
    // 一方通行は、少なくとも一方向に通れるので吸着の対象。
    let mut b3 = b.clone();
    *b3.edge_spec_mut(0) = EdgeSpec::new(0, 1).oneway();
    let w = World::new(&b3);
    assert_eq!(near(&w, Mode::Car).unwrap().edge, 0);
}

#[test]
fn snap_offset_scales_with_the_edge_length() {
    // 長さを手で指定した辺（幾何の長さと1dm以上違う）でも、offsetは0〜length_dmの割合になる。
    let mut b = GraphBuilder::new();
    let a = b.add_node(Coord::new(356_000_000, 1_397_000_000));
    let c = b.add_node(Coord::new(356_000_000, 1_397_100_000));
    b.add_edge(EdgeSpec::new(a, c).length_dm(2000));
    let w = World::new(&b);
    let at = |t: f64| {
        let lon = 1_397_000_000.0 + 100_000.0 * t;
        snap(
            &w.graph,
            &w.topology,
            &w.index,
            LatLon::new(35.6 + 0.0001, lon / 1e7),
            Mode::Car,
        )
        .unwrap()
    };
    assert_eq!(at(0.0).offset_dm, 0);
    assert!((at(0.25).offset_dm as i64 - 500).abs() <= 1);
    assert!((at(0.5).offset_dm as i64 - 1000).abs() <= 1);
    assert_eq!(at(1.0).offset_dm, 2000);
    // 辺の外側へずれた点は端（0か length_dm）に切られる。
    assert_eq!(at(-0.2).offset_dm, 0);
    assert_eq!(at(1.2).offset_dm, 2000);
    // point_on_edge は offset と逆の対応。
    let p = at(0.25);
    let q = point_on_edge(&w.graph, 0, p.offset_dm);
    assert!((q.lon_e7 - p.position.lon_e7).abs() <= 60, "{q:?} {p:?}");
    assert_eq!(point_on_edge(&w.graph, 0, 0), w.graph.node_coord(a));
    assert_eq!(point_on_edge(&w.graph, 0, 2000), w.graph.node_coord(c));
}

// ---------------------------------------------------------------------------------------------
// 出発・到着の扱い（endpoints）
// ---------------------------------------------------------------------------------------------

/// 一直線に東へ並ぶ3ノード a - b - c（辺は各100m = 1000dm、30km/h なので全長120デシ秒）。
fn line3() -> (GraphBuilder, [NodeId; 3]) {
    let mut b = GraphBuilder::new();
    let n: Vec<NodeId> = (0..3)
        .map(|i| b.add_node(Coord::new(356_000_000, 1_397_000_000 + 11_000 * i)))
        .collect();
    b.add_edge(EdgeSpec::new(n[0], n[1]).length_dm(1000));
    b.add_edge(EdgeSpec::new(n[1], n[2]).length_dm(1000));
    (b, [n[0], n[1], n[2]])
}

#[test]
fn partial_arcs_for_a_point_inside_an_edge() {
    let (b, _) = line3();
    let w = World::new(&b);
    let g = &w.graph;
    let p = w.point(0, 400);
    assert_eq!(placement(g, &p), Some(Placement::Inside));

    let dep = departure_arcs(g, Mode::Car, &p);
    assert_eq!(dep.len(), 2);
    // 向き0：吸着点から終点まで 600dm（72デシ秒）。向き1：始点まで 400dm（48デシ秒）。
    assert_eq!(
        (dep[0].dedge, dep[0].travelled_dm, dep[0].time_ds),
        (fwd(0), 600, 72)
    );
    assert_eq!((dep[0].from, dep[0].to), (0, 1));
    assert_eq!(
        (dep[1].dedge, dep[1].travelled_dm, dep[1].time_ds),
        (bwd(0), 400, 48)
    );
    assert_eq!((dep[1].from, dep[1].to), (1, 0));

    // 到着：向き0は始点から吸着点まで 400dm、向き1は終点から 600dm。
    let arr = arrival_arcs(g, Mode::Car, &p);
    assert_eq!(arr.len(), 2);
    assert_eq!(
        (arr[0].dedge, arr[0].travelled_dm, arr[0].time_ds),
        (fwd(0), 400, 48)
    );
    assert_eq!((arr[0].from, arr[0].to), (0, 1));
    assert_eq!(
        (arr[1].dedge, arr[1].travelled_dm, arr[1].time_ds),
        (bwd(0), 600, 72)
    );
    // 時間は partial_edge_time_ds と同じ。
    assert_eq!(partial_edge_time_ds(g, fwd(0), Mode::Car, 600), Some(72));
}

#[test]
fn partial_arcs_respect_one_way_edges_and_nodes() {
    let (mut b, [a, c, _]) = line3();
    *b.edge_spec_mut(0) = EdgeSpec::new(a, c).length_dm(1000).oneway();
    let w = World::new(&b);
    let g = &w.graph;
    let p = w.point(0, 400);
    // carは向き0にだけ出られ、向き0からだけ着ける。walkは両方向。
    assert_eq!(departure_arcs(g, Mode::Car, &p).len(), 1);
    assert_eq!(departure_arcs(g, Mode::Car, &p)[0].dedge, fwd(0));
    assert_eq!(arrival_arcs(g, Mode::Car, &p).len(), 1);
    assert_eq!(arrival_arcs(g, Mode::Car, &p)[0].dedge, fwd(0));
    assert_eq!(departure_arcs(g, Mode::Walk, &p).len(), 2);

    // ノード上（offset 0）：出発はそのノードから出る有向辺を全長。到着側の部分有向辺は空。
    let at_b = w.point(0, 1000);
    assert_eq!(placement(g, &at_b), Some(Placement::AtNode(c)));
    let dep = departure_arcs(g, Mode::Walk, &at_b);
    let dedges: Vec<DirEdge> = dep.iter().map(|d| d.dedge).collect();
    assert_eq!(dedges, vec![bwd(0), fwd(1)]);
    assert!(
        dep.iter()
            .all(|d| d.time_ds == 750 && d.travelled_dm == 1000)
    ); // walk 4.8km/h: 100mで75秒
    assert!(arrival_arcs(g, Mode::Walk, &at_b).is_empty());
    assert_eq!(placement(g, &w.point(0, 0)), Some(Placement::AtNode(a)));
    // 不正な吸着点。
    let bad = SnapPoint {
        offset_dm: 1001,
        ..w.point(0, 1000)
    };
    assert_eq!(placement(g, &bad), None);
    assert!(departure_arcs(g, Mode::Car, &bad).is_empty());
}

#[test]
fn direct_arc_and_same_position() {
    let (mut b, [a, c, _]) = line3();
    let w = World::new(&b);
    let g = &w.graph;
    let s = w.point(0, 300);
    let t = w.point(0, 800);
    // 到着が終点側なら向き0で500dm、始点側なら向き1。
    let d = direct_arc(g, Mode::Car, &s, &t).unwrap();
    assert_eq!((d.dedge, d.travelled_dm, d.time_ds), (fwd(0), 500, 60));
    let d = direct_arc(g, Mode::Car, &t, &s).unwrap();
    assert_eq!((d.dedge, d.travelled_dm), (bwd(0), 500));
    // 同じ辺でないもの・ノード上のものは直接行けない（普通の探索で扱う）。
    assert!(direct_arc(g, Mode::Car, &s, &w.point(1, 500)).is_none());
    assert!(direct_arc(g, Mode::Car, &s, &w.point(0, 1000)).is_none());
    // 一方通行の逆向きには行けない。
    *b.edge_spec_mut(0) = EdgeSpec::new(a, c).length_dm(1000).oneway();
    let w1 = World::new(&b);
    let (s1, t1) = (w1.point(0, 300), w1.point(0, 800));
    assert!(direct_arc(&w1.graph, Mode::Car, &s1, &t1).is_some());
    assert!(direct_arc(&w1.graph, Mode::Car, &t1, &s1).is_none());
    assert!(direct_arc(&w1.graph, Mode::Walk, &t1, &s1).is_some());

    // 同じ位置：同じ辺の同じoffset、または同じノードの上（辺は違ってよい）。
    assert!(is_same_position(g, &s, &w.point(0, 300)));
    assert!(!is_same_position(g, &s, &w.point(0, 301)));
    assert!(is_same_position(g, &w.point(0, 1000), &w.point(1, 0)));
    assert!(!is_same_position(g, &w.point(0, 1000), &w.point(1, 1)));
    assert!(Endpoints::new(g, Mode::Car, &s, &s).is_none());
    assert!(Endpoints::new(g, Mode::Car, &w.point(0, 1000), &w.point(1, 0)).is_none());
    assert!(Endpoints::new(g, Mode::Car, &s, &t).is_some());
    let bad = SnapPoint { edge: 99, ..s };
    assert!(!is_same_position(g, &bad, &bad));
    assert!(Endpoints::new(g, Mode::Car, &bad, &t).is_none());
}

#[test]
fn path_time_and_distance_follow_the_format_document() {
    // 東へ a→b、北へ b→c。b は信号（30秒）。a→b→c は左折（3秒）。辺は各100m = 1000dm（120デシ秒）。
    let mut b = GraphBuilder::new();
    b.set_time_model(penalized_model());
    let a = b.add_node(Coord::new(356_000_000, 1_397_000_000));
    let bb = b.add_node(Coord::new(356_000_000, 1_397_011_000));
    let c = b.add_node(Coord::new(356_009_000, 1_397_011_000));
    b.add_edge(EdgeSpec::new(a, bb).length_dm(1000));
    b.add_edge(EdgeSpec::new(bb, c).length_dm(1000));
    b.add_node_flags(bb, node_flags::SIGNAL);
    let w = World::new(&b);
    let g = &w.graph;
    let s = w.point(0, 400); // 出発：辺0の400dm地点
    let t = w.point(1, 250); // 到着：辺1の250dm地点
    let path = [fwd(0), fwd(1)];
    // 600dm（72）＋250dm（30）＋信号300＋左折30 = 432デシ秒。
    assert_eq!(partial_edge_time_ds(g, fwd(0), Mode::Car, 600), Some(72));
    assert_eq!(partial_edge_time_ds(g, fwd(1), Mode::Car, 250), Some(30));
    assert_eq!(node_penalty_ds(g, bb, Mode::Car), 300);
    assert_eq!(turn_cost_ds(g, fwd(0), fwd(1), Mode::Car), 30);
    assert_eq!(path_time_ds(g, Mode::Car, &s, &t, &path), Some(432));
    assert_eq!(path_distance_dm(g, &s, &t, &path), Some(850));
    // bikeは15km/h（600dm = 144、250dm = 60）、信号150、左折20 = 374。
    assert_eq!(path_time_ds(g, Mode::Bike, &s, &t, &path), Some(374));

    // 最初と最後の有向辺の間の「途中のノード」だけに加算が掛かる：1本で終わる経路には掛からない。
    let t0 = w.point(0, 900);
    assert_eq!(path_time_ds(g, Mode::Car, &s, &t0, &[fwd(0)]), Some(60));
    assert_eq!(path_distance_dm(g, &s, &t0, &[fwd(0)]), Some(500));
    // ノード上の到着（辺1の終点 c）：最後の有向辺は全長。
    let at_c = w.point(1, 1000);
    assert_eq!(
        path_time_ds(g, Mode::Car, &s, &at_c, &path),
        Some(72 + 120 + 300 + 30)
    );
    // ノード上の出発（a）：最初の有向辺は全長で、その前に何も付かない。
    let at_a = w.point(0, 0);
    assert_eq!(
        path_time_ds(g, Mode::Car, &at_a, &t, &path),
        Some(120 + 30 + 300 + 30)
    );

    // 形が正しくない経路は None：つながっていない、出発の辺でない、向きが逆で通る長さが負、空、通れない向き。
    assert_eq!(path_time_ds(g, Mode::Car, &s, &t, &[fwd(1)]), None);
    assert_eq!(path_time_ds(g, Mode::Car, &s, &t, &[fwd(0)]), None);
    assert_eq!(path_time_ds(g, Mode::Car, &s, &t, &[]), None);
    assert_eq!(path_time_ds(g, Mode::Car, &s, &t, &[fwd(0), bwd(1)]), None);
    assert_eq!(path_time_ds(g, Mode::Car, &t, &s, &[fwd(0)]), None);
    // 同じ辺の途中から途中へ：前へ進む有向辺だけが正しい。
    let (s2, t2) = (w.point(0, 700), w.point(0, 200));
    assert_eq!(path_time_ds(g, Mode::Car, &s2, &t2, &[fwd(0)]), None);
    assert_eq!(path_time_ds(g, Mode::Car, &s2, &t2, &[bwd(0)]), Some(60));
}

// ---------------------------------------------------------------------------------------------
// 最短時間経路：手で組んだ場合
// ---------------------------------------------------------------------------------------------

fn assert_legal(w: &World, mode: Mode, path: &Path) {
    let violations = check_path(&w.graph, &w.restrictions, mode, &path.dedges);
    assert!(
        violations.is_empty(),
        "違反 {violations:?}: {:?}",
        path.dedges
    );
    assert_eq!(
        path_time_ds(&w.graph, mode, &path.start, &path.end, &path.dedges),
        Some(path.time_ds),
        "time_dsがpath_time_dsと食い違う"
    );
    assert_eq!(path.risk_fx, 0);
}

/// 2×3の梯子：下の列 A-B-C、上の列 D-E-F、各段 A-D・B-E・C-F。辺は各100m。
/// ノード番号は A=0, B=1, C=2, D=3, E=4, F=5。
fn ladder() -> (GraphBuilder, [NodeId; 6]) {
    let mut b = GraphBuilder::new();
    let mut n = Vec::new();
    for lat in [356_000_000, 356_009_000] {
        for i in 0..3 {
            n.push(b.add_node(Coord::new(lat, 1_397_000_000 + 11_000 * i)));
        }
    }
    let mut road = |x: usize, y: usize| b.add_edge(EdgeSpec::new(n[x], n[y]).length_dm(1000));
    road(0, 1); // e0: A-B
    road(1, 2); // e1: B-C
    road(3, 4); // e2: D-E
    road(4, 5); // e3: E-F
    road(0, 3); // e4: A-D
    road(1, 4); // e5: B-E
    road(2, 5); // e6: C-F
    (b, [n[0], n[1], n[2], n[3], n[4], n[5]])
}

#[test]
fn via_node_restriction_makes_the_path_take_a_detour() {
    let (mut b, [a, bb, c, _, e, f]) = ladder();
    let w0 = World::new(&b);
    let (s, t) = (w0.point(0, 500), w0.point(1, 500)); // A-Bの中点 → B-Cの中点
    for mode in Mode::ALL {
        let free = w0.fastest(mode, &s, &t).unwrap();
        assert_eq!(free.dedges, vec![fwd(0), fwd(1)], "{mode:?}");
        assert_eq!(
            free.time_ds,
            path_time_ds(&w0.graph, mode, &s, &t, &free.dedges).unwrap()
        );
    }

    // B で A→B→C を禁止（via-node型）。合成グラフのUターンのコストは0なので、Eまで上がって戻る
    // （B→E→B のUターン）経路が最短になる。
    b.forbid_nodes(&[a, bb, c], ALL_MODES_MASK);
    let w = World::new(&b);
    let (s, t) = (w.point(0, 500), w.point(1, 500));
    assert_eq!(w.restrictions.items.len(), 1);
    for mode in Mode::ALL {
        let free = w0
            .fastest(mode, &w0.point(0, 500), &w0.point(1, 500))
            .unwrap();
        let detour = w.fastest(mode, &s, &t).unwrap();
        assert_legal(&w, mode, &detour);
        assert!(detour.time_ds > free.time_ds, "{mode:?}: 遠回りになる");
        assert_ne!(detour.dedges, free.dedges);
        // 禁止された列を含まない（隣り合う fwd(0), fwd(1)）。
        assert!(!detour.dedges.windows(2).any(|p| p == [fwd(0), fwd(1)]));
        assert_eq!(
            detour.dedges,
            vec![fwd(0), fwd(5), bwd(5), fwd(1)],
            "{mode:?}"
        );
        assert_eq!(
            Some(detour.time_ds),
            reference_time(&w, mode, &s, &t),
            "{mode:?}"
        );
        // 禁止がなければ使う方が速い配置であること：禁止なしの経路は禁止の列そのもの。
        let violations = check_path(&w.graph, &w.restrictions, mode, &free.dedges);
        assert!(!violations.is_empty());
    }

    // Eでの折り返し（B→E→B）も禁止すると、梯子を一周する5本の有向辺になる：
    // Eを回ってCから戻って到着するか、Aへ戻って回りBから到着する（同じ時間）。
    b.forbid_nodes(&[bb, e, bb], ALL_MODES_MASK);
    let w = World::new(&b);
    let (s, t) = (w.point(0, 500), w.point(1, 500));
    for mode in Mode::ALL {
        let around = w.fastest(mode, &s, &t).unwrap();
        assert_legal(&w, mode, &around);
        assert_eq!(around.dedges.len(), 5, "{mode:?}: {:?}", around.dedges);
        assert!(!around.dedges.windows(2).any(|p| p == [fwd(5), bwd(5)]));
        assert_eq!(
            Some(around.time_ds),
            reference_time(&w, mode, &s, &t),
            "{mode:?}"
        );
        let uturn = w.fastest(mode, &s, &t).unwrap();
        assert!(
            uturn.time_ds
                > w0.fastest(mode, &w0.point(0, 500), &w0.point(1, 500))
                    .unwrap()
                    .time_ds
        );
    }

    // 禁止の対象でない手段には効かない。
    let mut b2 = ladder().0;
    b2.forbid_nodes(&[a, bb, c], Mode::Car.mask());
    let w2 = World::new(&b2);
    let walk = w2
        .fastest(Mode::Walk, &w2.point(0, 500), &w2.point(1, 500))
        .unwrap();
    assert_eq!(walk.dedges, vec![fwd(0), fwd(1)]);
    let car = w2
        .fastest(Mode::Car, &w2.point(0, 500), &w2.point(1, 500))
        .unwrap();
    assert_ne!(car.dedges, walk.dedges);
    let _ = f;
}

#[test]
fn via_way_restriction_makes_the_path_take_a_detour() {
    let (mut b, [a, bb, c, _, e, f]) = ladder();
    // A-Bの中点 → C-Fの中点。直進する A→B→C→F（C-Fの途中まで）は短い。
    let w0 = World::new(&b);
    let (s, t) = (w0.point(0, 500), w0.point(6, 500));
    let free = w0.fastest(Mode::Car, &s, &t).unwrap();
    assert_eq!(free.dedges, vec![fwd(0), fwd(1), fwd(6)]);

    // 辺 A→B, B→C, C→F の連続を禁止（via-way型：長さ3の列）。途中の組(A→B→C, B→C→F)は許される。
    b.forbid_nodes(&[a, bb, c, f], ALL_MODES_MASK);
    let w = World::new(&b);
    assert_eq!(w.restrictions.items.len(), 1);
    assert_eq!(w.restrictions.items[0].seq.len(), 3);
    for mode in Mode::ALL {
        let free = w0
            .fastest(mode, &w0.point(0, 500), &w0.point(6, 500))
            .unwrap();
        assert!(!check_path(&w.graph, &w.restrictions, mode, &free.dedges).is_empty());
        let (s, t) = (w.point(0, 500), w.point(6, 500));
        let detour = w.fastest(mode, &s, &t).unwrap();
        assert_legal(&w, mode, &detour);
        assert!(detour.time_ds > free.time_ds, "{mode:?}");
        // 列 fwd(0), fwd(1), fwd(6) は現れない。一部（A→B→C だけ）は禁止ではない。
        assert!(
            !detour
                .dedges
                .windows(3)
                .any(|p| p == [fwd(0), fwd(1), fwd(6)])
        );
        assert_eq!(
            detour.dedges,
            vec![fwd(0), fwd(5), fwd(3), bwd(6)],
            "{mode:?}: B→E→F→(戻って到着)"
        );
    }
    let _ = e;
}

#[test]
fn only_restriction_forces_the_allowed_turn() {
    // B で A→B から先は E にしか行けない（only型）。
    let (mut b, [a, bb, c, _, e, f]) = ladder();
    b.only_nodes(&[a, bb], e, ALL_MODES_MASK);
    let w = World::new(&b);
    let (s, t) = (w.point(0, 500), w.point(1, 500));
    let path = w.fastest(Mode::Car, &s, &t).unwrap();
    assert_legal(&w, Mode::Car, &path);
    // 直進（B→C）も、戻る（B→A）も禁止。Eへ上がって折り返し、Bから到着する。
    assert_eq!(path.dedges, vec![fwd(0), fwd(5), bwd(5), fwd(1)]);
    assert_eq!(path.time_ds, 360);
    assert_eq!(Some(path.time_ds), reference_time(&w, Mode::Car, &s, &t));
    let _ = (c, f);
}

#[test]
fn one_way_edge_requires_a_loop_when_the_target_is_behind() {
    // 一方通行の環状の道：A→B→C→D→A。
    let mut b = GraphBuilder::new();
    let lat = 356_000_000;
    let lon = 1_397_000_000;
    let corners = [
        Coord::new(lat, lon),
        Coord::new(lat, lon + 11_000),
        Coord::new(lat + 9_000, lon + 11_000),
        Coord::new(lat + 9_000, lon),
    ];
    let n: Vec<NodeId> = corners.iter().map(|&c| b.add_node(c)).collect();
    for i in 0..4 {
        b.add_edge(EdgeSpec::new(n[i], n[(i + 1) % 4]).length_dm(1000).oneway());
    }
    let w = World::new(&b);
    let (s, t) = (w.point(0, 700), w.point(0, 300));

    // car・bike：逆向きには行けないので、環を1周する。
    for mode in [Mode::Car, Mode::Bike] {
        let path = w.fastest(mode, &s, &t).unwrap();
        assert_legal(&w, mode, &path);
        assert_eq!(
            path.dedges,
            vec![fwd(0), fwd(1), fwd(2), fwd(3), fwd(0)],
            "{mode:?}"
        );
        // 通った長さ：300 + 1000×3 + 300 = 3600dm。
        assert_eq!(path_distance_dm(&w.graph, &s, &t, &path.dedges), Some(3600));
    }
    // walk：一方通行に従わないので、逆向きに400dmで直接行ける。
    let walk = w.fastest(Mode::Walk, &s, &t).unwrap();
    assert_eq!(walk.dedges, vec![bwd(0)]);
    assert_eq!(path_distance_dm(&w.graph, &s, &t, &walk.dedges), Some(400));
    assert_eq!(
        walk.time_ds,
        partial_edge_time_ds(&w.graph, bwd(0), Mode::Walk, 400).unwrap() as u64
    );

    // 順方向（出発が手前）は1本の有向辺で直接行く。
    let direct = w.fastest(Mode::Car, &t, &s).unwrap();
    assert_eq!(direct.dedges, vec![fwd(0)]);
    assert_legal(&w, Mode::Car, &direct);
    assert_eq!(
        path_distance_dm(&w.graph, &t, &s, &direct.dedges),
        Some(400)
    );
}

#[test]
fn same_edge_goes_around_when_the_loop_is_shorter_than_the_edge() {
    // 長い辺(4km)の途中から途中へ。辺に沿って進むより、短い別の道（ループ）で回る方が速い配置。
    let mut b = GraphBuilder::new();
    let lat = 356_000_000;
    let lon = 1_397_000_000;
    let a = b.add_node(Coord::new(lat, lon));
    let c = b.add_node(Coord::new(lat, lon + 11_000));
    b.add_edge(EdgeSpec::new(a, c).length_dm(40_000)); // e0：折れ曲がった4km（幾何は100mだが長さを指定）
    b.add_edge(EdgeSpec::new(a, c).length_dm(1000)); // e1：100m
    let w = World::new(&b);
    // 辺0の 10%（4,000dm）地点と 90%（36,000dm）地点：直接なら 32,000dm。
    // 回る：4,000 + 1,000（e1）+ 4,000 = 9,000dm のほうが速い。
    let (s, t) = (w.point(0, 4000), w.point(0, 36_000));
    let direct = direct_arc(&w.graph, Mode::Car, &s, &t).unwrap();
    assert_eq!(direct.travelled_dm, 32_000);
    let path = w.fastest(Mode::Car, &s, &t).unwrap();
    assert_legal(&w, Mode::Car, &path);
    assert_eq!(path.dedges, vec![bwd(0), fwd(1), bwd(0)]);
    assert_eq!(
        path_distance_dm(&w.graph, &s, &t, &path.dedges),
        Some(4000 + 1000 + 4000)
    );
    assert!(path.time_ds < u64::from(direct.time_ds));
}

#[test]
fn start_or_end_on_a_node_has_no_phantom_partial_edge() {
    // 出発が信号のノード（b）の上にあるとき、その信号の加算は掛からない（実際には通過しない）。
    let mut b = GraphBuilder::new();
    b.set_time_model(penalized_model());
    let (a, bb, c) = {
        let lat = 356_000_000;
        let lon = 1_397_000_000;
        (
            b.add_node(Coord::new(lat, lon)),
            b.add_node(Coord::new(lat, lon + 11_000)),
            b.add_node(Coord::new(lat, lon + 22_000)),
        )
    };
    b.add_edge(EdgeSpec::new(a, bb).length_dm(1000));
    b.add_edge(EdgeSpec::new(bb, c).length_dm(1000));
    b.add_node_flags(bb, node_flags::SIGNAL);
    let w = World::new(&b);
    // 出発：辺0の終点（= b）。到着：辺1の途中。
    let (s, t) = (w.point(0, 1000), w.point(1, 600));
    assert_eq!(placement(&w.graph, &s), Some(Placement::AtNode(bb)));
    let path = w.fastest(Mode::Car, &s, &t).unwrap();
    assert_eq!(path.dedges, vec![fwd(1)]);
    assert_eq!(path.time_ds, 72);
    assert_legal(&w, Mode::Car, &path);

    // 出発：辺0の始点（a）。到着：辺1の始点（= b、ノード上）。最後の有向辺は辺0の全長で、bの加算は掛からない。
    let (s, t) = (w.point(0, 0), w.point(1, 0));
    let path = w.fastest(Mode::Car, &s, &t).unwrap();
    assert_eq!(path.dedges, vec![fwd(0)]);
    assert_eq!(path.time_ds, 120);
    // 出発と到着が同じノード（辺は違う）は同じ位置：経路なし。
    assert!(
        w.fastest(Mode::Car, &w.point(0, 1000), &w.point(1, 0))
            .is_none()
    );
}

#[test]
fn unreachable_targets_return_none() {
    // 2つの連結成分。
    let mut b = GraphBuilder::new();
    let lat = 356_000_000;
    let lon = 1_397_000_000;
    let n: Vec<NodeId> = (0..4)
        .map(|i| b.add_node(Coord::new(lat, lon + 11_000 * i)))
        .collect();
    b.add_road(n[0], n[1]);
    b.add_road(n[2], n[3]);
    let w = World::new(&b);
    for mode in Mode::ALL {
        assert!(
            w.fastest(mode, &w.point(0, 500), &w.point(1, 500))
                .is_none(),
            "{mode:?}"
        );
    }

    // 一方通行の行き止まり：出られない。
    let mut b = GraphBuilder::new();
    let n: Vec<NodeId> = (0..3)
        .map(|i| b.add_node(Coord::new(lat, lon + 11_000 * i)))
        .collect();
    b.add_edge(EdgeSpec::new(n[0], n[1]).length_dm(1000)); // 戻れる
    b.add_edge(EdgeSpec::new(n[1], n[2]).length_dm(1000).oneway()); // 入ったら戻れない
    let w = World::new(&b);
    let (inside, outside) = (w.point(1, 500), w.point(0, 500));
    assert!(w.fastest(Mode::Car, &outside, &inside).is_some());
    assert!(w.fastest(Mode::Car, &inside, &outside).is_none());
    assert!(w.fastest(Mode::Walk, &inside, &outside).is_some());

    // 通過できないノードの先には行けない（端点としては使える）。
    let mut b = GraphBuilder::new();
    let n: Vec<NodeId> = (0..3)
        .map(|i| b.add_node(Coord::new(lat, lon + 11_000 * i)))
        .collect();
    b.add_edge(EdgeSpec::new(n[0], n[1]).length_dm(1000));
    b.add_edge(EdgeSpec::new(n[1], n[2]).length_dm(1000));
    b.add_node_flags(n[1], node_flags::block_bit(Mode::Car));
    let w = World::new(&b);
    assert!(
        w.fastest(Mode::Car, &w.point(0, 500), &w.point(1, 500))
            .is_none()
    );
    assert!(
        w.fastest(Mode::Bike, &w.point(0, 500), &w.point(1, 500))
            .is_some()
    );
    // 到着がそのノード自体なら行ける。
    let at_block = w.point(0, 1000);
    assert!(w.fastest(Mode::Car, &w.point(0, 500), &at_block).is_some());
    // 出発がそのノード自体なら出られる。
    assert!(w.fastest(Mode::Car, &at_block, &w.point(1, 500)).is_some());
}

#[test]
fn a_signalized_grid_needs_the_slack_to_grow() {
    // すべての交差点に信号（car 30秒）：辺の時間だけの下界より、本当の最短時間がずっと大きい。
    // 初期のmでは足りず、2倍を繰り返して見つかる。
    let (mut b, shape) = grid(8, 8, 100.0);
    b.set_time_model(penalized_model());
    for r in 0..8 {
        for c in 0..8 {
            b.add_node_flags(shape.node(r, c), node_flags::SIGNAL);
        }
    }
    let w = World::new(&b);
    let (s, t) = (
        w.point(0, 500),
        w.point(b.dedge(shape.node(7, 6), shape.node(7, 7)).edge(), 500),
    );
    let mode = Mode::Car;
    let path = w.fastest(mode, &s, &t).unwrap();
    assert_legal(&w, mode, &path);
    assert_eq!(Some(path.time_ds), reference_time(&w, mode, &s, &t));
    // 辺の時間だけの下界（約13本分の辺）に対し、信号を10か所以上通るので、時間は2倍近い。
    let edge_only: u64 = path
        .dedges
        .iter()
        .map(|&d| u64::from(edge_time_ds(&w.graph, d, mode).unwrap()))
        .sum();
    assert!(
        path.time_ds > edge_only + 3000,
        "{} {}",
        path.time_ds,
        edge_only
    );
}

// ---------------------------------------------------------------------------------------------
// 参照実装
// ---------------------------------------------------------------------------------------------

fn relax(
    dist: &mut HashMap<(u32, u32), u64>,
    heap: &mut BinaryHeap<Reverse<(u64, u32, u32)>>,
    d: DirEdge,
    a: u32,
    cost: u64,
) {
    let slot = dist.entry((d.0, a)).or_insert(u64::MAX);
    if cost < *slot {
        *slot = cost;
        heap.push(Reverse((cost, d.0, a)));
    }
}

fn update(best: &mut Option<u64>, cost: u64) {
    *best = Some(best.map_or(cost, |b| b.min(cost)));
}

/// 参照実装：全体グラフで、状態「直前の有向辺＋照合器の状態」の素朴なDijkstra（枝刈り・下界なし）。
/// 出発・到着の吸着点の扱いは、形式書8章・11章から自力で組む（`search::endpoints` を使わない）。
fn reference_time(w: &World, mode: Mode, start: &SnapPoint, end: &SnapPoint) -> Option<u64> {
    let g = &w.graph;
    let automaton = w.automaton(mode);
    let at_node = |p: &SnapPoint| {
        let r = g.edge(p.edge);
        if p.offset_dm == 0 {
            Some(r.tail)
        } else if p.offset_dm == r.length_dm {
            Some(r.head)
        } else {
            None
        }
    };
    let (start_node, end_node) = (at_node(start), at_node(end));
    if (start.edge == end.edge && start.offset_dm == end.offset_dm)
        || (start_node.is_some() && start_node == end_node)
    {
        return None;
    }
    let mut best: Option<u64> = None;
    let mut dist: HashMap<(u32, u32), u64> = HashMap::new();
    let mut heap = BinaryHeap::new();
    let a0 = automaton.start();

    // 最初の有向辺。
    match start_node {
        None => {
            let len = g.edge(start.edge).length_dm;
            for (rev, travelled) in [(false, len - start.offset_dm), (true, start.offset_dm)] {
                let d = DirEdge::new(start.edge, rev);
                if let (Some(cost), Some(a1)) = (
                    partial_edge_time_ds(g, d, mode, travelled),
                    automaton.step(a0, d),
                ) {
                    relax(&mut dist, &mut heap, d, a1, u64::from(cost));
                }
            }
        }
        Some(n) => {
            for &d in g.out_dedges(n) {
                if let (Some(cost), Some(a1)) = (edge_time_ds(g, d, mode), automaton.step(a0, d)) {
                    relax(&mut dist, &mut heap, d, a1, u64::from(cost));
                }
            }
        }
    }
    // 最後の有向辺（到着が辺の途中のとき）。
    let last: Vec<(DirEdge, u32)> = if end_node.is_none() {
        let len = g.edge(end.edge).length_dm;
        vec![
            (DirEdge::new(end.edge, false), end.offset_dm),
            (DirEdge::new(end.edge, true), len - end.offset_dm),
        ]
    } else {
        vec![]
    };
    // 同じ辺の途中から途中へ、1本で行く。
    if start_node.is_none() && end_node.is_none() && start.edge == end.edge {
        let (d, travelled) = if end.offset_dm > start.offset_dm {
            (
                DirEdge::new(start.edge, false),
                end.offset_dm - start.offset_dm,
            )
        } else {
            (
                DirEdge::new(start.edge, true),
                start.offset_dm - end.offset_dm,
            )
        };
        if let Some(cost) = partial_edge_time_ds(g, d, mode, travelled) {
            update(&mut best, u64::from(cost));
        }
    }
    // 出発がノード上で、そこから最後の有向辺に入る。
    if let Some(n) = start_node {
        for &(d, travelled) in &last {
            if g.dedge_from(d) == n
                && automaton.step(a0, d).is_some()
                && let Some(cost) = partial_edge_time_ds(g, d, mode, travelled)
            {
                update(&mut best, u64::from(cost));
            }
        }
    }

    while let Some(Reverse((cost, d, a))) = heap.pop() {
        if dist[&(d, a)] != cost {
            continue;
        }
        let d = DirEdge(d);
        let v = g.dedge_to(d);
        if end_node == Some(v) {
            update(&mut best, cost);
        }
        if g.node_blocked(v, mode) {
            continue;
        }
        let penalty = u64::from(node_penalty_ds(g, v, mode));
        for &(l, travelled) in &last {
            if g.dedge_from(l) == v
                && automaton.step(a, l).is_some()
                && let Some(t) = partial_edge_time_ds(g, l, mode, travelled)
            {
                let turn = u64::from(turn_cost_ds(g, d, l, mode));
                update(&mut best, cost + penalty + turn + u64::from(t));
            }
        }
        for &o in g.out_dedges(v) {
            let (Some(t), Some(a2)) = (edge_time_ds(g, o, mode), automaton.step(a, o)) else {
                continue;
            };
            let turn = u64::from(turn_cost_ds(g, d, o, mode));
            relax(
                &mut dist,
                &mut heap,
                o,
                a2,
                cost + penalty + turn + u64::from(t),
            );
        }
    }
    best
}

/// 小さなグラフ向けの全探索：長さ `max_len` 以下の有向辺の並びを、時間が `cap` 以下になり得る範囲で試し、
/// `check_path` が違反0件、`path_time_ds` が `Some` のもののうち最短の時間を求める。
/// 有向辺の並びの評価に `check_path` と `path_time_ds` だけを使うので、探索（照合器・部分有向辺）とは独立。
struct Brute<'a> {
    w: &'a World,
    mode: Mode,
    start: &'a SnapPoint,
    end: &'a SnapPoint,
    max_len: usize,
    cap: u64,
    best: Option<u64>,
}

impl Brute<'_> {
    fn run(&mut self) {
        let g = &self.w.graph;
        let start_node = match placement(g, self.start).unwrap() {
            Placement::AtNode(n) => Some(n),
            Placement::Inside => None,
        };
        for raw in 0..2 * g.edge_count() as u32 {
            let d = DirEdge(raw);
            let ok_start = match start_node {
                Some(n) => g.dedge_from(d) == n,
                None => d.edge() == self.start.edge,
            };
            if ok_start && edge_time_ds(g, d, self.mode).is_some() {
                self.dfs(&mut vec![d], 0);
            }
        }
    }

    /// `seq` を完成した経路として評価し、さらに延ばす。`lower` は `seq` の最後の有向辺を除く時間の合計
    /// （最後の有向辺は到着で短くなるので除く）。後ろのコストは0以上なので、これが `cap` を超えたら延ばさない。
    fn dfs(&mut self, seq: &mut Vec<DirEdge>, lower: u64) {
        let (w, mode) = (self.w, self.mode);
        let g = &w.graph;
        if let Some(t) = path_time_ds(g, mode, self.start, self.end, seq)
            && check_path(g, &w.restrictions, mode, seq).is_empty()
        {
            update(&mut self.best, t);
        }
        if seq.len() == self.max_len || lower > self.cap {
            return;
        }
        let last = *seq.last().unwrap();
        // 最後の有向辺が先頭なら出発の部分、そうでなければ全長（到着の部分は延ばした後に評価する）。
        let last_time = if seq.len() == 1 && placement(g, self.start) == Some(Placement::Inside) {
            let len = g.edge(last.edge()).length_dm;
            let travelled = if last.is_reverse() {
                self.start.offset_dm
            } else {
                len - self.start.offset_dm
            };
            partial_edge_time_ds(g, last, mode, travelled)
        } else {
            edge_time_ds(g, last, mode)
        };
        let Some(last_time) = last_time else { return };
        for &o in g.out_dedges(g.dedge_to(last)) {
            if edge_time_ds(g, o, mode).is_none() {
                continue;
            }
            seq.push(o);
            if check_path(g, &w.restrictions, mode, seq).is_empty() {
                self.dfs(seq, lower + u64::from(last_time));
            }
            seq.pop();
        }
    }
}

/// `cap` デシ秒以下の合法な経路のうち最短の時間。なければ `None`。
fn brute_force(
    w: &World,
    mode: Mode,
    start: &SnapPoint,
    end: &SnapPoint,
    max_len: usize,
    cap: u64,
) -> Option<u64> {
    let mut brute = Brute {
        w,
        mode,
        start,
        end,
        max_len,
        cap,
        best: None,
    };
    brute.run();
    brute.best
}

// ---------------------------------------------------------------------------------------------
// 最短時間経路：ランダムなグラフ
// ---------------------------------------------------------------------------------------------

fn random_snap(rng: &mut Rng, graph: &Graph) -> SnapPoint {
    let e = rng.below(graph.edge_count()) as u32;
    let len = graph.edge(e).length_dm;
    let offset = match rng.below(10) {
        0 => 0,
        1 => len,
        _ => 1 + rng.below((len - 1) as usize) as u32,
    };
    SnapPoint::on_edge(graph, e, offset)
}

fn rich_world(seed: u64, nodes: usize, penalties: bool) -> World {
    let mut b = random_graph(seed, &RandomGraphConfig::rich(nodes));
    if penalties {
        b.set_time_model(penalized_model());
    }
    World::new(&b)
}

/// 返した経路の3つの検査と、参照実装との一致。見つかったかどうかを返す。
fn check_against_reference(
    w: &World,
    mode: Mode,
    params: SearchParams,
    s: &SnapPoint,
    t: &SnapPoint,
) -> bool {
    let ctx = w.ctx_with(mode, params);
    let found = fastest_path(&ctx, s, t, &CancelToken::unbounded()).unwrap();
    let expected = reference_time(w, mode, s, t);
    match (&found, expected) {
        (Some(path), Some(time)) => {
            assert_legal(w, mode, path);
            assert_eq!(
                path.time_ds, time,
                "{mode:?} {s:?} → {t:?}: {:?}",
                path.dedges
            );
            assert_eq!((path.start, path.end), (*s, *t));
            true
        }
        (None, None) => false,
        (a, b) => panic!(
            "{mode:?} {s:?} → {t:?}: 探索 {:?}、参照 {b:?}",
            a.as_ref().map(|p| p.time_ds)
        ),
    }
}

#[test]
fn random_rich_graphs_match_the_reference() {
    let mut found_per_mode = [0_usize; 3];
    let mut queries_per_mode = [0_usize; 3];
    for (i, mode) in Mode::ALL.into_iter().enumerate() {
        for seed in 0..120_u64 {
            let nodes = 8 + (seed as usize * 7) % 45;
            let w = rich_world(1000 + seed, nodes, seed % 3 != 0);
            let mut rng = Rng::new(seed * 31 + mode as u64);
            for _ in 0..3 {
                let (s, t) = (
                    random_snap(&mut rng, &w.graph),
                    random_snap(&mut rng, &w.graph),
                );
                queries_per_mode[i] += 1;
                if check_against_reference(&w, mode, SearchParams::DEFAULT, &s, &t) {
                    found_per_mode[i] += 1;
                }
            }
        }
    }
    eprintln!("見つかった数 {found_per_mode:?} / {queries_per_mode:?}（walk, bike, car の順）");
    for i in 0..3 {
        assert_eq!(queries_per_mode[i], 360);
        // 見つからないものばかり（自明な試験）になっていない。
        assert!(found_per_mode[i] >= 100, "{i}: {}", found_per_mode[i]);
    }
}

#[test]
fn larger_graphs_match_the_reference_without_exhausting_the_graph() {
    // ノード数百〜千の疎なグラフ：探索は全ノードに届く前に終わるので、部分グラフ（楕円）で切って探す。
    // `m` の初期値を変えて（1、7、既定）、部分グラフの境界をいろいろな位置で通す。結果は同じでなければならない。
    let mut found = 0;
    let mut partial = 0;
    let mut total = 0;
    for (i, mode) in Mode::ALL.into_iter().enumerate() {
        for seed in 0..14_u64 {
            let nodes = 300 + 90 * seed as usize;
            let w = rich_world(20_000 + seed * 3 + i as u64, nodes, seed % 3 != 1);
            let mut rng = Rng::new(seed * 101 + i as u64);
            let ctx = w.ctx(mode);
            let mut work = FastestWork::new();
            for q in 0..4 {
                let (s, t) = (
                    random_snap(&mut rng, &w.graph),
                    random_snap(&mut rng, &w.graph),
                );
                let expected = reference_time(&w, mode, &s, &t);
                total += 1;
                for slack in [None, Some(1), Some(7)] {
                    work.set_initial_slack_ds(slack);
                    let path =
                        fastest_path_with(&ctx, &s, &t, &CancelToken::unbounded(), &mut work)
                            .unwrap();
                    assert_eq!(
                        path.as_ref().map(|p| p.time_ds),
                        expected,
                        "{mode:?} seed {seed} q {q} slack {slack:?}"
                    );
                    if let Some(path) = path {
                        assert_legal(&w, mode, &path);
                        if slack.is_none() {
                            found += 1;
                            // 部分グラフで見つけた：前向き・後ろ向きで触れたノードの合計がグラフ全体より少ない。
                            if work.touched().0 < w.graph.node_count() {
                                partial += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    eprintln!("見つかった {found} / {total}、うち触れたノードが全ノード数未満 {partial}");
    assert!(found >= 60, "{found}");
    assert!(partial >= found / 2, "{partial} / {found}");
}

#[test]
fn full_graph_fallback_matches_the_reference() {
    // 部分のノード数の上限を小さくして、全体グラフでの探索（下界つきのA*）に切り替えさせる。
    let tiny = SearchParams {
        ellipse_node_limit: 2,
        ..SearchParams::DEFAULT
    };
    let zero = SearchParams {
        ellipse_node_limit: 0,
        ..SearchParams::DEFAULT
    };
    let mut found = 0;
    for mode in Mode::ALL {
        for seed in 0..60_u64 {
            let w = rich_world(5000 + seed, 10 + (seed as usize * 5) % 40, seed % 2 == 0);
            let mut rng = Rng::new(seed + 77);
            for params in [tiny, zero] {
                let (s, t) = (
                    random_snap(&mut rng, &w.graph),
                    random_snap(&mut rng, &w.graph),
                );
                if check_against_reference(&w, mode, params, &s, &t) {
                    found += 1;
                }
            }
        }
    }
    assert!(found >= 120, "{found}");
}

#[test]
fn small_graphs_match_the_exhaustive_search() {
    const MAX_LEN: usize = 12;
    let mut compared = 0;
    let mut not_found = 0;
    for mode in Mode::ALL {
        for seed in 0..80_u64 {
            let w = rich_world(9000 + seed, 5 + (seed as usize) % 4, seed % 2 == 1);
            let mut rng = Rng::new(seed * 13 + 5);
            for _ in 0..3 {
                let (s, t) = (
                    random_snap(&mut rng, &w.graph),
                    random_snap(&mut rng, &w.graph),
                );
                let Some(path) = w.fastest(mode, &s, &t) else {
                    // 見つからないときは、参照実装も見つけない（上のテストでも確かめている）。
                    assert_eq!(
                        reference_time(&w, mode, &s, &t),
                        None,
                        "{mode:?} seed {seed}"
                    );
                    not_found += 1;
                    continue;
                };
                assert_legal(&w, mode, &path);
                // 探索の結果の時間以下の合法な経路を全探索する。探索より速い経路があれば失敗。
                // 探索の経路自体が全探索の長さの上限に収まるなら、同じ時間の経路が見つかるはず。
                let best = brute_force(&w, mode, &s, &t, MAX_LEN, path.time_ds);
                if let Some(b) = best {
                    assert!(
                        b >= path.time_ds,
                        "{mode:?} seed {seed}: 全探索が{b}、探索が{}",
                        path.time_ds
                    );
                }
                if path.dedges.len() <= MAX_LEN {
                    assert_eq!(best, Some(path.time_ds), "{mode:?} seed {seed}");
                    compared += 1;
                }
            }
        }
    }
    assert!(
        compared >= 300,
        "{compared}（見つからなかった {not_found}）"
    );
}

#[test]
fn restrictions_never_appear_in_returned_paths() {
    // 右左折禁止を多く付けた密なグラフ：返す経路に禁止の列が含まれない。禁止のないグラフより速くならない。
    let mut checked = 0;
    let mut slower = 0;
    for mode in Mode::ALL {
        for seed in 0..40_u64 {
            let config = RandomGraphConfig {
                via_node_restrictions: 40,
                via_way_restrictions: 40,
                restriction_modes: Some(ALL_MODES_MASK),
                ..RandomGraphConfig::new(30)
            };
            let restricted = World::new(&random_graph(seed + 300, &config));
            let free = World::new(&random_graph(
                seed + 300,
                &RandomGraphConfig {
                    via_node_restrictions: 0,
                    via_way_restrictions: 0,
                    ..config.clone()
                },
            ));
            assert_eq!(restricted.graph.edge_count(), free.graph.edge_count());
            let mut rng = Rng::new(seed);
            for _ in 0..4 {
                let (s, t) = (
                    random_snap(&mut rng, &restricted.graph),
                    random_snap(&mut rng, &restricted.graph),
                );
                let a = restricted.fastest(mode, &s, &t);
                let b = free.fastest(mode, &s, &t);
                if let Some(a) = &a {
                    assert_legal(&restricted, mode, a);
                    checked += 1;
                    let b = b.expect("禁止を外せば同じ経路がある");
                    assert!(a.time_ds >= b.time_ds);
                    if a.time_ds > b.time_ds {
                        slower += 1;
                    }
                }
                assert_eq!(
                    a.as_ref().map(|p| p.time_ds),
                    reference_time(&restricted, mode, &s, &t)
                );
            }
        }
    }
    assert!(checked > 100 && slower > 10, "{checked} {slower}");
}

#[test]
fn the_work_area_can_be_reused() {
    let mut work = FastestWork::new();
    let cancel = CancelToken::unbounded();
    for seed in 0..20_u64 {
        let w = rich_world(7000 + seed, 30, true);
        let mut rng = Rng::new(seed);
        for mode in Mode::ALL {
            let (s, t) = (
                random_snap(&mut rng, &w.graph),
                random_snap(&mut rng, &w.graph),
            );
            let ctx = w.ctx(mode);
            let fresh = fastest_path(&ctx, &s, &t, &cancel).unwrap();
            let reused = fastest_path_with(&ctx, &s, &t, &cancel, &mut work).unwrap();
            assert_eq!(fresh, reused, "seed {seed} {mode:?}");
        }
    }
    work.release();
    let (b, shape) = grid(4, 4, 100.0);
    let w = World::new(&b);
    let e = b.dedge(shape.node(3, 2), shape.node(3, 3)).edge();
    let after = fastest_path_with(
        &w.ctx(Mode::Car),
        &w.point(0, 500),
        &w.point(e, 500),
        &cancel,
        &mut work,
    )
    .unwrap();
    assert!(after.is_some());
}

#[test]
fn the_search_touches_only_a_part_of_a_large_graph() {
    // 200×200の格子（40,000ノード）で、同じ行の30辺ぶんの経路。作業領域はグラフ全体の大きさにならない。
    let (b, shape) = grid(200, 200, 100.0);
    let w = World::new(&b);
    let first = b.dedge(shape.node(100, 80), shape.node(100, 81)).edge();
    let last = b.dedge(shape.node(100, 109), shape.node(100, 110)).edge();
    let (s, t) = (w.point(first, 500), w.point(last, 500));
    let mut work = FastestWork::new();
    let path = fastest_path_with(
        &w.ctx(Mode::Car),
        &s,
        &t,
        &CancelToken::unbounded(),
        &mut work,
    )
    .unwrap()
    .unwrap();
    assert_legal(&w, Mode::Car, &path);
    assert_eq!(path.dedges.len(), 30);
    let (nodes, states) = work.touched();
    eprintln!(
        "触れたノード {nodes}、状態 {states}（全体 {}）",
        w.graph.node_count()
    );
    assert!(nodes < w.graph.node_count() / 10, "{nodes}");
    assert!(states < w.graph.node_count() / 10, "{states}");
}

// ---------------------------------------------------------------------------------------------
// 状態つき探索の部品（turn_dijkstra）
// ---------------------------------------------------------------------------------------------

/// 時間を2倍にし、有向辺 `penalized` の遷移に大きな加算をする、差し替え用のコスト。
struct Weighted {
    penalized: DirEdge,
    seen: Vec<Leg>,
}

impl Pricing for Weighted {
    type Cost = u64;
    const ZERO: u64 = 0;

    fn price(&mut self, t: &Transition) -> u64 {
        self.seen.push(t.leg);
        2 * t.time_ds
            + if t.out == self.penalized {
                1_000_000
            } else {
                0
            }
    }
}

#[test]
fn the_cost_type_of_the_state_search_can_be_replaced() {
    let (b, [_, _, _, _, _, _]) = ladder();
    let w = World::new(&b);
    let g = &w.graph;
    let (s, t) = (w.point(0, 500), w.point(1, 500));
    let ep = Endpoints::new(g, Mode::Car, &s, &t).unwrap();
    let cancel = CancelToken::unbounded();
    let mut work = TurnWork::new();

    // 時間だけ：fwd(0), fwd(1)。
    let plain = turn_dijkstra(
        &mut TurnSearch {
            graph: g,
            automaton: w.automaton(Mode::Car),
            mode: Mode::Car,
            endpoints: &ep,
            pricing: TimeOnly,
            filter: None,
            potential: None,
            bound: None,
        },
        &mut work,
        &cancel,
    )
    .unwrap()
    .unwrap();
    assert_eq!(plain.dedges, vec![fwd(0), fwd(1)]);
    assert_eq!(
        plain.cost,
        path_time_ds(g, Mode::Car, &s, &t, &plain.dedges).unwrap()
    );

    // 辺1の有向辺を通る遷移に大きな加算：遠回りの別の経路（Cから戻って到着）を選ぶ。
    let mut search = TurnSearch {
        graph: g,
        automaton: w.automaton(Mode::Car),
        mode: Mode::Car,
        endpoints: &ep,
        pricing: Weighted {
            penalized: fwd(1),
            seen: vec![],
        },
        filter: None,
        potential: None,
        bound: None,
    };
    let other = turn_dijkstra(&mut search, &mut work, &cancel)
        .unwrap()
        .unwrap();
    assert_eq!(other.dedges.first(), Some(&fwd(0)));
    assert_eq!(other.dedges.last(), Some(&bwd(1)));
    let seen = &search.pricing.seen;
    assert!(seen.contains(&Leg::Departure) && seen.contains(&Leg::Arrival));
    assert!(seen.contains(&Leg::Interior));

    // 上限：時間を超える経路は見つけない。
    let bounded = turn_dijkstra(
        &mut TurnSearch {
            graph: g,
            automaton: w.automaton(Mode::Car),
            mode: Mode::Car,
            endpoints: &ep,
            pricing: TimeOnly,
            filter: None,
            potential: None,
            bound: Some(plain.cost - 1),
        },
        &mut work,
        &cancel,
    )
    .unwrap();
    assert!(bounded.is_none());
    assert!(work.touched_states() > 0);
}

// ---------------------------------------------------------------------------------------------
// 中断
// ---------------------------------------------------------------------------------------------

#[test]
fn a_cancelled_token_interrupts_the_search() {
    let (b, shape) = grid(6, 6, 100.0);
    let w = World::new(&b);
    let e = b.dedge(shape.node(5, 4), shape.node(5, 5)).edge();
    let (s, t) = (w.point(0, 500), w.point(e, 500));
    let ctx = w.ctx(Mode::Car);

    let token = CancelToken::unbounded();
    token.cancel();
    assert_eq!(
        fastest_path(&ctx, &s, &t, &token),
        Err(Interrupt::Cancelled)
    );
    let expired = CancelToken::with_deadline(Instant::now() - Duration::from_millis(1));
    assert_eq!(
        fastest_path(&ctx, &s, &t, &expired),
        Err(Interrupt::DeadlineExceeded)
    );
    // 取り消されていなければ通る。
    assert!(
        fastest_path(&ctx, &s, &t, &CancelToken::unbounded())
            .unwrap()
            .is_some()
    );
    // 同じ位置・経路なしでも、取り消しが先に判定される。
    assert_eq!(
        fastest_path(&ctx, &s, &s, &token),
        Err(Interrupt::Cancelled)
    );
}

#[test]
fn the_search_stops_soon_after_the_token_is_cancelled() {
    // 大きな格子（160,000ノード）の端から端まで。探索の途中で取り消す。
    let (mut b, shape) = grid(400, 400, 100.0);
    b.set_time_model(penalized_model());
    // 経路の途中の1か所を通れなくして、探索が長引くようにする（下界との差が大きい）。
    for r in 0..400 {
        b.add_node_flags(shape.node(r, 200), node_flags::SIGNAL);
    }
    let w = World::new(&b);
    let last = b.dedge(shape.node(399, 398), shape.node(399, 399)).edge();
    let (s, t) = (w.point(0, 500), w.point(last, 500));
    let token = CancelToken::unbounded();
    let canceller = {
        let token = token.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            token.cancel();
        })
    };
    let begin = Instant::now();
    let result = fastest_path(&w.ctx(Mode::Car), &s, &t, &token);
    let elapsed = begin.elapsed();
    canceller.join().unwrap();
    assert_eq!(result, Err(Interrupt::Cancelled), "{elapsed:?}");
    // 取り消し（30ms）から遅れすぎない。テストの実行環境の負荷を見込んだ余裕を取る。
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
}
