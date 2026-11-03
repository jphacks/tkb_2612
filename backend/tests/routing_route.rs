//! 探索の結合（M3b.4）の試験：behavior.md 5章 手順2〜8。合成グラフと、辺ごとに決めたリスクを返す
//! 簡単な `ArcCost` を使う。乱数は seed を固定する。

use accident_route_backend::contract::{CancelToken, Mode};
use accident_route_backend::domain::routing::NONE;
use accident_route_backend::domain::routing::graph::restrictions::{Restrictions, TurnAutomaton};
use accident_route_backend::domain::routing::graph::synthetic::{
    EdgeSpec, GraphBuilder, Rng, grid,
};
use accident_route_backend::domain::routing::graph::{DirEdge, EdgeId, Graph, HighwayClass};
use accident_route_backend::domain::routing::legality::check_path;
use accident_route_backend::domain::routing::search::alt::Landmarks;
use accident_route_backend::domain::routing::search::candidates::MAX_ROUTE_DISTANCE_DM;
use accident_route_backend::domain::routing::search::ellipse::ellipse_subgraph;
use accident_route_backend::domain::routing::search::endpoints::is_same_position;
use accident_route_backend::domain::routing::search::fastest::fastest_path;
use accident_route_backend::domain::routing::search::reduced::reduced_ellipse;
use accident_route_backend::domain::routing::search::route::{
    RouteError, RouteSearch, find_routes, path_risk_fx,
};
use accident_route_backend::domain::routing::search::{
    ArcCost, SearchContext, SearchParams, SearchStatus,
};
use accident_route_backend::domain::routing::snap::SnapPoint;
use accident_route_backend::domain::routing::topology::Topology;

// ---------------------------------------------------------------------------------------------
// 共通の部品（`routing_search.rs` と同じ作り）
// ---------------------------------------------------------------------------------------------

struct World {
    graph: Graph,
    restrictions: Restrictions,
    topology: Topology,
    automaton: TurnAutomaton,
    landmarks: Landmarks,
    mode: Mode,
}

/// 辺 `edges` 本ぶんのビットがすべて立ったビット列。
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

/// クラスタ・双子・歩道のない `Topology`（すべての辺が吸着先になれる）。
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

impl World {
    fn new(builder: &GraphBuilder, mode: Mode) -> World {
        let (graph, restrictions) = builder.build_all();
        let topology = empty_topology(&graph);
        let automaton = TurnAutomaton::build(&restrictions, mode);
        World {
            graph,
            restrictions,
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

/// 時間予算なし（ラベル数の上限もなし）の探索の設定。
fn unlimited_params() -> SearchParams {
    SearchParams {
        rcsp_budget_ms: u64::MAX / 4,
        larac_iterations: 10,
        ellipse_node_limit: u32::MAX,
        rcsp_label_limit: u32::MAX,
    }
}

/// 辺ごとの任意のリスク。出発・到着の辺の部分は長さに比例させる。
struct TestCost {
    risk: Vec<u64>,
    entry: Vec<u64>,
    length: Vec<u32>,
}

impl TestCost {
    fn zero(graph: &Graph) -> TestCost {
        TestCost {
            risk: vec![0; 2 * graph.edge_count()],
            entry: vec![0; 2 * graph.edge_count()],
            length: (0..graph.edge_count() as u32)
                .map(|e| graph.edge(e).length_dm)
                .collect(),
        }
    }

    /// 有向辺のリスクは `長さ(dm) × 0〜40`、20%の有向辺はクラスタに入る加算（0〜50,000）を持つ。
    fn random(rng: &mut Rng, graph: &Graph) -> TestCost {
        let mut cost = TestCost::zero(graph);
        for d in 0..(2 * graph.edge_count()) as u32 {
            let len = u64::from(cost.length[(d / 2) as usize]);
            cost.risk[d as usize] = len * rng.below(41) as u64;
            cost.entry[d as usize] = if rng.chance(0.2) {
                rng.below(50_001) as u64
            } else {
                0
            };
        }
        cost
    }

    fn set_risk(&mut self, d: DirEdge, risk: u64) {
        self.risk[d.0 as usize] = risk;
    }

    fn set_entry(&mut self, d: DirEdge, entry: u64) {
        self.entry[d.0 as usize] = entry;
    }

    /// 辺 `d` を `travelled_dm` だけ通るときの、長さに比例したリスク。
    fn partial(&self, d: DirEdge, travelled_dm: u32) -> u64 {
        let len = u128::from(self.length[d.edge() as usize]);
        if len == 0 {
            return 0;
        }
        ((u128::from(self.risk[d.0 as usize]) * u128::from(travelled_dm) + len / 2) / len) as u64
    }
}

impl ArcCost for TestCost {
    fn risk_fx(&mut self, d: DirEdge) -> u64 {
        self.risk[d.0 as usize]
    }

    fn partial_risk_fx(&mut self, d: DirEdge, from_dm: u32, to_dm: u32) -> u64 {
        let len = u128::from(self.length[d.edge() as usize]);
        if len == 0 {
            return 0;
        }
        let part = u128::from(to_dm - from_dm);
        ((u128::from(self.risk[d.0 as usize]) * part + len / 2) / len) as u64
    }

    fn cluster_entry_risk_fx(&mut self, d: DirEdge) -> u64 {
        self.entry[d.0 as usize]
    }
}

/// ランダムな吸着点。7割は辺の途中、残りは辺の端のノード。
fn random_snap(rng: &mut Rng, graph: &Graph) -> SnapPoint {
    let edge = rng.below(graph.edge_count()) as u32;
    let length = graph.edge(edge).length_dm;
    let offset = if length >= 2 && rng.chance(0.7) {
        1 + rng.below((length - 1) as usize) as u32
    } else if rng.chance(0.5) {
        0
    } else {
        length
    };
    SnapPoint::on_edge(graph, edge, offset)
}

/// 候補の条件（1.）と、`path_risk_fx` の一致（3.）を確かめる。
fn assert_candidates_and_risk(world: &World, cost: &mut TestCost, result: &RouteSearch) {
    let g = &world.graph;
    assert!(!result.candidates.is_empty());
    assert!(
        result.candidates.len() <= 6,
        "候補が6本を超えた: {}",
        result.candidates.len()
    );
    assert!(
        result.candidates.iter().any(|p| p == &result.p0),
        "p0 が候補にない"
    );
    for pair in result.candidates.windows(2) {
        assert!(
            pair[0].time_ds <= pair[1].time_ds,
            "候補が所要時間の昇順でない: {} > {}",
            pair[0].time_ds,
            pair[1].time_ds
        );
    }
    for candidate in &result.candidates {
        assert!(
            candidate.time_ds <= result.budget_ds,
            "候補の時間が上限を超えた: {} > {}",
            candidate.time_ds,
            result.budget_ds
        );
        assert!(
            check_path(g, &world.restrictions, world.mode, &candidate.dedges).is_empty(),
            "候補が合法でない: {:?}",
            check_path(g, &world.restrictions, world.mode, &candidate.dedges)
        );
        assert_eq!(
            path_risk_fx(g, cost, candidate),
            candidate.risk_fx,
            "候補のリスクが一致しない: {:?}",
            candidate.dedges
        );
    }
    assert_eq!(
        path_risk_fx(g, cost, &result.best),
        result.best.risk_fx,
        "best のリスクが一致しない: {:?}",
        result.best.dedges
    );
    assert_eq!(path_risk_fx(g, cost, &result.p0), result.p0.risk_fx);
    assert_eq!(
        result.stats.rcsp_completed,
        result.stats.status == SearchStatus::Optimal
    );
}

// ---------------------------------------------------------------------------------------------
// 1. 候補の条件
// ---------------------------------------------------------------------------------------------

#[test]
fn candidates_are_legal_within_budget_sorted_and_bounded() {
    let (builder, shape) = grid(8, 8, 120.0);
    let world = World::new(&builder, Mode::Car);
    let g = &world.graph;
    let mut rng = Rng::new(11);
    let mut cost = TestCost::random(&mut rng, g);
    let first = builder.dedge(shape.node(0, 0), shape.node(0, 1));
    let last = builder.dedge(shape.node(7, 6), shape.node(7, 7));
    let start = SnapPoint::on_edge(g, first.edge(), 0);
    let end = SnapPoint::on_edge(g, last.edge(), g.edge(last.edge()).length_dm);
    let cancel = CancelToken::unbounded();
    let ctx = world.ctx(unlimited_params());
    let result = find_routes(&ctx, &mut cost, &start, &end, 5, &cancel).unwrap();
    assert_eq!(result.budget_ds, result.p0.time_ds + 600 * 5);
    assert_candidates_and_risk(&world, &mut cost, &result);
    assert!(result.stats.ellipse_nodes.is_some());
    assert!(result.stats.ellipse_arcs.is_some());
    assert!(result.stats.fastest_ms.is_finite());
}

// ---------------------------------------------------------------------------------------------
// 2. リスクの高い近道と、リスクの低い回り道
// ---------------------------------------------------------------------------------------------

/// 近道（0→1、100 m・リスク高）と回り道（0→2→3→1、各60 m・リスク0）を持つグラフ。
/// 30 km/h なら近道は120デシ秒、回り道は216デシ秒。
struct DetourWorld {
    builder: GraphBuilder,
    direct: EdgeId,
    detour: [DirEdge; 3],
}

fn detour_world() -> DetourWorld {
    let mut b = GraphBuilder::new();
    let n0 = b.add_node_deg(35.000, 139.000);
    let n1 = b.add_node_deg(35.000, 139.001);
    let n2 = b.add_node_deg(35.001, 139.000);
    let n3 = b.add_node_deg(35.001, 139.001);
    let direct = b.add_edge(EdgeSpec::new(n0, n1).length_dm(1000));
    let d0 = b.add_edge(EdgeSpec::new(n0, n2).length_dm(600));
    let d1 = b.add_edge(EdgeSpec::new(n2, n3).length_dm(600));
    let d2 = b.add_edge(EdgeSpec::new(n3, n1).length_dm(600));
    DetourWorld {
        builder: b,
        direct,
        detour: [
            DirEdge::new(d0, false),
            DirEdge::new(d1, false),
            DirEdge::new(d2, false),
        ],
    }
}

#[test]
fn best_uses_the_low_risk_detour_when_the_budget_allows() {
    let DetourWorld {
        builder,
        direct,
        detour,
    } = detour_world();
    let world = World::new(&builder, Mode::Car);
    let g = &world.graph;
    let mut cost = TestCost::zero(g);
    let risky = DirEdge::new(direct, false);
    cost.set_risk(risky, 1_000_000);
    let start = SnapPoint::on_edge(g, direct, 0);
    let end = SnapPoint::on_edge(g, direct, g.edge(direct).length_dm);
    let cancel = CancelToken::unbounded();
    let ctx = world.ctx(unlimited_params());

    // 上限0分：回り道は上限の外なので、best は最短時間経路（近道）。
    let tight = find_routes(&ctx, &mut cost, &start, &end, 0, &cancel).unwrap();
    assert_eq!(tight.best.dedges, tight.p0.dedges);
    assert_eq!(tight.p0.risk_fx, 1_000_000);
    assert_candidates_and_risk(&world, &mut cost, &tight);

    // 上限1分（600デシ秒）：回り道（216デシ秒）が入り、リスク0なので best になる。
    let loose = find_routes(&ctx, &mut cost, &start, &end, 1, &cancel).unwrap();
    assert_eq!(loose.p0.dedges, vec![risky]);
    assert_eq!(loose.best.dedges, detour);
    assert_eq!(loose.best.risk_fx, 0);
    assert_eq!(loose.best.time_ds, 216);
    assert_candidates_and_risk(&world, &mut cost, &loose);
}

#[test]
fn path_risk_handles_inside_endpoints_on_the_direct_edge() {
    let DetourWorld {
        builder, direct, ..
    } = detour_world();
    let world = World::new(&builder, Mode::Car);
    let g = &world.graph;
    let mut cost = TestCost::zero(g);
    // 近道の途中どうし（200 dm〜700 dm）。近道の部分はリスクが高い。
    let risky = DirEdge::new(direct, false);
    cost.set_risk(risky, 1_000_000);
    let start = SnapPoint::on_edge(g, direct, 200);
    let end = SnapPoint::on_edge(g, direct, 700);
    let cancel = CancelToken::unbounded();
    let ctx = world.ctx(unlimited_params());

    // 上限0分：近道の途中だけを使う p0 が best になる。
    let tight = find_routes(&ctx, &mut cost, &start, &end, 0, &cancel).unwrap();
    assert_eq!(tight.p0.dedges, vec![risky]);
    assert_eq!(tight.best.dedges, tight.p0.dedges);
    assert_eq!(
        tight.p0.risk_fx,
        cost.partial(risky, 500),
        "近道の途中だけのリスク"
    );
    assert_candidates_and_risk(&world, &mut cost, &tight);

    // 上限1分：回り道（近道の外側の部分を通る）も候補になる。p0 と同じリスクでもよい。
    let loose = find_routes(&ctx, &mut cost, &start, &end, 1, &cancel).unwrap();
    assert!(loose.best.time_ds <= loose.budget_ds);
    assert_candidates_and_risk(&world, &mut cost, &loose);
}

// ---------------------------------------------------------------------------------------------
// 3. path_risk_fx と探索が返した risk_fx の一致
// ---------------------------------------------------------------------------------------------

#[test]
fn path_risk_matches_search_for_inside_and_node_endpoints() {
    let (builder, shape) = grid(6, 6, 150.0);
    let world = World::new(&builder, Mode::Car);
    let g = &world.graph;
    let mut rng = Rng::new(21);
    let mut cost = TestCost::random(&mut rng, g);
    let cancel = CancelToken::unbounded();
    let ctx = world.ctx(unlimited_params());
    let first = builder.dedge(shape.node(0, 0), shape.node(0, 1));
    let last = builder.dedge(shape.node(5, 4), shape.node(5, 5));

    // 辺の途中どうし。
    let start = SnapPoint::on_edge(g, first.edge(), 250);
    let end = SnapPoint::on_edge(g, last.edge(), 500);
    let inside = find_routes(&ctx, &mut cost, &start, &end, 3, &cancel).unwrap();
    assert_candidates_and_risk(&world, &mut cost, &inside);

    // ノード上どうし（吸着点が辺の端）。
    let start = SnapPoint::on_edge(g, first.edge(), 0);
    let end = SnapPoint::on_edge(g, last.edge(), g.edge(last.edge()).length_dm);
    let nodes = find_routes(&ctx, &mut cost, &start, &end, 3, &cancel).unwrap();
    assert_candidates_and_risk(&world, &mut cost, &nodes);
}

#[test]
fn path_risk_of_a_single_direct_edge_has_no_cluster_entry() {
    let mut b = GraphBuilder::new();
    let a = b.add_node_deg(35.0, 139.0);
    let c = b.add_node_deg(35.0, 139.001);
    let e = b.add_edge(EdgeSpec::new(a, c).length_dm(1000));
    let world = World::new(&b, Mode::Car);
    let g = &world.graph;
    let mut cost = TestCost::zero(g);
    let d = DirEdge::new(e, false);
    cost.set_risk(d, 500_000);
    // 到着の辺（1本だけ）ではクラスタに入らないので、この加算は足さない。
    cost.set_entry(d, 12_345);
    let start = SnapPoint::on_edge(g, e, 200);
    let end = SnapPoint::on_edge(g, e, 700);
    let cancel = CancelToken::unbounded();
    let ctx = world.ctx(unlimited_params());
    let result = find_routes(&ctx, &mut cost, &start, &end, 0, &cancel).unwrap();
    assert_eq!(result.p0.dedges, vec![d]);
    let expected = cost.partial(d, 500);
    assert_eq!(result.p0.risk_fx, expected);
    assert_eq!(path_risk_fx(g, &mut cost, &result.p0), expected);
}

// ---------------------------------------------------------------------------------------------
// 4. 縮小と、最短時間経路だけにする分岐
// ---------------------------------------------------------------------------------------------

#[test]
fn small_ellipse_limits_reduce_then_fall_back_to_fastest_only() {
    // 30×30の格子（間隔150 m）。行0と列29を primary にすると、縮小でもその道は残る。
    let n = 30;
    let (mut builder, shape) = grid(n, n, 150.0);
    for e in 0..builder.edge_count() as u32 {
        let spec = builder.edge_spec(e);
        let (tail, head) = (spec.tail, spec.head);
        let (tr, tc) = (tail as usize / n, tail as usize % n);
        let (hr, hc) = (head as usize / n, head as usize % n);
        let arterial = (tr == 0 && hr == 0) || (tc == n - 1 && hc == n - 1);
        if arterial {
            let spec = builder.edge_spec(e).clone();
            *builder.edge_spec_mut(e) = spec.class(HighwayClass::Primary);
        }
    }
    let world = World::new(&builder, Mode::Car);
    let g = &world.graph;
    let mut rng = Rng::new(31);
    let mut cost = TestCost::random(&mut rng, g);
    let first = builder.dedge(shape.node(0, 0), shape.node(0, 1));
    let last = builder.dedge(shape.node(n - 1, n - 2), shape.node(n - 1, n - 1));
    let start = SnapPoint::on_edge(g, first.edge(), 0);
    let end = SnapPoint::on_edge(g, last.edge(), g.edge(last.edge()).length_dm);
    let cancel = CancelToken::unbounded();
    let unlimited = world.ctx(unlimited_params());
    let p0 = fastest_path(&unlimited, &start, &end, &cancel)
        .unwrap()
        .unwrap();
    let detour_ds = 600 * 5;
    let full = ellipse_subgraph(
        &unlimited, &start, &end, p0.time_ds, detour_ds, &mut cost, &cancel,
    )
    .unwrap();
    let reduced = reduced_ellipse(
        &unlimited, &start, &end, p0.time_ds, detour_ds, &mut cost, &cancel,
    )
    .unwrap();
    assert!(
        reduced.nodes.len() < full.nodes.len(),
        "縮小でノードが減らない: {} < {}",
        reduced.nodes.len(),
        full.nodes.len()
    );

    let limit = reduced.nodes.len() as u32;
    let params = SearchParams {
        ellipse_node_limit: limit,
        rcsp_budget_ms: 200,
        ..SearchParams::DEFAULT
    };
    let ctx = world.ctx(params);
    let result = find_routes(&ctx, &mut cost, &start, &end, 5, &cancel).unwrap();
    assert!(result.stats.reduced);
    assert!(!result.stats.fastest_only);
    // `ellipse_nodes` は仮想ノード（出発・到着）を含む。切り出しの上限は実ノードに掛かる。
    assert!(
        result
            .stats
            .ellipse_nodes
            .is_some_and(|v| v <= limit as usize + 2)
    );

    // さらに小さくすると、縮小でも大きすぎるので p0 だけになる。
    let params = SearchParams {
        ellipse_node_limit: 1,
        rcsp_budget_ms: 50,
        ..SearchParams::DEFAULT
    };
    let ctx = world.ctx(params);
    let result = find_routes(&ctx, &mut cost, &start, &end, 5, &cancel).unwrap();
    assert!(result.stats.reduced);
    assert!(result.stats.fastest_only);
    assert_eq!(result.stats.status, SearchStatus::FastestOnly);
    assert_eq!(result.stats.ellipse_nodes, None);
    assert_eq!(result.candidates.len(), 1);
    assert_eq!(result.candidates[0], result.p0);
}

// ---------------------------------------------------------------------------------------------
// 5. エラー
// ---------------------------------------------------------------------------------------------

#[test]
fn same_position_and_unreachable_are_no_route() {
    let (builder, shape) = grid(3, 3, 100.0);
    let world = World::new(&builder, Mode::Car);
    let g = &world.graph;
    let mut cost = TestCost::zero(g);
    let cancel = CancelToken::unbounded();
    let ctx = world.ctx(unlimited_params());
    let e = builder.dedge(shape.node(0, 0), shape.node(0, 1));
    let same = SnapPoint::on_edge(g, e.edge(), 300);
    assert_eq!(
        find_routes(&ctx, &mut cost, &same, &same, 0, &cancel).unwrap_err(),
        RouteError::NoRoute
    );

    // 2つの連結成分（それぞれ1本の辺）。
    let mut b = GraphBuilder::new();
    let n0 = b.add_node_deg(35.000, 139.000);
    let n1 = b.add_node_deg(35.000, 139.001);
    let n2 = b.add_node_deg(35.010, 139.010);
    let n3 = b.add_node_deg(35.010, 139.011);
    let a = b.add_edge(EdgeSpec::new(n0, n1).length_dm(1000));
    let c = b.add_edge(EdgeSpec::new(n2, n3).length_dm(1000));
    let world = World::new(&b, Mode::Car);
    let g = &world.graph;
    let mut cost = TestCost::zero(g);
    let ctx = world.ctx(unlimited_params());
    let start = SnapPoint::on_edge(g, a, 0);
    let end = SnapPoint::on_edge(g, c, 0);
    assert_eq!(
        find_routes(&ctx, &mut cost, &start, &end, 0, &cancel).unwrap_err(),
        RouteError::NoRoute
    );
}

#[test]
fn fastest_path_longer_than_80km_is_route_too_long() {
    let mut b = GraphBuilder::new();
    let a = b.add_node_deg(35.0, 139.0);
    let c = b.add_node_deg(35.0, 140.0);
    // 90 km（900,000 dm）。80,000 m を超える。
    let e = b.add_edge(EdgeSpec::new(a, c).length_dm(900_000));
    assert!(u64::from(b.edge_spec(e).length_dm.unwrap()) > MAX_ROUTE_DISTANCE_DM);
    let world = World::new(&b, Mode::Car);
    let g = &world.graph;
    let mut cost = TestCost::zero(g);
    let cancel = CancelToken::unbounded();
    let ctx = world.ctx(unlimited_params());
    let start = SnapPoint::on_edge(g, e, 0);
    let end = SnapPoint::on_edge(g, e, g.edge(e).length_dm);
    assert_eq!(
        find_routes(&ctx, &mut cost, &start, &end, 0, &cancel).unwrap_err(),
        RouteError::RouteTooLong
    );
}

// ---------------------------------------------------------------------------------------------
// 6. 取り消し
// ---------------------------------------------------------------------------------------------

#[test]
fn cancelled_token_is_interrupted() {
    let (builder, shape) = grid(3, 3, 100.0);
    let world = World::new(&builder, Mode::Car);
    let g = &world.graph;
    let mut cost = TestCost::zero(g);
    let first = builder.dedge(shape.node(0, 0), shape.node(0, 1));
    let last = builder.dedge(shape.node(2, 1), shape.node(2, 2));
    let start = SnapPoint::on_edge(g, first.edge(), 100);
    let end = SnapPoint::on_edge(g, last.edge(), 300);
    let cancel = CancelToken::unbounded();
    cancel.cancel();
    let ctx = world.ctx(unlimited_params());
    assert_eq!(
        find_routes(&ctx, &mut cost, &start, &end, 0, &cancel).unwrap_err(),
        RouteError::Interrupted
    );
}

// ---------------------------------------------------------------------------------------------
// 7. 乱数のグラフ（seed 固定）
// ---------------------------------------------------------------------------------------------

#[test]
fn random_graphs_keep_candidate_and_risk_invariants() {
    for seed in 0..40_u64 {
        let rows = 4 + (seed as usize % 4);
        let cols = 4 + ((seed / 4) as usize % 4);
        let spacing = 100.0 + ((seed / 7) % 3) as f64 * 20.0;
        let (builder, _shape) = grid(rows, cols, spacing);
        let world = World::new(&builder, Mode::Car);
        let g = &world.graph;
        let mut rng = Rng::new(seed);
        let mut cost = TestCost::random(&mut rng, g);
        let (start, end) = loop {
            let start = random_snap(&mut rng, g);
            let end = random_snap(&mut rng, g);
            if !is_same_position(g, &start, &end) {
                break (start, end);
            }
        };
        let max_detour_min = (seed % 6) as u32;
        let cancel = CancelToken::unbounded();
        let ctx = world.ctx(unlimited_params());
        let result = find_routes(&ctx, &mut cost, &start, &end, max_detour_min, &cancel)
            .unwrap_or_else(|e| panic!("seed {seed}: {e:?}"));
        assert_eq!(
            result.budget_ds,
            result.p0.time_ds + 600 * u64::from(max_detour_min)
        );
        assert_candidates_and_risk(&world, &mut cost, &result);
    }
}
