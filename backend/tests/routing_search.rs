//! 自前経路探索（M3a）の結合試験：ALT、部分グラフ上のリスク最小・LARAC・RCSP・ペナルティ法の代替経路、
//! 候補の整理。合成グラフの上で、全経路の列挙（`Oracle`）と照らして正しさを確かめる。
//! 乱数は seed を固定する。
//!
//! 楕円の切り出し（`ellipse_subgraph`、`reduced_ellipse`）の試験は第2段階で足す。

use std::collections::HashSet;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use accident_route_backend::contract::geo::straight_distance_m;
use accident_route_backend::contract::{CancelToken, Interrupt, Mode};
use accident_route_backend::domain::routing::NONE;
use accident_route_backend::domain::routing::graph::restrictions::{Restrictions, TurnAutomaton};
use accident_route_backend::domain::routing::graph::synthetic::{
    EdgeSpec, GraphBuilder, RandomGraphConfig, Rng, grid, random_graph,
};
use accident_route_backend::domain::routing::graph::time::{
    PerMode, TimeModel, TurnParams, edge_time_ds, node_penalty_ds, partial_edge_time_ds,
    turn_cost_ds,
};
use accident_route_backend::domain::routing::graph::{
    DirEdge, EdgeId, Graph, HighwayClass, node_flags,
};
use accident_route_backend::domain::routing::legality::check_path;
use accident_route_backend::domain::routing::search::alt::{
    Landmarks, UNREACHABLE_Q, edge_only_distances, farthest_landmark_table, select_landmarks,
    straight_lower_bound_ds,
};
use accident_route_backend::domain::routing::search::candidates::{
    MAX_ROUTE_DISTANCE_DM, overlap_ratio, path_length_dm, select_candidates,
};
use accident_route_backend::domain::routing::search::ellipse::{
    EllipseError, NO_DEDGE, Subgraph, ellipse_subgraph, whole_graph_subgraph,
};
use accident_route_backend::domain::routing::search::endpoints::{path_distance_dm, path_time_ds};
use accident_route_backend::domain::routing::search::fastest::fastest_path;
use accident_route_backend::domain::routing::search::larac::larac;
use accident_route_backend::domain::routing::search::pipeline::{
    penalty_alternatives, search_subgraph,
};
use accident_route_backend::domain::routing::search::rcsp::bounded_rcsp;
use accident_route_backend::domain::routing::search::reduced::{
    REDUCED_MAX_CLASS, REDUCED_RADIUS_M, reduced_ellipse,
};
use accident_route_backend::domain::routing::search::{
    ArcCost, Path, SearchContext, SearchParams, SearchStatus,
};
use accident_route_backend::domain::routing::snap::SnapPoint;
use accident_route_backend::domain::routing::topology::{Topology, TwinPair};

// ---------------------------------------------------------------------------------------------
// 共通の部品
// ---------------------------------------------------------------------------------------------

/// ノードの加算・右左折コストが0でないモデル（`TimeModel::synthetic()` は0）。
fn costly_time_model() -> TimeModel {
    let mut model = TimeModel::synthetic();
    model.signal_penalty_s = PerMode {
        walk: 8.0,
        bike: 12.0,
        car: 20.0,
    };
    model.stop_penalty_s = PerMode {
        walk: 0.0,
        bike: 5.0,
        car: 8.0,
    };
    model.level_crossing_penalty_s = PerMode {
        walk: 10.0,
        bike: 15.0,
        car: 30.0,
    };
    let turn = TurnParams {
        right_s: 6.0,
        left_s: 3.0,
        u_turn_s: 15.0,
        straight_max_deg: 30.0,
        u_turn_min_deg: 150.0,
    };
    model.turn = PerMode {
        walk: turn.clone(),
        bike: turn.clone(),
        car: turn,
    };
    model
}

/// 常駐データ一式の代わり（探索が参照するもの）。
struct World {
    graph: Graph,
    restrictions: Restrictions,
    topology: Topology,
    automaton: TurnAutomaton,
    landmarks: Landmarks,
    mode: Mode,
}

/// 辺 `edges` 本ぶんのビットがすべて立ったビット列（余りのビットは0）。
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

/// 時間の予算なし（ラベル数の上限もなし）の探索の設定。
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
    /// 有向辺のリスクは `長さ(dm) × 0〜40`、20%の有向辺はクラスタに入る加算（0〜50,000）を持つ。
    fn random(rng: &mut Rng, graph: &Graph) -> TestCost {
        let mut risk = Vec::new();
        let mut entry = Vec::new();
        let mut length = Vec::new();
        for e in 0..graph.edge_count() as u32 {
            length.push(graph.edge(e).length_dm);
        }
        for d in 0..(2 * graph.edge_count()) as u32 {
            let len = u64::from(length[(d / 2) as usize]);
            risk.push(len * rng.below(41) as u64);
            entry.push(if rng.chance(0.2) {
                rng.below(50_001) as u64
            } else {
                0
            });
        }
        TestCost {
            risk,
            entry,
            length,
        }
    }

    fn with_risk(graph: &Graph, per_dedge: &[(DirEdge, u64)]) -> TestCost {
        let mut cost = TestCost {
            risk: vec![0; 2 * graph.edge_count()],
            entry: vec![0; 2 * graph.edge_count()],
            length: (0..graph.edge_count() as u32)
                .map(|e| graph.edge(e).length_dm)
                .collect(),
        };
        for &(d, r) in per_dedge {
            cost.risk[d.0 as usize] = r;
        }
        cost
    }
}

impl ArcCost for TestCost {
    fn risk_fx(&mut self, d: DirEdge) -> u64 {
        self.risk[d.0 as usize]
    }

    fn partial_risk_fx(&mut self, d: DirEdge, from_dm: u32, to_dm: u32) -> u64 {
        let len = u128::from(self.length[d.edge() as usize]);
        let part = u128::from(to_dm - from_dm);
        ((u128::from(self.risk[d.0 as usize]) * part + len / 2) / len) as u64
    }

    fn cluster_entry_risk_fx(&mut self, d: DirEdge) -> u64 {
        self.entry[d.0 as usize]
    }
}

/// 吸着点（辺と、辺の始点からの距離）。
fn snap_at(graph: &Graph, edge: EdgeId, offset_dm: u32) -> SnapPoint {
    SnapPoint {
        edge,
        offset_dm,
        position: graph.node_coord(graph.edge(edge).tail),
        distance_m: 0.0,
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
    snap_at(graph, edge, offset)
}

fn same_place(graph: &Graph, a: &SnapPoint, b: &SnapPoint) -> bool {
    let key = |p: &SnapPoint| {
        let e = graph.edge(p.edge);
        if p.offset_dm == 0 {
            (0, e.tail, 0)
        } else if p.offset_dm >= e.length_dm {
            (0, e.head, 0)
        } else {
            (1, p.edge, p.offset_dm)
        }
    };
    key(a) == key(b)
}

// ---------------------------------------------------------------------------------------------
// 全経路の列挙（探索とは別に、定義から書く）
// ---------------------------------------------------------------------------------------------

/// 出発・到着の吸着点を持つ経路の定義（`Path` のドキュメントの規則）に従って、有向辺の列から時間と
/// リスクを求める。探索の部品（状態・照合器・部分グラフ）は使わず、グラフの関数と `check_path` だけを使う。
struct Oracle<'a> {
    graph: &'a Graph,
    restrictions: &'a Restrictions,
    mode: Mode,
    start: SnapPoint,
    end: SnapPoint,
    cost: &'a mut TestCost,
}

impl Oracle<'_> {
    fn interior(&self, p: &SnapPoint) -> bool {
        let length = self.graph.edge(p.edge).length_dm;
        p.offset_dm > 0 && p.offset_dm < length
    }

    /// 端のノード（`offset_dm` が0なら始点、辺の長さなら終点）。
    fn end_node(&self, p: &SnapPoint) -> u32 {
        let e = self.graph.edge(p.edge);
        if p.offset_dm == 0 { e.tail } else { e.head }
    }

    /// 有向辺の列が出発から到着までの合法な経路なら、(時間, リスク)。
    fn eval(&mut self, seq: &[DirEdge]) -> Option<(u64, u64)> {
        let g = self.graph;
        let n = seq.len();
        if n == 0 || !check_path(g, self.restrictions, self.mode, seq).is_empty() {
            return None;
        }
        let (start_interior, end_interior) = (self.interior(&self.start), self.interior(&self.end));
        if start_interior {
            if seq[0].edge() != self.start.edge {
                return None;
            }
        } else if g.dedge_from(seq[0]) != self.end_node(&self.start) {
            return None;
        }
        if end_interior {
            if seq[n - 1].edge() != self.end.edge {
                return None;
            }
        } else if g.dedge_to(seq[n - 1]) != self.end_node(&self.end) {
            return None;
        }
        let (mut time, mut risk) = (0_u64, 0_u64);
        for (i, &d) in seq.iter().enumerate() {
            let length = g.edge(d.edge()).length_dm;
            let (mut lo, mut hi) = (0, length);
            let start_partial = i == 0 && start_interior;
            let end_partial = i == n - 1 && end_interior;
            if start_partial {
                let s = self.start.offset_dm;
                if d.is_reverse() {
                    hi = s;
                } else {
                    lo = s;
                }
            }
            if end_partial {
                let t = self.end.offset_dm;
                if d.is_reverse() {
                    lo = lo.max(t);
                } else {
                    hi = hi.min(t);
                }
            }
            if hi <= lo {
                return None;
            }
            let full = (lo, hi) == (0, length);
            time += u64::from(if full {
                edge_time_ds(g, d, self.mode)?
            } else {
                partial_edge_time_ds(g, d, self.mode, hi - lo)?
            });
            risk += if full {
                self.cost.risk_fx(d)
            } else {
                self.cost.partial_risk_fx(d, lo, hi)
            };
            if !end_partial {
                risk += self.cost.cluster_entry_risk_fx(d);
            }
            if i + 1 < n {
                time += u64::from(
                    node_penalty_ds(g, g.dedge_to(d), self.mode)
                        + turn_cost_ds(g, d, seq[i + 1], self.mode),
                );
            }
        }
        Some((time, risk))
    }
}

/// 全ノード対の、辺の時間だけの最短時間（フロイド–ワーシャル）。到達できなければ `u64::MAX / 4`。
fn all_pairs_edge_time(graph: &Graph, mode: Mode) -> Vec<Vec<u64>> {
    const FAR: u64 = u64::MAX / 4;
    let n = graph.node_count();
    let mut dist = vec![vec![FAR; n]; n];
    for (v, row) in dist.iter_mut().enumerate() {
        row[v] = 0;
    }
    for e in 0..graph.edge_count() as u32 {
        for reverse in [false, true] {
            let d = DirEdge::new(e, reverse);
            if let Some(t) = edge_time_ds(graph, d, mode) {
                let (u, v) = (graph.dedge_from(d) as usize, graph.dedge_to(d) as usize);
                dist[u][v] = dist[u][v].min(u64::from(t));
            }
        }
    }
    for k in 0..n {
        for i in 0..n {
            for j in 0..n {
                let via = dist[i][k] + dist[k][j];
                if via < dist[i][j] {
                    dist[i][j] = via;
                }
            }
        }
    }
    dist
}

/// 列挙の結果：時間が上限以下の合法な歩道の (時間, リスク) すべて。
struct Enumerated {
    walks: Vec<(u64, u64)>,
}

impl Enumerated {
    /// 時間が `budget` 以下のもののうち、(リスク, 時間) の辞書順で最小のもの。
    fn best_within(&self, budget: u64) -> Option<(u64, u64)> {
        self.walks
            .iter()
            .filter(|&&(time, _)| time <= budget)
            .map(|&(time, risk)| (risk, time))
            .min()
    }

    fn min_time(&self) -> Option<u64> {
        self.walks.iter().map(|&(time, _)| time).min()
    }

    /// 時間とリスクがどちらも他に劣らない解（パレート解）の時間の昇順。
    fn pareto_times(&self) -> Vec<(u64, u64)> {
        let mut sorted = self.walks.clone();
        sorted.sort_unstable();
        let mut front: Vec<(u64, u64)> = Vec::new();
        for (time, risk) in sorted {
            if front.last().is_none_or(|&(_, r)| risk < r) {
                front.push((time, risk));
            }
        }
        front
    }
}

/// 時間が `budget` 以下の、合法な歩道（同じノードを何度通ってもよい）をすべて列挙する。
/// 訪問が `visit_cap` を超えたら `None`。
fn enumerate_walks(oracle: &mut Oracle<'_>, budget: u64, visit_cap: u64) -> Option<Enumerated> {
    struct Search<'o, 'a> {
        oracle: &'o mut Oracle<'a>,
        budget: u64,
        visits: u64,
        cap: u64,
        remaining_lb: Vec<u64>,
        out: Enumerated,
    }
    impl Search<'_, '_> {
        fn dfs(&mut self, seq: &mut Vec<DirEdge>, acc: u64) -> bool {
            self.visits += 1;
            if self.visits > self.cap {
                return false;
            }
            let g = self.oracle.graph;
            let last = *seq.last().expect("非空");
            let node = g.dedge_to(last);
            // 到着の条件を満たすときだけ、定義どおりに評価する。
            let end = self.oracle.end;
            let ends_here = if self.oracle.interior(&end) {
                last.edge() == end.edge
            } else {
                node == self.oracle.end_node(&end)
            };
            if ends_here
                && let Some((time, risk)) = self.oracle.eval(seq)
                && time <= self.budget
            {
                self.out.walks.push((time, risk));
            }
            if acc.saturating_add(self.remaining_lb[node as usize]) > self.budget {
                return true;
            }
            let mode = self.oracle.mode;
            for &d in g.out_dedges(node) {
                let Some(full) = edge_time_ds(g, d, mode) else {
                    continue;
                };
                seq.push(d);
                if check_path(g, self.oracle.restrictions, mode, seq).is_empty() {
                    let extra = node_penalty_ds(g, node, mode) + turn_cost_ds(g, last, d, mode);
                    if !self.dfs(seq, acc + u64::from(extra) + u64::from(full)) {
                        return false;
                    }
                }
                seq.pop();
            }
            true
        }
    }

    let g = oracle.graph;
    let mode = oracle.mode;
    // 各ノードから到着までの時間の下界（辺の時間だけ）。
    let dist = all_pairs_edge_time(g, mode);
    let end = oracle.end;
    let end_len = g.edge(end.edge).length_dm;
    let mut remaining_lb = vec![u64::MAX / 4; g.node_count()];
    for (v, slot) in remaining_lb.iter_mut().enumerate() {
        if oracle.interior(&end) {
            for reverse in [false, true] {
                let d = DirEdge::new(end.edge, reverse);
                let travelled = if reverse {
                    end_len - end.offset_dm
                } else {
                    end.offset_dm
                };
                if let Some(t) = partial_edge_time_ds(g, d, mode, travelled) {
                    let from = g.dedge_from(d) as usize;
                    *slot = (*slot).min(dist[v][from].saturating_add(u64::from(t)));
                }
            }
        } else {
            *slot = dist[v][oracle.end_node(&end) as usize];
        }
    }
    // 出発の最初の有向辺。
    let start = oracle.start;
    let first: Vec<DirEdge> = if oracle.interior(&start) {
        vec![
            DirEdge::new(start.edge, false),
            DirEdge::new(start.edge, true),
        ]
    } else {
        g.out_dedges(oracle.end_node(&start)).to_vec()
    };
    let mut search = Search {
        oracle,
        budget,
        visits: 0,
        cap: visit_cap,
        remaining_lb,
        out: Enumerated { walks: Vec::new() },
    };
    for d in first {
        let Some(full) = edge_time_ds(g, d, mode) else {
            continue;
        };
        let first_time = if search.oracle.interior(&start) {
            let travelled = if d.is_reverse() {
                start.offset_dm
            } else {
                g.edge(start.edge).length_dm - start.offset_dm
            };
            u64::from(partial_edge_time_ds(g, d, mode, travelled).unwrap_or(full))
        } else {
            u64::from(full)
        };
        let mut seq = vec![d];
        if !search.dfs(&mut seq, first_time) {
            return None;
        }
    }
    Some(search.out)
}

// ---------------------------------------------------------------------------------------------
// ALT
// ---------------------------------------------------------------------------------------------

#[test]
fn alt_lower_bound_never_exceeds_the_true_edge_only_distance() {
    // 表あり（from だけ・from と to）と表なし（直線距離）のどれも、辺の時間だけの最短時間以下。
    let mut rng = Rng::new(20_261_103);
    let mut checked_pairs = 0_u64;
    let mut informative = 0_u64;
    for seed in 0..60_u64 {
        let nodes = 8 + rng.below(30);
        let config = RandomGraphConfig {
            lat_e7: (356_000_000, 356_010_000),
            lon_e7: (1_397_000_000, 1_397_010_000),
            ..RandomGraphConfig::rich(nodes)
        };
        let mut builder = random_graph(seed, &config);
        builder.set_time_model(costly_time_model());
        let graph = builder.build();
        for mode in Mode::ALL {
            let unit = [1, 4, 40][rng.below(3)];
            let count = 1 + rng.below(5);
            let start = rng.below(graph.node_count()) as u32;
            let with_to = rng.chance(0.5);
            let table = farthest_landmark_table(&graph, mode, count, start, unit, with_to);
            let mut tables = [None, None, None];
            tables[accident_route_backend::domain::routing::risk::mode_index(mode)] = Some(table);
            let with_table = Landmarks::from_tables(unit, tables);
            let without = Landmarks::empty();
            for t in 0..graph.node_count() as u32 {
                let to_t = edge_only_distances(&graph, mode, t, true);
                for v in 0..graph.node_count() as u32 {
                    let truth = to_t[v as usize];
                    for lm in [&with_table, &without] {
                        let lb = lm.lower_bound_ds(&graph, mode, v, t);
                        if truth != u32::MAX {
                            assert!(
                                lb <= truth,
                                "seed={seed} {mode:?} v={v} t={t}: lb={lb} > true={truth}"
                            );
                        }
                        // 到達できなくても、下界の関数は panic しない。
                    }
                    checked_pairs += 1;
                    if truth != u32::MAX && with_table.lower_bound_ds(&graph, mode, v, t) > 0 {
                        informative += 1;
                    }
                }
            }
        }
    }
    assert!(checked_pairs > 20_000, "{checked_pairs}");
    assert!(
        informative > checked_pairs / 20,
        "下界が0ばかりでは試験にならない: {informative}/{checked_pairs}"
    );
}

#[test]
fn alt_table_matches_a_hand_computed_path_graph() {
    // 一直線 0-1-2-3-4（各辺 300 m、car 30 km/h → 1辺 360 ds）。
    let (builder, shape) = grid(1, 5, 300.0);
    let graph = builder.build();
    assert_eq!(
        edge_time_ds(&graph, DirEdge::new(0, false), Mode::Car),
        Some(360)
    );
    let dist = edge_only_distances(&graph, Mode::Car, shape.node(0, 1), false);
    assert_eq!(dist, vec![360, 0, 360, 720, 1080]);
    let dist_to = edge_only_distances(&graph, Mode::Car, shape.node(0, 1), true);
    assert_eq!(dist_to, dist, "両方向に通れる辺だけの無向グラフ");

    // 単位 100 ds、ランドマーク1つ（ノード0から最も遠いノード4）。
    let table = farthest_landmark_table(&graph, Mode::Car, 1, shape.node(0, 0), 100, true);
    assert_eq!(table.nodes, vec![4]);
    // d(4→v) = 1440, 1080, 720, 360, 0 → q = floor(d/100)
    assert_eq!(table.from, vec![14, 10, 7, 3, 0]);
    assert_eq!(table.to.as_deref(), Some(&[14u16, 10, 7, 3, 0][..]));
    let mut tables = [None, None, None];
    tables[2] = Some(table);
    let landmarks = Landmarks::from_tables(100, tables);
    // d(0→3) の下界：from の式 100×(q[3] − q[0] − 1) は負、to の式 100×(q[0] − q[3] − 1) = 100×(14−3−1)。
    // 真の値は 1080。
    assert_eq!(landmarks.lower_bound_ds(&graph, Mode::Car, 0, 3), 1000);
    assert_eq!(landmarks.lower_bound_ds(&graph, Mode::Car, 3, 0), 1000);
    // d(1→2)：真の値 360。q は 10 と 7。式は 100×(10−7−1)=200。
    assert_eq!(landmarks.lower_bound_ds(&graph, Mode::Car, 1, 2), 200);
    assert_eq!(landmarks.lower_bound_ds(&graph, Mode::Car, 2, 2), 0);
    // PARAMS の記録。
    assert_eq!(landmarks.params.landmarks.get("car"), Some(&1));
    assert_eq!(
        landmarks.params.directions.get("car"),
        Some(&vec!["from".to_owned(), "to".to_owned()])
    );
    assert!(landmarks.table(Mode::Walk).is_none());
}

#[test]
fn alt_marks_unreachable_nodes_and_skips_those_landmarks() {
    // 0→1→2 の一方通行と、離れた 3-4。ランドマークは start=0 から届くノードだけから選ぶ。
    let mut b = GraphBuilder::new();
    for i in 0..5 {
        b.add_node_deg(35.0, 139.0 + 0.001 * f64::from(i));
    }
    b.add_edge(EdgeSpec::new(0, 1).length_dm(1000).oneway());
    b.add_edge(EdgeSpec::new(1, 2).length_dm(1000).oneway());
    b.add_edge(EdgeSpec::new(3, 4).length_dm(1000));
    let graph = b.build();
    let table = farthest_landmark_table(&graph, Mode::Car, 3, 0, 40, true);
    // start=0 から届くのは 0,1,2。最も遠いのは 2、次は 2 から届かない（距離=到達不能）ノード。
    assert_eq!(table.nodes, vec![2, 0, 1]);
    let l = table.nodes.len();
    // d(2→0)・d(2→1) は到達不能。ノード3,4は、どのランドマークからも届かない。
    assert_eq!(table.from[l], UNREACHABLE_Q);
    assert_eq!(table.from[3 * l], UNREACHABLE_Q);
    assert_eq!(table.from[4 * l + 2], UNREACHABLE_Q);
    let mut tables = [None, None, None];
    tables[2] = Some(table);
    let landmarks = Landmarks::from_tables(40, tables);
    for v in 0..5 {
        for t in 0..5 {
            let truth = edge_only_distances(&graph, Mode::Car, t, true)[v as usize];
            let lb = landmarks.lower_bound_ds(&graph, Mode::Car, v, t);
            if truth != u32::MAX {
                assert!(lb <= truth, "{v}->{t}: {lb} > {truth}");
            }
        }
    }
    // 直線距離の下界も、辺の時間以下。
    for v in 0..3 {
        let truth = edge_only_distances(&graph, Mode::Car, 2, true)[v as usize];
        assert!(straight_lower_bound_ds(&graph, Mode::Car, v, 2) <= truth);
    }
}

#[test]
fn alt_farthest_selection_is_deterministic_and_spread_out() {
    // 10×10の格子：始点から最も遠い角、次にその対角、そのあと残りの角へ。
    let (builder, shape) = grid(10, 10, 100.0);
    let graph = builder.build();
    let start = shape.node(0, 0);
    let a = select_landmarks(&graph, Mode::Car, 4, start);
    let b = select_landmarks(&graph, Mode::Car, 4, start);
    assert_eq!(a, b);
    assert_eq!(a[0], shape.node(9, 9), "始点から最も遠い");
    assert_eq!(
        a[1],
        shape.node(0, 0),
        "それから最も遠い（同じ値は番号の小さい方）"
    );
    // 3つ目：どちらからも9離れたノードのうち番号が最小の (0,9)。4つ目：3つからの最小が9のノードのうち
    // 番号が最小の (5,4)。
    assert_eq!(a, vec![99, 0, shape.node(0, 9), shape.node(5, 4)]);
    // 個数が候補より多ければ、候補の数で止まる。
    let (small, _) = grid(1, 3, 100.0);
    let small = small.build();
    assert_eq!(select_landmarks(&small, Mode::Car, 16, 0).len(), 3);
    assert!(select_landmarks(&small, Mode::Car, 0, 0).is_empty());
    // すべて distinct
    let sixteen = select_landmarks(&graph, Mode::Car, 16, start);
    assert_eq!(sixteen.len(), 16);
    assert_eq!(sixteen.iter().collect::<HashSet<_>>().len(), 16);
}

// ---------------------------------------------------------------------------------------------
// ランダムなグラフでの探索の正しさ（全経路の列挙との照合）
// ---------------------------------------------------------------------------------------------

/// seed から作るランダムな世界：ノード5〜12個、`rich` 設定（一方通行・通行不可・信号・障害物・
/// via-node型とvia-way型の右左折禁止）に、0でないノードの加算と右左折コストのモデルを付ける。
fn random_world(seed: u64) -> (GraphBuilder, Mode, Rng) {
    let mut rng = Rng::new(seed ^ 0x5eed_0000);
    let nodes = 5 + rng.below(8);
    // 辺の長さが 100 m 前後になる小さな矩形（ノードの加算や右左折コストと同じ大きさの時間になる）。
    let config = RandomGraphConfig {
        lat_e7: (356_000_000, 356_004_000),
        lon_e7: (1_397_000_000, 1_397_004_000),
        extra_edges: nodes + rng.below(nodes),
        ..RandomGraphConfig::rich(nodes)
    };
    let mut builder = random_graph(seed, &config);
    builder.set_time_model(costly_time_model());
    (builder, Mode::ALL[(seed % 3) as usize], rng)
}

#[derive(Default, Debug)]
struct CaseStats {
    /// 照合したグラフ（出発・到着）の数。
    checked: u32,
    /// 照合した上限Bの数（1つのグラフにBを複数選ぶ）。
    budgets: u32,
    /// リスク最小の経路が予算を超えた（制約が効いた）件数。
    binding: u32,
    /// LARAC（10回）の解が最適でなかった件数（RCSPが改善した）。
    larac_suboptimal: u32,
    /// ペナルティ法などの代替経路が1本以上あった件数。
    with_alternatives: u32,
    /// 右左折禁止・ノードの加算・右左折コストのいずれかが最短時間経路の時間に影響した件数
    /// （辺の時間だけの最短時間より、合法な最短時間が長い）。
    extras_matter: u32,
    /// 列挙が大きすぎて飛ばした件数。
    skipped: u32,
    /// 列挙した経路の最大数。
    max_walks: u64,
    /// 列挙にかかった時間。
    enumerate_time: Duration,
    /// 楕円が全体グラフの部分グラフより辺が少なかった件数（切り出しが効いた）。
    ellipse_smaller: u32,
}

fn assert_valid_path(oracle: &mut Oracle<'_>, path: &Path, what: &str) {
    assert_eq!(
        oracle.eval(&path.dedges),
        Some((path.time_ds, path.risk_fx)),
        "{what}: 経路が不正、または時間・リスクが定義と違う: {path:?}"
    );
    // 経路の時間・距離の正本（`endpoints`）とも一致する。
    assert_eq!(
        path_time_ds(
            oracle.graph,
            oracle.mode,
            &path.start,
            &path.end,
            &path.dedges
        ),
        Some(path.time_ds),
        "{what}: path_time_ds と違う"
    );
    assert_eq!(
        path_distance_dm(oracle.graph, &path.start, &path.end, &path.dedges),
        Some(path_length_dm(oracle.graph, path)),
        "{what}: path_distance_dm と違う"
    );
}

fn check_random_case(seed: u64, stats: &mut CaseStats) {
    let (builder, mode, mut rng) = random_world(seed);
    let world = World::new(&builder, mode);
    let g = &world.graph;
    if g.edge_count() == 0 {
        return;
    }
    let (start, end) = (random_snap(&mut rng, g), random_snap(&mut rng, g));
    if same_place(g, &start, &end) {
        return;
    }
    let mut cost = TestCost::random(&mut rng, g);
    let ctx = world.ctx(unlimited_params());
    let cancel = CancelToken::unbounded();

    // 予算0で作り、最短時間を求めてから予算を決める。
    let mut sub = whole_graph_subgraph(&ctx, &start, &end, 0, 0, &mut cost, &cancel).unwrap();
    let probe = larac(&ctx, &sub, 10, &cancel).unwrap();
    let Some(fastest) = probe.fastest.clone() else {
        assert!(
            probe.min_risk.is_none(),
            "最短時間経路がないのにリスク最小の経路がある"
        );
        return;
    };
    let t0 = fastest.time_ds;
    // 最短時間の1.7倍までの全歩道を列挙してから、上限Bを選ぶ。Bの7割は、パレート解の時間（制約が効き、
    // LARACが凸包の内側の解を取りこぼしやすい）にする。
    let mut oracle = Oracle {
        graph: g,
        restrictions: &world.restrictions,
        mode,
        start,
        end,
        cost: &mut cost,
    };
    let enumerate_started = Instant::now();
    let found = enumerate_walks(&mut oracle, t0 + t0 * 7 / 10, 120_000);
    stats.enumerate_time += enumerate_started.elapsed();
    let Some(found) = found else {
        stats.skipped += 1;
        return;
    };
    stats.checked += 1;
    stats.max_walks = stats.max_walks.max(found.walks.len() as u64);
    assert_eq!(
        found.min_time(),
        Some(t0),
        "seed={seed}: 合法な最短時間が列挙と違う"
    );
    assert_valid_path(&mut oracle, &fastest, "最短時間経路");
    // 別に実装した `fastest_path`（M1.5）の最短時間が、部分グラフ上の最短時間と一致する。
    let theirs = fastest_path(&ctx, &start, &end, &cancel)
        .unwrap()
        .expect("到達できる");
    assert_eq!(theirs.time_ds, t0, "seed={seed}: fastest_path の時間が違う");
    assert_eq!(
        check_path(g, &world.restrictions, mode, &theirs.dedges),
        vec![],
        "seed={seed}"
    );
    assert_eq!(
        path_time_ds(g, mode, &start, &end, &theirs.dedges),
        Some(t0)
    );
    let min_risk = probe.min_risk.clone().expect("到達できる");
    assert_valid_path(&mut oracle, &min_risk, "リスク最小の経路");

    // 上限Bを複数選ぶ：パレート解の時間（制約が効き、LARACが凸包の内側の解を取りこぼしやすい）から
    // 3つ、最短時間の5〜70%増しから1つ。
    let front = found.pareto_times();
    let mut budgets: Vec<u64> = Vec::new();
    for _ in 0..3 {
        if front.len() > 1 {
            budgets.push(front[rng.below(front.len())].0);
        }
    }
    budgets.push(t0 + (t0 * (5 + rng.below(66)) as u64 / 100).max(1));
    budgets.sort_unstable();
    budgets.dedup();
    for budget in budgets {
        sub.fastest_ds = t0;
        sub.budget_ds = budget;
        check_budget(
            seed,
            &world,
            &ctx,
            &sub,
            &mut oracle,
            &found,
            (&fastest, &min_risk),
            stats,
        );
    }
    count_extras(&world, &start, &end, t0, stats);
}

/// 部分グラフの構造の不変条件：CSR、仮想ノード、辺の時間とリスクが元のグラフと `cost` の値であること。
fn check_subgraph_invariants(graph: &Graph, mode: Mode, sub: &Subgraph, cost: &mut TestCost) {
    let n = sub.nodes.len();
    assert_eq!(sub.node_ofs.len(), n + 3);
    assert_eq!((sub.source as usize, sub.target as usize), (n, n + 1));
    assert_eq!(sub.node_count(), n + 2);
    assert!(sub.node_ofs.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(*sub.node_ofs.last().unwrap() as usize, sub.arcs.len());
    assert!(
        sub.nodes.windows(2).all(|w| w[0] < w[1]),
        "局所番号は元の番号の昇順"
    );
    assert!(
        sub.arcs_of(sub.target).is_empty(),
        "到着の仮想ノードから出る辺はない"
    );
    assert!(sub.arcs.iter().all(|a| (a.head as usize) < n + 2));
    for v in 0..n as u32 {
        let node = sub.nodes[v as usize];
        for arc in sub.arcs_of(v) {
            if arc.dedge == NO_DEDGE {
                assert_eq!((arc.head, arc.time_ds, arc.risk_fx), (sub.target, 0, 0));
            } else if arc.head == sub.target {
                // 到着の辺（途中まで）：到着の辺の有向辺で、出発ノードが v。
                assert_eq!(arc.dedge.edge(), sub.end.edge);
                assert_eq!(graph.dedge_from(arc.dedge), node);
                assert!(graph.passable(arc.dedge, mode));
            } else {
                assert_eq!(graph.dedge_from(arc.dedge), node);
                assert_eq!(sub.nodes[arc.head as usize], graph.dedge_to(arc.dedge));
                assert_eq!(Some(arc.time_ds), edge_time_ds(graph, arc.dedge, mode));
                assert_eq!(
                    arc.risk_fx,
                    cost.risk_fx(arc.dedge) + cost.cluster_entry_risk_fx(arc.dedge)
                );
            }
        }
    }
    for arc in sub.arcs_of(sub.source) {
        if arc.dedge == NO_DEDGE {
            assert_eq!((arc.time_ds, arc.risk_fx), (0, 0));
            assert!((arc.head as usize) < n);
        } else {
            assert_eq!(arc.dedge.edge(), sub.start.edge);
            assert!(graph.passable(arc.dedge, mode));
            if arc.head != sub.target {
                assert_eq!(sub.nodes[arc.head as usize], graph.dedge_to(arc.dedge));
            }
        }
    }
}

/// 上限 `sub.budget_ds` でのLARAC・RCSP・組み合わせ・候補の整理を、列挙の結果と照らす。
#[allow(clippy::too_many_arguments)]
fn check_budget(
    seed: u64,
    world: &World,
    ctx: &SearchContext<'_>,
    sub: &Subgraph,
    oracle: &mut Oracle<'_>,
    found: &Enumerated,
    (fastest, min_risk): (&Path, &Path),
    stats: &mut CaseStats,
) {
    let g = &world.graph;
    let cancel = CancelToken::unbounded();
    let budget = sub.budget_ds;
    stats.budgets += 1;
    let best_expected = found.best_within(budget).expect("最短時間経路自身が入る");
    let opt_risk = best_expected.0;
    assert!(
        min_risk.risk_fx <= opt_risk,
        "seed={seed}: リスク最小の経路のリスクは最適リスク以下"
    );
    if min_risk.time_ds > budget {
        stats.binding += 1;
    } else {
        assert_eq!(
            (min_risk.risk_fx, min_risk.time_ds),
            best_expected,
            "seed={seed}"
        );
    }

    for iterations in [0, 1, 3, 10] {
        let lr = larac(ctx, sub, iterations, &cancel).unwrap();
        let best = lr.best.clone().expect("最短時間経路が予算内なので解がある");
        assert!(
            best.time_ds <= budget,
            "seed={seed}: LARACの解が上限を超えた"
        );
        assert_valid_path(oracle, &best, "LARACの解");
        assert!(
            best.risk_fx >= opt_risk,
            "seed={seed}: LARACの解のリスクが最適リスクより小さい"
        );
        assert!(
            lr.lower_bound_fx <= opt_risk,
            "seed={seed} iterations={iterations}: 下界{} が最適リスク{opt_risk} を超えた",
            lr.lower_bound_fx
        );
        if lr.exact {
            assert_eq!(
                best.risk_fx, opt_risk,
                "seed={seed}: exact なのに最適でない"
            );
        }
        let mut previous_time = 0;
        for point in &lr.hull {
            assert_valid_path(oracle, point, "凸包の点");
            assert!(point.time_ds >= previous_time, "凸包は時間の昇順");
            previous_time = point.time_ds;
        }
        if iterations == 10 && best.risk_fx > opt_risk {
            stats.larac_suboptimal += 1;
        }

        // 時間予算なしのRCSPは、(リスク, 時間) の辞書順で最小の経路を返す。暫定解が何であっても同じ。
        let result = bounded_rcsp(ctx, sub, &lr, &cancel)
            .unwrap()
            .expect("到達できる");
        assert_eq!(result.status, SearchStatus::Optimal, "seed={seed}");
        assert_eq!(result.gap, 0.0);
        assert_eq!(
            (result.path.risk_fx, result.path.time_ds),
            best_expected,
            "seed={seed} iterations={iterations}: RCSPの結果が列挙と違う"
        );
        assert_valid_path(oracle, &result.path, "RCSPの解");
    }

    // 暫定解なしのRCSP（A*の探索そのもの）も同じ。
    let mut no_incumbent = larac(ctx, sub, 0, &cancel).unwrap();
    no_incumbent.best = None;
    let result = bounded_rcsp(ctx, sub, &no_incumbent, &cancel)
        .unwrap()
        .expect("到達できる");
    assert_eq!(
        (result.path.risk_fx, result.path.time_ds),
        best_expected,
        "seed={seed}"
    );

    // 楕円で切った部分グラフ上の最適解は、全体グラフ上の最適解（列挙）と一致する（楕円の安全性）。
    check_subgraph_invariants(g, world.mode, sub, oracle.cost);
    let (start, end) = (oracle.start, oracle.end);
    let ellipse = ellipse_subgraph(
        ctx,
        &start,
        &end,
        sub.fastest_ds,
        budget - sub.fastest_ds,
        &mut *oracle.cost,
        &cancel,
    )
    .unwrap_or_else(|e| panic!("seed={seed}: 楕円を切り出せない: {e:?}"));
    check_subgraph_invariants(g, world.mode, &ellipse, oracle.cost);
    assert!(ellipse.arcs.len() <= sub.arcs.len());
    if ellipse.arcs.len() < sub.arcs.len() {
        stats.ellipse_smaller += 1;
    }
    let lr = larac(ctx, &ellipse, 10, &cancel).unwrap();
    let result = bounded_rcsp(ctx, &ellipse, &lr, &cancel)
        .unwrap()
        .expect("楕円の中に最短時間経路がある");
    assert_eq!(result.status, SearchStatus::Optimal, "seed={seed}");
    assert_eq!(
        (result.path.risk_fx, result.path.time_ds),
        best_expected,
        "seed={seed}: 楕円上の最適解が全体グラフ上と違う"
    );
    assert_valid_path(oracle, &result.path, "楕円上のRCSPの解");

    // 組み合わせ（リスク最小 → LARAC → RCSP → 代替経路 → 候補の整理）。
    let outcome = search_subgraph(ctx, sub, fastest, &cancel)
        .unwrap()
        .expect("到達できる");
    assert_eq!(outcome.status, SearchStatus::Optimal, "seed={seed}");
    assert_eq!(outcome.best.risk_fx, opt_risk, "seed={seed}");
    assert_valid_path(oracle, &outcome.best, "事故考慮経路");
    for path in &outcome.alternatives {
        assert!(
            path.time_ds <= budget,
            "seed={seed}: 代替経路が上限を超えた"
        );
        assert_valid_path(oracle, path, "代替経路");
    }
    if outcome
        .alternatives
        .iter()
        .any(|p| p.dedges != fastest.dedges && p.dedges != outcome.best.dedges)
    {
        stats.with_alternatives += 1;
    }
    let picked = select_candidates(
        g,
        &world.topology,
        fastest.clone(),
        outcome.best.clone(),
        outcome.alternatives.clone(),
        budget,
    );
    assert!(!picked.is_empty() && picked.len() <= 6);
    assert!(picked.windows(2).all(|w| w[0].time_ds <= w[1].time_ds));
    for path in &picked {
        assert!(
            path.time_ds <= budget,
            "seed={seed}: 返す候補が上限を超えた"
        );
        assert!(path_length_dm(g, path) <= MAX_ROUTE_DISTANCE_DM);
        assert_valid_path(oracle, path, "返す候補");
    }
    assert!(
        picked.iter().any(|p| p.dedges == fastest.dedges),
        "p0 は残る"
    );
    assert!(
        picked.iter().any(|p| p.dedges == outcome.best.dedges),
        "best は残る"
    );
}

/// 辺の時間だけで求めた最短時間（下界）より、合法な最短時間が長い（ノードの加算・右左折コスト・禁止が
/// 効いた）件数を数える。
fn count_extras(world: &World, start: &SnapPoint, end: &SnapPoint, t0: u64, stats: &mut CaseStats) {
    let lower = edge_only_distances_snapped(world, start, end);
    if lower.is_some_and(|lower| t0 > lower) {
        stats.extras_matter += 1;
    }
}

/// 出発・到着の吸着点を含む、辺の時間だけの最短時間（ノードの加算・右左折コスト・禁止・通過不可を
/// 無視）。吸着点が辺の途中でも端でも扱う。到達できなければ `None`。
fn edge_only_distances_snapped(world: &World, start: &SnapPoint, end: &SnapPoint) -> Option<u64> {
    let g = &world.graph;
    let mode = world.mode;
    let dist = all_pairs_edge_time(g, mode);
    let far = u64::MAX / 4;
    let interior = |p: &SnapPoint| p.offset_dm > 0 && p.offset_dm < g.edge(p.edge).length_dm;
    // 出発：(到着ノード, そこまでの時間)
    let mut starts: Vec<(usize, u64)> = Vec::new();
    if interior(start) {
        for reverse in [false, true] {
            let d = DirEdge::new(start.edge, reverse);
            let len = g.edge(start.edge).length_dm;
            let travelled = if reverse {
                start.offset_dm
            } else {
                len - start.offset_dm
            };
            if let Some(t) = partial_edge_time_ds(g, d, mode, travelled) {
                starts.push((g.dedge_to(d) as usize, u64::from(t)));
            }
        }
    } else {
        let e = g.edge(start.edge);
        starts.push((
            if start.offset_dm == 0 { e.tail } else { e.head } as usize,
            0,
        ));
    }
    // 同じ辺の途中から途中へ、辺の中だけで着く場合。
    let mut best = far;
    if start.edge == end.edge
        && interior(start)
        && interior(end)
        && start.offset_dm != end.offset_dm
    {
        let reverse = end.offset_dm < start.offset_dm;
        let d = DirEdge::new(start.edge, reverse);
        let travelled = start.offset_dm.abs_diff(end.offset_dm);
        if let Some(t) = partial_edge_time_ds(g, d, mode, travelled) {
            best = u64::from(t);
        }
    }
    let mut ends: Vec<(usize, u64)> = Vec::new();
    if interior(end) {
        for reverse in [false, true] {
            let d = DirEdge::new(end.edge, reverse);
            let len = g.edge(end.edge).length_dm;
            let travelled = if reverse {
                len - end.offset_dm
            } else {
                end.offset_dm
            };
            if let Some(t) = partial_edge_time_ds(g, d, mode, travelled) {
                ends.push((g.dedge_from(d) as usize, u64::from(t)));
            }
        }
    } else {
        let e = g.edge(end.edge);
        ends.push((if end.offset_dm == 0 { e.tail } else { e.head } as usize, 0));
    }
    for &(a, ta) in &starts {
        for &(b, tb) in &ends {
            if dist[a][b] < far {
                best = best.min(ta + dist[a][b] + tb);
            }
        }
    }
    (best < far).then_some(best)
}

#[test]
fn rcsp_matches_exhaustive_enumeration_on_600_random_graphs() {
    // behavior.md 11章「RCSPの正しさ」「LARACの性質」「上限の保証」「右左折禁止」：
    // ノード12個以下のランダムなグラフ600通り（リスクもランダム、rich設定）で、時間予算なしのRCSPの結果
    // （リスク最小、同じなら時間最小）が、時間がB以下の全歩道の列挙と一致する。
    let mut stats = CaseStats::default();
    let mut seed = 0;
    while stats.checked < 600 {
        check_random_case(seed, &mut stats);
        seed += 1;
        assert!(seed < 6000, "照合できた件数が少ない: {stats:?}");
    }
    eprintln!("seeds={seed} {stats:?}");
    assert!(stats.binding >= 300, "制約が効いた件数が少ない: {stats:?}");
    assert!(
        stats.larac_suboptimal >= 20,
        "LARACが最適でない件数が少ない（RCSPの試験にならない）: {stats:?}"
    );
    assert!(
        stats.ellipse_smaller >= 200,
        "楕円が全体グラフより小さかった件数が少ない（楕円の試験にならない）: {stats:?}"
    );
    assert!(
        stats.extras_matter >= 100,
        "禁止・ノードの加算・右左折コストが効いた件数が少ない: {stats:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// 構造のはっきりした例
// ---------------------------------------------------------------------------------------------

/// 辺の長さ（デシメートル）を指定して、2点を結ぶ。`nodes` の座標は `(lat, lon)` の度。
fn small_graph(nodes: &[(f64, f64)], edges: &[(u32, u32, u32)]) -> GraphBuilder {
    let mut b = GraphBuilder::new();
    for &(lat, lon) in nodes {
        b.add_node_deg(lat, lon);
    }
    for &(a, c, length_dm) in edges {
        b.add_edge(EdgeSpec::new(a, c).length_dm(length_dm));
    }
    b
}

/// 有向辺の列から、ノードで出発・到着する `Path`（時間とリスクは呼び出し側が決める）。
fn node_path(graph: &Graph, dedges: Vec<DirEdge>, time_ds: u64, risk_fx: u64) -> Path {
    let first = dedges[0];
    let last = *dedges.last().expect("非空");
    let start_len = graph.edge(first.edge()).length_dm;
    let end_len = graph.edge(last.edge()).length_dm;
    Path {
        start: snap_at(
            graph,
            first.edge(),
            if first.is_reverse() { start_len } else { 0 },
        ),
        end: snap_at(
            graph,
            last.edge(),
            if last.is_reverse() { 0 } else { end_len },
        ),
        dedges,
        time_ds,
        risk_fx,
    }
}

#[test]
fn lagrangian_relaxation_misses_a_solution_inside_the_convex_hull_but_rcsp_finds_it() {
    // plan1.md 1.3節：時間上限12分で、A(10分, リスク10)・B(12分, リスク7)・C(14分, リスク0)。
    // 最適解は B だが、「時間＋λ×リスク」ではどのλでも選ばれない。car 30km/h → 500 m が1分。
    let nodes = [
        (35.000, 139.000), // 0: S
        (35.001, 139.001), // 1: A1
        (35.000, 139.002), // 2: T
        (34.999, 139.001), // 3: B1
        (35.002, 139.001), // 4: C1
    ];
    let edges = [
        (0, 1, 25_000), // A: 2500 m × 2 = 10分
        (1, 2, 25_000),
        (0, 3, 30_000), // B: 3000 m × 2 = 12分
        (3, 2, 30_000),
        (0, 4, 35_000), // C: 3500 m × 2 = 14分
        (4, 2, 35_000),
    ];
    let builder = small_graph(&nodes, &edges);
    let world = World::new(&builder, Mode::Car);
    let g = &world.graph;
    let d = |a, b| builder.dedge(a, b);
    let mut cost = TestCost::with_risk(g, &[(d(0, 1), 10_000), (d(0, 3), 7_000), (d(0, 4), 0)]);
    let ctx = world.ctx(unlimited_params());
    let cancel = CancelToken::unbounded();
    // S から T まで（辺 0 の始点ノード → 辺 3 の終点ノード）。
    let start = snap_at(g, 0, 0);
    let end = snap_at(g, 1, 25_000);
    let minutes = |m: u64| m * 600;
    let mut sub = whole_graph_subgraph(
        &ctx,
        &start,
        &end,
        minutes(10),
        minutes(2),
        &mut cost,
        &cancel,
    )
    .unwrap();
    assert_eq!(sub.budget_ds, minutes(12));

    let result = larac(&ctx, &sub, 10, &cancel).unwrap();
    let pd = result.best.clone().unwrap();
    assert_eq!(
        (pd.time_ds, pd.risk_fx),
        (minutes(10), 10_000),
        "LARACの解は A"
    );
    assert_eq!(pd.dedges, vec![d(0, 1), d(1, 2)]);
    let pc = result.min_risk.clone().unwrap();
    assert_eq!((pc.time_ds, pc.risk_fx), (minutes(14), 0), "リスク最小は C");
    // 重みは α=25, β=6（直線 A–C の法線）。B は直線の上側にあるので、凸包の点にならない。
    assert_eq!(result.weights, Some((25, 6)));
    assert_eq!(result.iterations, 1);
    assert_eq!(result.hull.len(), 2, "凸包の点は A と C だけ");
    // 下界 ceil((25×6000 + 6×10000 − 25×7200) / 6) = 5000。最適リスク7000以下。
    assert_eq!(result.lower_bound_fx, 5_000);
    assert!(!result.exact);

    let rcsp = bounded_rcsp(&ctx, &sub, &result, &cancel).unwrap().unwrap();
    assert_eq!(rcsp.status, SearchStatus::Optimal);
    assert_eq!(
        (rcsp.path.time_ds, rcsp.path.risk_fx),
        (minutes(12), 7_000),
        "最適解は B"
    );
    assert_eq!(rcsp.path.dedges, vec![d(0, 3), d(3, 2)]);

    // 上限を14分にすれば、リスク最小の C が最適。
    sub.budget_ds = minutes(14);
    let outcome = search_subgraph(&ctx, &sub, &pd, &cancel).unwrap().unwrap();
    assert_eq!(outcome.status, SearchStatus::Optimal);
    assert_eq!(outcome.best.dedges, vec![d(0, 4), d(4, 2)]);
    // 上限を10分にすれば、解は A だけ。
    sub.budget_ds = minutes(10);
    let outcome = search_subgraph(&ctx, &sub, &pd, &cancel).unwrap().unwrap();
    assert_eq!(outcome.status, SearchStatus::Optimal);
    assert_eq!(outcome.best.dedges, pd.dedges);
    // 上限が最短時間より小さければ、予算内の経路はなく、最短時間経路を返す。
    sub.budget_ds = minutes(9);
    let outcome = search_subgraph(&ctx, &sub, &pd, &cancel).unwrap().unwrap();
    assert_eq!(outcome.status, SearchStatus::FastestOnly);
    assert_eq!(outcome.best.dedges, pd.dedges);
}

#[test]
fn penalty_alternatives_take_a_different_route_within_the_budget() {
    // 同じ3経路。上限14分なら C（リスク100）が最適で、B（リスク140）は C のリスクを1.5倍（150）にすると
    // 最小になる。1.3倍（130）では C のまま。A（リスク1000）は選ばれない。
    let nodes = [
        (35.000, 139.000),
        (35.001, 139.001),
        (35.000, 139.002),
        (34.999, 139.001),
        (35.002, 139.001),
    ];
    let edges = [
        (0, 1, 25_000),
        (1, 2, 25_000),
        (0, 3, 30_000),
        (3, 2, 30_000),
        (0, 4, 35_000),
        (4, 2, 35_000),
    ];
    let builder = small_graph(&nodes, &edges);
    let world = World::new(&builder, Mode::Car);
    let g = &world.graph;
    let d = |a, b| builder.dedge(a, b);
    let mut cost = TestCost::with_risk(g, &[(d(0, 1), 1_000), (d(0, 3), 140), (d(0, 4), 100)]);
    let ctx = world.ctx(unlimited_params());
    let cancel = CancelToken::unbounded();
    let start = snap_at(g, 0, 0);
    let end = snap_at(g, 1, 25_000);
    let sub = whole_graph_subgraph(&ctx, &start, &end, 6_000, 2_400, &mut cost, &cancel).unwrap();
    let p0 = node_path(g, vec![d(0, 1), d(1, 2)], 6_000, 1_000);
    let outcome = search_subgraph(&ctx, &sub, &p0, &cancel).unwrap().unwrap();
    assert_eq!(outcome.best.dedges, vec![d(0, 4), d(4, 2)]);
    assert_eq!(outcome.status, SearchStatus::Optimal);
    let via_b = vec![d(0, 3), d(3, 2)];
    assert!(
        outcome.alternatives.iter().any(|p| p.dedges == via_b),
        "ペナルティ法で B が代替経路になる: {:?}",
        outcome.alternatives
    );
    // 直接呼ぶと、B だけ（1.3倍では best と同じ経路になり捨てる。1.5倍で B）。返す値は元の時間・リスク。
    let alternatives = penalty_alternatives(&ctx, &sub, &outcome.best, None, &cancel).unwrap();
    assert_eq!(alternatives.len(), 1);
    assert_eq!(alternatives[0].dedges, via_b);
    assert_eq!(
        (alternatives[0].time_ds, alternatives[0].risk_fx),
        (7_200, 140)
    );
    // best が p0 と同じ経路なら、ペナルティ法の代替経路は求めない。
    let p0_is_best = node_path(g, vec![d(0, 4), d(4, 2)], 8_400, 100);
    let outcome = search_subgraph(&ctx, &sub, &p0_is_best, &cancel)
        .unwrap()
        .unwrap();
    assert!(!outcome.alternatives.iter().any(|p| p.dedges == via_b));
}

#[test]
fn rcsp_does_not_compare_labels_that_entered_by_different_edges() {
    // 右左折コストは直前の有向辺に依存する。M へ着く2つのラベルのうち、南から着くもの（S経由）は
    // 時間もリスクも小さいが、M から東の T へ出るのに右折コスト（60秒）がかかり、上限を超える。
    // 西から着くもの（W経由）は直進で出られる。ノードだけで優越判定すると、W 経由を落として解を失う。
    let nodes = [
        (34.998, 138.998), // 0: O（出発）
        (34.998, 139.000), // 1: S（Mの南の側）
        (35.000, 138.998), // 2: W（Mの西の側）
        (35.000, 139.000), // 3: M
        (35.000, 139.002), // 4: T（Mの東）
    ];
    let edges = [
        (0, 1, 1_000), // O→S（東へ）
        (1, 3, 1_000), // S→M（北へ）
        (0, 2, 2_000), // O→W（北へ）
        (2, 3, 2_000), // W→M（東へ）
        (3, 4, 1_000), // M→T（東へ）
    ];
    let mut builder = small_graph(&nodes, &edges);
    let mut model = TimeModel::synthetic();
    model.turn.car.right_s = 60.0;
    model.turn.car.left_s = 60.0;
    model.turn.car.u_turn_s = 60.0;
    builder.set_time_model(model);
    let world = World::new(&builder, Mode::Car);
    let g = &world.graph;
    let d = |a, b| builder.dedge(a, b);
    // 南経由は安全（リスク0）、西経由は危険（リスク5000）。
    let mut cost = TestCost::with_risk(g, &[(d(0, 2), 5_000)]);
    let ctx = world.ctx(unlimited_params());
    let cancel = CancelToken::unbounded();
    let start = snap_at(g, 0, 0);
    let end = snap_at(g, 4, 1_000);
    // 南経由：辺の時間 (1000+1000+1000)dm → 360 ds、S で左折・M で右折 (600+600) ds → 1560。
    // 西経由：(2000+2000+1000)dm → 600 ds、W で右折 600 ds、M は直進 → 1200。
    // M に着いた時点では、南経由 (840 ds) が西経由 (1080 ds) より速い。
    let mut sub = whole_graph_subgraph(&ctx, &start, &end, 1_200, 100, &mut cost, &cancel).unwrap();
    let result = larac(&ctx, &sub, 10, &cancel).unwrap();
    assert_eq!(
        result.fastest.as_ref().unwrap().time_ds,
        1_200,
        "最短時間経路は西経由"
    );
    assert_eq!(
        result.min_risk.as_ref().unwrap().time_ds,
        1_560,
        "リスク最小は南経由"
    );
    let mut no_incumbent = result.clone();
    no_incumbent.best = None;
    let rcsp = bounded_rcsp(&ctx, &sub, &no_incumbent, &cancel)
        .unwrap()
        .unwrap();
    assert_eq!(rcsp.status, SearchStatus::Optimal);
    assert_eq!(rcsp.path.dedges, vec![d(0, 2), d(2, 3), d(3, 4)]);
    assert_eq!((rcsp.path.time_ds, rcsp.path.risk_fx), (1_200, 5_000));
    // 上限を1560に上げれば、南経由（リスク0）が最適。
    sub.budget_ds = 1_560;
    let rcsp = bounded_rcsp(&ctx, &sub, &no_incumbent, &cancel)
        .unwrap()
        .unwrap();
    assert_eq!(rcsp.path.dedges, vec![d(0, 1), d(1, 3), d(3, 4)]);
    assert_eq!((rcsp.path.time_ds, rcsp.path.risk_fx), (1_560, 0));
}

#[test]
fn rcsp_does_not_compare_labels_in_different_automaton_states() {
    // via-way型の禁止 (P→N, N→M, M→T)。N→M を P 経由で通ったラベルは M→T に出られない。Q 経由なら出られる。
    // P 経由の方が時間もリスクも小さいが、M から T へは Z を回ると上限を超える。
    // 直前の有向辺（N→M）が同じでも、照合器の状態で優越判定を分けないと Q 経由の解を失う。
    let nodes = [
        (35.000, 139.000), // 0: O
        (35.001, 139.001), // 1: P
        (34.999, 139.001), // 2: Q
        (35.000, 139.002), // 3: N
        (35.000, 139.003), // 4: M
        (35.000, 139.004), // 5: T
        (35.002, 139.003), // 6: Z
    ];
    let edges = [
        (0, 1, 1_000),
        (0, 2, 1_500),
        (1, 3, 1_000),
        (2, 3, 1_500),
        (3, 4, 1_000),
        (4, 5, 1_000),
        (4, 6, 3_000),
        (6, 5, 3_000),
    ];
    let mut builder = small_graph(&nodes, &edges);
    builder.forbid_nodes(&[1, 3, 4, 5], Mode::Car.mask());
    let world = World::new(&builder, Mode::Car);
    let g = &world.graph;
    let d = |a, b| builder.dedge(a, b);
    // Q 経由は危険。
    let mut cost = TestCost::with_risk(g, &[(d(0, 2), 500), (d(2, 3), 500)]);
    let ctx = world.ctx(unlimited_params());
    let cancel = CancelToken::unbounded();
    let start = snap_at(g, 0, 0);
    let end = snap_at(g, 5, 1_000);
    // Q 経由 (1500+1500+1000+1000)dm = 5000dm → 600 ds。Z 経由は P 経由でも 9000dm → 1080 ds。
    let sub = whole_graph_subgraph(&ctx, &start, &end, 600, 100, &mut cost, &cancel).unwrap();
    let result = larac(&ctx, &sub, 10, &cancel).unwrap();
    let fastest = result.fastest.as_ref().or(result.best.as_ref()).unwrap();
    assert_eq!(fastest.dedges, vec![d(0, 2), d(2, 3), d(3, 4), d(4, 5)]);
    assert_eq!(fastest.time_ds, 600);
    let mut no_incumbent = result.clone();
    no_incumbent.best = None;
    let rcsp = bounded_rcsp(&ctx, &sub, &no_incumbent, &cancel)
        .unwrap()
        .unwrap();
    assert_eq!(rcsp.status, SearchStatus::Optimal);
    assert_eq!(rcsp.path.dedges, fastest.dedges);
    assert_eq!(
        check_path(g, &world.restrictions, Mode::Car, &rcsp.path.dedges),
        vec![]
    );
    // 禁止の列を使う P 経由の経路は、どの探索も返さない。
    let outcome = search_subgraph(&ctx, &sub, fastest, &cancel)
        .unwrap()
        .unwrap();
    for path in std::iter::once(&outcome.best).chain(&outcome.alternatives) {
        assert_eq!(
            check_path(g, &world.restrictions, Mode::Car, &path.dedges),
            vec![]
        );
    }
}

#[test]
fn a_forbidden_turn_can_force_a_loop_around_a_block() {
    // A→B→X は禁止（via-node型）。B には小さな一方通行の周回路 B→P→Q→B がある。A から X へ
    // 行くには、周回して B に別の辺から戻る（同じノードを2回通る）しかない。状態付きの探索は周回を許し、
    // 時間は有限で終わる。
    let nodes = [
        (35.000, 139.000), // 0: A
        (35.000, 139.001), // 1: B
        (35.001, 139.001), // 2: X
        (34.999, 139.002), // 3: P
        (34.999, 139.001), // 4: Q
    ];
    let edges = [
        (0, 1, 1_000),
        (1, 2, 1_000),
        (1, 3, 1_000),
        (3, 4, 1_000),
        (4, 1, 1_000),
    ];
    let mut builder = small_graph(&nodes, &edges);
    // B→P→Q→B は一方通行の周回路（途中で引き返せない）。
    for e in [2, 4] {
        let spec = builder.edge_spec(e).clone();
        *builder.edge_spec_mut(e) = spec.oneway();
    }
    builder.forbid_nodes(&[0, 1, 2], Mode::Car.mask());
    let world = World::new(&builder, Mode::Car);
    let g = &world.graph;
    let d = |a, b| builder.dedge(a, b);
    let mut cost = TestCost::with_risk(g, &[]);
    let ctx = world.ctx(unlimited_params());
    let cancel = CancelToken::unbounded();
    let start = snap_at(g, 0, 0);
    let end = snap_at(g, 1, 1_000);
    let mut sub = whole_graph_subgraph(&ctx, &start, &end, 0, 0, &mut cost, &cancel).unwrap();
    let result = larac(&ctx, &sub, 10, &cancel).unwrap();
    let fastest = result.fastest.expect("周回すれば着く");
    assert_eq!(
        fastest.dedges,
        vec![d(0, 1), d(1, 3), d(3, 4), d(4, 1), d(1, 2)],
        "B を2回通る"
    );
    // 5辺 × 100 m = 500 m → 60秒。
    assert_eq!(fastest.time_ds, 600);
    assert_eq!(
        check_path(g, &world.restrictions, Mode::Car, &fastest.dedges),
        vec![]
    );
    // 禁止の列を含む近道は、他の手段（walk）では通れる。
    let walk = TurnAutomaton::build(&world.restrictions, Mode::Walk);
    assert_eq!(walk.state_count(), 1, "walk には禁止がない");
    let walk_world = World::new(&builder, Mode::Walk);
    let walk_ctx = walk_world.ctx(unlimited_params());
    sub.budget_ds = 0;
    let direct = larac(&walk_ctx, &sub, 10, &cancel)
        .unwrap()
        .fastest
        .unwrap();
    assert_eq!(direct.dedges, vec![d(0, 1), d(1, 2)]);
}

#[test]
fn blocked_nodes_can_be_endpoints_but_not_passed_through() {
    // 3×3の格子で中央 (1,1) を car が通過できない。中央を出発・到着にはできるが、通過はできない。
    let (mut builder, shape) = grid(3, 3, 100.0);
    let center = shape.node(1, 1);
    builder.add_node_flags(center, node_flags::BLOCK_CAR);
    let world = World::new(&builder, Mode::Car);
    let g = &world.graph;
    let mut rng = Rng::new(7);
    let mut cost = TestCost::random(&mut rng, g);
    let ctx = world.ctx(unlimited_params());
    let cancel = CancelToken::unbounded();
    let d = |a: (usize, usize), b: (usize, usize)| {
        builder.dedge(shape.node(a.0, a.1), shape.node(b.0, b.1))
    };
    // (1,0) → (1,2)：中央を通れないので、回り道（4辺）。
    let start = snap_at(g, d((1, 0), (1, 1)).edge(), 0);
    let end = snap_at(g, d((1, 1), (1, 2)).edge(), 1_000);
    let sub = whole_graph_subgraph(&ctx, &start, &end, 0, 0, &mut cost, &cancel).unwrap();
    let fastest = larac(&ctx, &sub, 10, &cancel).unwrap().fastest.unwrap();
    assert_eq!(fastest.dedges.len(), 4);
    assert_eq!(
        check_path(g, &world.restrictions, Mode::Car, &fastest.dedges),
        vec![]
    );
    // 中央から出発して (1,2) へ：1辺。
    let start = snap_at(g, d((1, 1), (1, 2)).edge(), 0);
    let sub = whole_graph_subgraph(&ctx, &start, &end, 0, 0, &mut cost, &cancel).unwrap();
    let fastest = larac(&ctx, &sub, 10, &cancel).unwrap().fastest.unwrap();
    assert_eq!(fastest.dedges, vec![d((1, 1), (1, 2))]);
    // (1,0) から中央へ着く：1辺。
    let start = snap_at(g, d((1, 0), (1, 1)).edge(), 0);
    let end = snap_at(g, d((1, 0), (1, 1)).edge(), 1_000);
    let sub = whole_graph_subgraph(&ctx, &start, &end, 0, 0, &mut cost, &cancel).unwrap();
    let fastest = larac(&ctx, &sub, 10, &cancel).unwrap().fastest.unwrap();
    assert_eq!(fastest.dedges, vec![d((1, 0), (1, 1))]);
}

#[test]
fn endpoints_in_the_middle_of_edges_use_partial_edges() {
    // 一直線 0-1-2（各辺 100 m）。辺0の 30 m の点から辺1の 40 m の点まで。
    let (builder, shape) = grid(1, 3, 100.0);
    let world = World::new(&builder, Mode::Car);
    let g = &world.graph;
    let mut cost = TestCost::with_risk(g, &[]);
    let ctx = world.ctx(unlimited_params());
    let cancel = CancelToken::unbounded();
    let fwd = |a, b| builder.dedge(shape.node(0, a), shape.node(0, b));
    let (e0, e1) = (fwd(0, 1).edge(), fwd(1, 2).edge());
    let start = snap_at(g, e0, 300);
    let end = snap_at(g, e1, 400);
    let sub = whole_graph_subgraph(&ctx, &start, &end, 0, 0, &mut cost, &cancel).unwrap();
    let fastest = larac(&ctx, &sub, 10, &cancel).unwrap().fastest.unwrap();
    // 70 m + 40 m = 110 m → 13.2秒 → 部分の時間は 70 m が 84 ds、40 m が 48 ds。
    assert_eq!(fastest.dedges, vec![fwd(0, 1), fwd(1, 2)]);
    assert_eq!(fastest.time_ds, 84 + 48);
    // 同じ辺の途中から途中へ：辺0の 30 m から 80 m。進行方向なら1つの部分有向辺。
    let end_same = snap_at(g, e0, 800);
    let sub = whole_graph_subgraph(&ctx, &start, &end_same, 0, 0, &mut cost, &cancel).unwrap();
    let fastest = larac(&ctx, &sub, 10, &cancel).unwrap().fastest.unwrap();
    assert_eq!(fastest.dedges, vec![fwd(0, 1)]);
    assert_eq!(fastest.time_ds, 60);
    // 戻る向きも（両方向に通れる）。
    let sub = whole_graph_subgraph(&ctx, &end_same, &start, 0, 0, &mut cost, &cancel).unwrap();
    let fastest = larac(&ctx, &sub, 10, &cancel).unwrap().fastest.unwrap();
    assert_eq!(fastest.dedges, vec![fwd(1, 0)]);
    // 一方通行で戻れないときは、周回できないので到達できない（行き止まり）。
    let (mut one_way, shape) = grid(1, 3, 100.0);
    let e = one_way.dedge(shape.node(0, 0), shape.node(0, 1)).edge();
    let spec = one_way.edge_spec(e).clone();
    *one_way.edge_spec_mut(e) = spec.oneway();
    let world = World::new(&one_way, Mode::Car);
    let ctx = world.ctx(unlimited_params());
    let mut cost = TestCost::with_risk(&world.graph, &[]);
    let sub = whole_graph_subgraph(&ctx, &end_same, &start, 0, 0, &mut cost, &cancel).unwrap();
    assert!(larac(&ctx, &sub, 10, &cancel).unwrap().fastest.is_none());
}

// ---------------------------------------------------------------------------------------------
// 打ち切りと中断
// ---------------------------------------------------------------------------------------------

/// 格子の上の探索（car、右左折コストあり）。`n`×`n` の格子で、左下から右上へ。
struct GridCase {
    world: World,
    sub: Subgraph,
    fastest: Path,
}

fn grid_case(n: usize, detour_percent: u64, seed: u64) -> GridCase {
    let (mut builder, shape) = grid(n, n, 60.0);
    builder.set_time_model(costly_time_model());
    let world = World::new(&builder, Mode::Car);
    let g = &world.graph;
    let mut rng = Rng::new(seed);
    let mut cost = TestCost::random(&mut rng, g);
    let ctx = world.ctx(unlimited_params());
    let cancel = CancelToken::unbounded();
    let first = builder.dedge(shape.node(0, 0), shape.node(0, 1));
    let last = builder.dedge(shape.node(n - 1, n - 2), shape.node(n - 1, n - 1));
    let start = snap_at(g, first.edge(), 0);
    let end = snap_at(g, last.edge(), g.edge(last.edge()).length_dm);
    let mut sub = whole_graph_subgraph(&ctx, &start, &end, 0, 0, &mut cost, &cancel).unwrap();
    let fastest = larac(&ctx, &sub, 10, &cancel).unwrap().fastest.unwrap();
    sub.fastest_ds = fastest.time_ds;
    sub.budget_ds = fastest.time_ds + fastest.time_ds * detour_percent / 100;
    GridCase {
        world,
        sub,
        fastest,
    }
}

#[test]
fn rcsp_stopped_by_budget_or_label_limit_is_never_recorded_as_optimal() {
    // LARACが最適を確認できない（凸包の内側に最適解がある）格子を探す。
    let (case, lr) = (0..40_u64)
        .map(|seed| grid_case(8, 12, seed))
        .find_map(|case| {
            let ctx = case.world.ctx(unlimited_params());
            let lr = larac(&ctx, &case.sub, 10, &CancelToken::unbounded()).unwrap();
            (!lr.exact).then_some((case, lr))
        })
        .expect("LARACが最適を確認できない例がある");
    let cancel = CancelToken::unbounded();
    let full = bounded_rcsp(&case.world.ctx(unlimited_params()), &case.sub, &lr, &cancel)
        .unwrap()
        .unwrap();
    assert_eq!(full.status, SearchStatus::Optimal);
    let incumbent = lr.best.clone().unwrap();
    assert!(full.path.risk_fx <= incumbent.risk_fx);

    // 時間予算0ms：1回も展開せずに、暫定解（LARACの解）を返す。
    let zero_budget = SearchParams {
        rcsp_budget_ms: 0,
        ..unlimited_params()
    };
    let stopped = bounded_rcsp(&case.world.ctx(zero_budget), &case.sub, &lr, &cancel)
        .unwrap()
        .unwrap();
    assert_eq!(stopped.status, SearchStatus::Bounded);
    assert_eq!(stopped.path, incumbent);
    assert!(stopped.path.time_ds <= case.sub.budget_ds);
    assert!(stopped.gap.is_finite() && stopped.gap >= 0.0);

    // ラベル数の上限：途中で止まる。暫定解より悪くならず、上限を守り、最適とは記録しない。
    let mut bounded = 0;
    for limit in [1_u32, 2, 5, 20, 50, 200] {
        let limited = SearchParams {
            rcsp_label_limit: limit,
            ..unlimited_params()
        };
        let result = bounded_rcsp(&case.world.ctx(limited), &case.sub, &lr, &cancel)
            .unwrap()
            .unwrap();
        if result.status == SearchStatus::Optimal {
            // 上限より少ないラベルで終われば、最適を確認したことになる。
            assert_eq!(result.path, full.path, "limit={limit}");
            continue;
        }
        bounded += 1;
        assert_eq!(result.status, SearchStatus::Bounded, "limit={limit}");
        assert!(result.path.time_ds <= case.sub.budget_ds);
        assert!(result.path.risk_fx <= incumbent.risk_fx);
        assert!(result.path.risk_fx >= full.path.risk_fx);
        // 下界は最適リスク以下：リスク − 下界 = gap×リスク。
        let lower = result.path.risk_fx as f64 * (1.0 - result.gap);
        assert!(
            lower <= full.path.risk_fx as f64 + 1.0,
            "limit={limit}: 下界{lower} > 最適{}",
            full.path.risk_fx
        );
    }
    assert!(
        bounded >= 2,
        "ラベル数の上限で止まった例が少ない: {bounded}"
    );

    // 組み合わせでも同じ：時間予算0なら、LARACで最適を確認できないので最適とは記録しない。
    let outcome = search_subgraph(
        &case.world.ctx(zero_budget),
        &case.sub,
        &case.fastest,
        &cancel,
    )
    .unwrap()
    .unwrap();
    assert_eq!(outcome.status, SearchStatus::Bounded);
    assert_eq!(outcome.best, incumbent);

    // 時間予算1ms：最適を返すか、打ち切って上限内の経路を返すか（どちらでも正しい）。
    let one_ms = SearchParams {
        rcsp_budget_ms: 1,
        ..unlimited_params()
    };
    let big = grid_case(40, 8, 3);
    let big_ctx = big.world.ctx(one_ms);
    let big_lr = larac(&big_ctx, &big.sub, 10, &cancel).unwrap();
    let started = Instant::now();
    let result = bounded_rcsp(&big_ctx, &big.sub, &big_lr, &cancel)
        .unwrap()
        .unwrap();
    assert!(result.path.time_ds <= big.sub.budget_ds);
    assert!(result.path.risk_fx <= big_lr.best.as_ref().unwrap().risk_fx);
    if result.status == SearchStatus::Optimal {
        assert_eq!(result.gap, 0.0);
    } else {
        assert_eq!(result.status, SearchStatus::Bounded);
        // 1msの予算なら、展開をあまり進められない。
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
    }
}

#[test]
fn every_stage_returns_the_interrupt_when_cancelled() {
    let case = grid_case(6, 15, 1);
    let ctx = case.world.ctx(unlimited_params());
    let cancelled = CancelToken::unbounded();
    cancelled.cancel();
    let lr_ok = larac(&ctx, &case.sub, 10, &CancelToken::unbounded()).unwrap();
    assert_eq!(
        larac(&ctx, &case.sub, 10, &cancelled).unwrap_err(),
        Interrupt::Cancelled
    );
    assert_eq!(
        bounded_rcsp(&ctx, &case.sub, &lr_ok, &cancelled).unwrap_err(),
        Interrupt::Cancelled
    );
    assert_eq!(
        search_subgraph(&ctx, &case.sub, &case.fastest, &cancelled).unwrap_err(),
        Interrupt::Cancelled
    );
    let best = lr_ok.best.clone().unwrap();
    assert_eq!(
        penalty_alternatives(&ctx, &case.sub, &best, None, &cancelled).unwrap_err(),
        Interrupt::Cancelled
    );
    let mut cost = TestCost::with_risk(&case.world.graph, &[]);
    assert_eq!(
        whole_graph_subgraph(
            &ctx,
            &case.sub.start,
            &case.sub.end,
            0,
            0,
            &mut cost,
            &cancelled
        )
        .unwrap_err(),
        Interrupt::Cancelled
    );
    // 期限切れ。
    let expired = CancelToken::with_timeout(Duration::ZERO);
    assert_eq!(
        larac(&ctx, &case.sub, 10, &expired).unwrap_err(),
        Interrupt::DeadlineExceeded
    );
}

#[test]
fn cancelling_a_running_search_returns_interrupted_within_about_10ms() {
    // 大きめの格子（200×200＝4万ノード）で探索を回し、別スレッドから取り消す。取り消してから
    // `Interrupted` で返るまでの時間を測る（約1024回の展開ごとに `cancel.check()`）。
    let case = grid_case(200, 5, 11);
    let ctx = case.world.ctx(unlimited_params());
    let mut worst = Duration::ZERO;
    for round in 0..4_u64 {
        let token = CancelToken::unbounded();
        let (sender, receiver) = mpsc::channel();
        let delay = Duration::from_millis(30 + 40 * round);
        let canceller = {
            let token = token.clone();
            std::thread::spawn(move || {
                std::thread::sleep(delay);
                let at = Instant::now();
                token.cancel();
                sender.send(at).unwrap();
            })
        };
        // 取り消されるまで探索を繰り返す（1回の探索は取消しの前には終わらない大きさ）。
        let mut completed = 0;
        let outcome = loop {
            match search_subgraph(&ctx, &case.sub, &case.fastest, &token) {
                Ok(_) => completed += 1,
                Err(interrupt) => break interrupt,
            }
            assert!(completed < 3, "取り消しの前に探索が何度も終わった");
        };
        let returned = Instant::now();
        let cancelled_at = receiver.recv().unwrap();
        canceller.join().unwrap();
        assert_eq!(outcome, Interrupt::Cancelled);
        let latency = returned.saturating_duration_since(cancelled_at);
        worst = worst.max(latency);
    }
    eprintln!("取消しから返るまで（最悪）: {worst:?}");
    // 約10ms。デバッグビルドの遅さと実行環境の揺らぎを見て、判定は余裕を持たせる。
    assert!(worst < Duration::from_millis(40), "{worst:?}");
}

// ---------------------------------------------------------------------------------------------
// 候補の整理
// ---------------------------------------------------------------------------------------------

/// 3×7の格子（間隔 100 m、辺の長さ 1000 dm）。ノードの列を辿る `Path` を作る。
struct CandidateWorld {
    builder: GraphBuilder,
    graph: Graph,
    topology: Topology,
    shape: accident_route_backend::domain::routing::graph::synthetic::GridShape,
}

impl CandidateWorld {
    fn new() -> CandidateWorld {
        let (builder, shape) = grid(3, 7, 100.0);
        let graph = builder.build();
        let topology = empty_topology(&graph);
        CandidateWorld {
            builder,
            graph,
            topology,
            shape,
        }
    }

    /// 格子の (行, 列) の列を辿る。時間は呼び出し側が決める。
    fn path(&self, cells: &[(usize, usize)], time_ds: u64) -> Path {
        let nodes: Vec<u32> = cells.iter().map(|&(r, c)| self.shape.node(r, c)).collect();
        node_path(&self.graph, self.builder.dedges(&nodes), time_ds, 0)
    }
}

fn row(r: usize, from: usize, to: usize) -> Vec<(usize, usize)> {
    (from..=to).map(|c| (r, c)).collect()
}

#[test]
fn overlap_ratio_counts_shared_physical_length_relative_to_the_candidate() {
    let w = CandidateWorld::new();
    // 行0を (0,0)→(0,5)（5辺）と、行0の4辺＋縦1辺（(0,0)→(0,4)→(1,4)）。
    let a = w.path(&row(0, 0, 5), 600);
    let mut cells = row(0, 0, 4);
    cells.push((1, 4));
    let b = w.path(&cells, 600);
    assert_eq!(overlap_ratio(&w.graph, &w.topology, &b, &a), 4.0 / 5.0);
    assert_eq!(overlap_ratio(&w.graph, &w.topology, &a, &b), 4.0 / 5.0);
    // 長さが違うと、比べる側（第1引数）の長さが分母になる：a は5辺、c は a + 縦1辺 = 6辺。
    let mut cells = row(0, 0, 5);
    cells.push((1, 5));
    let c = w.path(&cells, 700);
    assert_eq!(overlap_ratio(&w.graph, &w.topology, &c, &a), 5.0 / 6.0);
    assert_eq!(overlap_ratio(&w.graph, &w.topology, &a, &c), 1.0);
    // 逆向きに通っても同じ物理区間。
    let reversed: Vec<(usize, usize)> = row(0, 0, 5).into_iter().rev().collect();
    let r = w.path(&reversed, 600);
    assert_eq!(overlap_ratio(&w.graph, &w.topology, &r, &a), 1.0);
    // 共有なし。
    let d = w.path(&row(2, 0, 5), 600);
    assert_eq!(overlap_ratio(&w.graph, &w.topology, &d, &a), 0.0);
}

#[test]
fn overlap_ratio_counts_only_the_overlapped_part_of_partial_edges() {
    let w = CandidateWorld::new();
    let g = &w.graph;
    let e = w.builder.dedge(w.shape.node(0, 0), w.shape.node(0, 1));
    let e2 = w.builder.dedge(w.shape.node(0, 1), w.shape.node(0, 2));
    // a：辺 e の 200 dm から終点まで、続けて e2 の 400 dm まで（800 + 400 = 1200 dm）。
    let a = Path {
        dedges: vec![e, e2],
        start: snap_at(g, e.edge(), 200),
        end: snap_at(g, e2.edge(), 400),
        time_ds: 0,
        risk_fx: 0,
    };
    // b：辺 e の 0 から 500 dm まで（同じ辺に1本だけ：始点から 500 dm）。
    let b = Path {
        dedges: vec![e],
        start: snap_at(g, e.edge(), 0),
        end: snap_at(g, e.edge(), 500),
        time_ds: 0,
        risk_fx: 0,
    };
    assert_eq!(path_length_dm(g, &a), 1200);
    assert_eq!(path_length_dm(g, &b), 500);
    // 共有は e の [200, 500] の 300 dm。
    assert_eq!(overlap_ratio(g, &w.topology, &a, &b), 300.0 / 1200.0);
    assert_eq!(overlap_ratio(g, &w.topology, &b, &a), 300.0 / 500.0);
    // 逆向きの部分有向辺：e を終点側から 700 dm 戻る（始点からの 300 dm 〜 1000 dm）。
    let back = Path {
        dedges: vec![e.reversed()],
        start: snap_at(g, e.edge(), 700),
        end: snap_at(g, e.edge(), 0),
        time_ds: 0,
        risk_fx: 0,
    };
    assert_eq!(path_length_dm(g, &back), 700);
    // 区間 [0,700] と [200,1000]... ではなく、back は始点側 0〜700 dm（offset 700 から始点へ戻る）。
    assert_eq!(overlap_ratio(g, &w.topology, &back, &b), 500.0 / 700.0);
    // 同じ辺の途中から途中へ（進行方向の1本）。
    let inner = Path {
        dedges: vec![e],
        start: snap_at(g, e.edge(), 300),
        end: snap_at(g, e.edge(), 800),
        time_ds: 0,
        risk_fx: 0,
    };
    assert_eq!(path_length_dm(g, &inner), 500);
    assert_eq!(overlap_ratio(g, &w.topology, &inner, &b), 200.0 / 500.0);
}

#[test]
fn twin_edges_are_the_same_physical_section() {
    // 上下線の双子：u→v の一方通行 e と、v'→u' の一方通行 t（u≈u'、v≈v'）。
    let nodes = [
        (35.0000, 139.0000), // 0: u
        (35.0000, 139.0010), // 1: v
        (35.0001, 139.0010), // 2: v'
        (35.0001, 139.0000), // 3: u'
    ];
    let mut b = GraphBuilder::new();
    for &(lat, lon) in &nodes {
        b.add_node_deg(lat, lon);
    }
    let e = b.add_edge(EdgeSpec::new(0, 1).length_dm(1000).oneway());
    let t = b.add_edge(EdgeSpec::new(2, 3).length_dm(1000).oneway());
    let graph = b.build();
    let mut topology = empty_topology(&graph);
    topology.twins = vec![TwinPair { a: e, b: t }, TwinPair { a: t, b: e }];
    assert_eq!(topology.validate(&graph), Ok(()));
    let fwd_e = DirEdge::new(e, false);
    let fwd_t = DirEdge::new(t, false);
    let full = |d: DirEdge| node_path(&graph, vec![d], 100, 0);
    assert_eq!(
        overlap_ratio(&graph, &topology, &full(fwd_e), &full(fwd_t)),
        1.0
    );
    assert_eq!(
        overlap_ratio(&graph, &topology, &full(fwd_t), &full(fwd_e)),
        1.0
    );
    // 双子でなければ重ならない。
    let plain = empty_topology(&graph);
    assert_eq!(
        overlap_ratio(&graph, &plain, &full(fwd_e), &full(fwd_t)),
        0.0
    );
    // 部分：e の始点から 400 dm までと、t の（向きが逆なので）終点側 400 dm（始点からの 600〜1000）は
    // 同じ区間（e の 0〜400 に写る）。t の始点側 400 dm（始点から 0〜400 → e の 600〜1000）は重ならない。
    let part_e = Path {
        dedges: vec![fwd_e],
        start: snap_at(&graph, e, 0),
        end: snap_at(&graph, e, 400),
        time_ds: 0,
        risk_fx: 0,
    };
    let part_t_same_place = Path {
        dedges: vec![fwd_t],
        start: snap_at(&graph, t, 600),
        end: snap_at(&graph, t, 1000),
        time_ds: 0,
        risk_fx: 0,
    };
    let part_t_other_end = Path {
        dedges: vec![fwd_t],
        start: snap_at(&graph, t, 0),
        end: snap_at(&graph, t, 400),
        time_ds: 0,
        risk_fx: 0,
    };
    let ratio = overlap_ratio(&graph, &topology, &part_e, &part_t_same_place);
    assert!((ratio - 1.0).abs() < 1e-9, "{ratio}");
    assert_eq!(
        overlap_ratio(&graph, &topology, &part_e, &part_t_other_end),
        0.0
    );
}

#[test]
fn many_to_many_twins_count_only_the_stretch_they_run_alongside() {
    // 東行きの長い辺 e（u→v、3000 dm）と、その北10mを西へ向かう3本の辺 t1・t2・t3（各1000 dm。東の端から順に）。
    // e が3本すべての双子（多対多）。t1〜t3 どうしは同じ向きで、双子ではない。
    let nodes = [
        (35.0000, 139.0000), // 0: u
        (35.0000, 139.0030), // 1: v
        (35.0001, 139.0030), // 2: t3の始点
        (35.0001, 139.0020), // 3: t3の終点＝t2の始点
        (35.0001, 139.0010), // 4: t2の終点＝t1の始点
        (35.0001, 139.0000), // 5: t1の終点
    ];
    let mut b = GraphBuilder::new();
    for &(lat, lon) in &nodes {
        b.add_node_deg(lat, lon);
    }
    let e = b.add_edge(EdgeSpec::new(0, 1).length_dm(3000).oneway());
    let t3 = b.add_edge(EdgeSpec::new(2, 3).length_dm(1000).oneway());
    let t2 = b.add_edge(EdgeSpec::new(3, 4).length_dm(1000).oneway());
    let t1 = b.add_edge(EdgeSpec::new(4, 5).length_dm(1000).oneway());
    let graph = b.build();
    let mut topology = empty_topology(&graph);
    topology.twins = [(e, t1), (e, t2), (e, t3)]
        .iter()
        .flat_map(|&(a, b)| [TwinPair { a, b }, TwinPair { a: b, b: a }])
        .collect();
    topology.twins.sort_by_key(|t| (t.a, t.b));
    assert_eq!(topology.validate(&graph), Ok(()));
    let full = |edge: EdgeId| node_path(&graph, vec![DirEdge::new(edge, false)], 100, 0);
    let ratio = |a: &Path, b: &Path| overlap_ratio(&graph, &topology, a, b);
    // 長い辺 e の全体に対して、短い辺は、それぞれ自分の長さの全部が重なる
    for t in [t1, t2, t3] {
        assert!((ratio(&full(t), &full(e)) - 1.0).abs() < 1e-9, "{t}");
        assert!((ratio(&full(e), &full(t)) - 1.0 / 3.0).abs() < 1e-9, "{t}");
    }
    // 短い辺どうしは双子でない（同じ向きの別の区間）ので、重ならない
    assert_eq!(ratio(&full(t1), &full(t2)), 0.0);
    // 2本の短い辺（t3→t2：e の東側の 2000 dm）は、e の 2/3 を共有する。向きの違う部分を足した合計
    let t3_t2 = node_path(
        &graph,
        vec![DirEdge::new(t3, false), DirEdge::new(t2, false)],
        100,
        0,
    );
    assert!((ratio(&full(e), &t3_t2) - 2.0 / 3.0).abs() < 1e-9);
    assert!((ratio(&t3_t2, &full(e)) - 1.0).abs() < 1e-9);
    // 3本そろえれば e の全体
    let all = node_path(
        &graph,
        vec![
            DirEdge::new(t3, false),
            DirEdge::new(t2, false),
            DirEdge::new(t1, false),
        ],
        100,
        0,
    );
    assert!((ratio(&full(e), &all) - 1.0).abs() < 1e-9);
    // 部分：e の始点から 500〜1500 dm（t1 の東側半分と t2 の西側半分）。t1 の始点側 500 dm（e の 500〜1000 に当たる）と
    // 重なるのは 500 dm で、e のこの部分の半分。t2 は 0〜500 dm 側（e の 1500〜1000 に当たる）の 500 dm
    let part_e = Path {
        dedges: vec![DirEdge::new(e, false)],
        start: snap_at(&graph, e, 500),
        end: snap_at(&graph, e, 1500),
        time_ds: 0,
        risk_fx: 0,
    };
    let part_t1 = Path {
        dedges: vec![DirEdge::new(t1, false)],
        start: snap_at(&graph, t1, 0),
        end: snap_at(&graph, t1, 500),
        time_ds: 0,
        risk_fx: 0,
    };
    assert!(
        (ratio(&part_e, &part_t1) - 0.5).abs() < 1e-6,
        "{}",
        ratio(&part_e, &part_t1)
    );
    assert!((ratio(&part_t1, &part_e) - 1.0).abs() < 1e-6);
    // t1 の西側 500 dm（e の 0〜500 に当たる）は、part_e（500〜1500）と重ならない
    let t1_west = Path {
        dedges: vec![DirEdge::new(t1, false)],
        start: snap_at(&graph, t1, 500),
        end: snap_at(&graph, t1, 1000),
        time_ds: 0,
        risk_fx: 0,
    };
    assert_eq!(ratio(&part_e, &t1_west), 0.0);
    assert_eq!(ratio(&t1_west, &part_e), 0.0);
    // 双子でなければ重ならない
    let plain = empty_topology(&graph);
    assert_eq!(overlap_ratio(&graph, &plain, &full(e), &full(t2)), 0.0);
    // 選別：e と、その双子だけでできた別の候補は、80%を超えて重なるので捨てる（e が先）
    let kept = select_candidates(
        &graph,
        &topology,
        full(e),
        full(e),
        vec![all.clone()],
        1_000,
    );
    assert_eq!(kept.len(), 1);
}

#[test]
fn select_candidates_removes_duplicates_and_keeps_p0_and_best() {
    let w = CandidateWorld::new();
    let (g, topo) = (&w.graph, &w.topology);
    let p0 = w.path(&row(0, 0, 5), 600);
    // best は p0 と同じ：1本にまとめる。
    let kept = select_candidates(g, topo, p0.clone(), p0.clone(), vec![], 1_000);
    assert_eq!(kept, vec![p0.clone()]);
    // best が p0 と90%以上重なっても残す（p0 と best は常に残す）。
    let mut cells = row(0, 0, 5);
    cells.push((1, 5));
    let best = w.path(&cells, 650);
    let kept = select_candidates(g, topo, p0.clone(), best.clone(), vec![], 1_000);
    assert_eq!(kept, vec![p0.clone(), best.clone()]);
    // 同じ辺の列でも、吸着点（出発の位置）が違えば別の候補。
    let mut shifted = p0.clone();
    shifted.start = snap_at(g, shifted.dedges[0].edge(), 300);
    let kept = select_candidates(g, topo, p0.clone(), shifted.clone(), vec![], 1_000);
    assert_eq!(kept.len(), 2);

    // 80%ちょうど（5辺のうち4辺を共有）は残し、それを超える（6辺のうち5辺）は捨てる。
    let mut four_of_five = row(0, 0, 4);
    four_of_five.push((1, 4));
    let exactly_80 = w.path(&four_of_five, 700);
    let mut five_of_six = row(0, 0, 5);
    five_of_six.push((1, 5));
    let over_80 = w.path(&five_of_six, 710);
    let kept = select_candidates(
        g,
        topo,
        p0.clone(),
        p0.clone(),
        vec![over_80.clone(), exactly_80.clone()],
        1_000,
    );
    assert_eq!(kept, vec![p0.clone(), exactly_80.clone()]);
    // 重複の判定は、すでに残した候補すべてと比べる：exactly_80 と80%を超えて重なる候補も捨てる。
    let near_exactly_80 = {
        let mut cells = row(0, 0, 4);
        cells.push((1, 4));
        cells.push((1, 5));
        w.path(&cells, 720)
    };
    let kept = select_candidates(
        g,
        topo,
        p0.clone(),
        p0.clone(),
        vec![exactly_80.clone(), near_exactly_80],
        1_000,
    );
    assert_eq!(
        kept.len(),
        2,
        "6辺のうち5辺（行0の4辺＋縦1辺）を exactly_80 と共有"
    );
}

#[test]
fn select_candidates_limits_to_six_sorts_by_time_and_enforces_the_budget() {
    let w = CandidateWorld::new();
    let (g, topo) = (&w.graph, &w.topology);
    // 互いに重ならない経路を8本：p0 は行0、best は行2、ほかは縦の1辺だけを使う別々の経路にしたいが、
    // 3×7の格子で重ならない経路は多くないので、縦の辺を1本ずつ使う短い経路を作る。
    let vertical = |c: usize, time: u64| w.path(&[(0, c), (1, c)], time);
    let p0 = w.path(&row(0, 0, 1), 500);
    let best = w.path(&row(2, 0, 1), 900);
    let others: Vec<Path> = (2..=7)
        .map(|c| vertical(c.min(6), 1_000 + c as u64))
        .collect();
    // vertical(6) が2本できるので重複して1本は捨てられる（列2〜6の5本 + 重複1）。
    let kept = select_candidates(g, topo, p0.clone(), best.clone(), others.clone(), 5_000);
    assert_eq!(kept.len(), 6, "p0、best、縦の辺4本で上限6本");
    assert!(kept.windows(2).all(|w| w[0].time_ds <= w[1].time_ds));
    assert_eq!(kept[0], p0);
    assert!(kept.contains(&best));
    // 上限6本で打ち切るとき、残るのは前から数えた先頭（p0・best・others の順）。
    let ids: Vec<u64> = kept.iter().map(|p| p.time_ds).collect();
    assert_eq!(ids, vec![500, 900, 1_002, 1_003, 1_004, 1_005]);

    // 同じ所要時間は入力の順を保つ（安定ソート）。
    let a = vertical(2, 700);
    let b = vertical(3, 700);
    let c = vertical(4, 700);
    let kept = select_candidates(
        g,
        topo,
        p0.clone(),
        best.clone(),
        vec![c.clone(), a.clone(), b.clone()],
        5_000,
    );
    assert_eq!(kept[0], p0);
    assert_eq!(&kept[1..4], &[c, a, b]);
    assert_eq!(kept[4], best);

    // 時間が B を超えるものは捨てる（B ちょうどは残す）。p0 は対象外（呼び出し側が検査済み）。
    let at_budget = vertical(2, 5_000);
    let over_budget = vertical(3, 5_001);
    let kept = select_candidates(
        g,
        topo,
        p0.clone(),
        best.clone(),
        vec![over_budget, at_budget.clone()],
        5_000,
    );
    assert_eq!(kept, vec![p0.clone(), best.clone(), at_budget]);
    // best が上限を超えていたら（上流の不具合）、残さない：返す候補は必ず B 以下。
    let slow_best = w.path(&row(2, 0, 1), 6_000);
    let kept = select_candidates(g, topo, p0.clone(), slow_best, vec![], 5_000);
    assert_eq!(kept, vec![p0]);
}

#[test]
fn select_candidates_drops_candidates_longer_than_80km() {
    // 1本の辺が 90 km（900,000 dm）の2点。p0 は短い辺、候補の1つは 90 km。
    let nodes = [
        (35.00, 139.00),
        (35.00, 139.01),
        (35.01, 139.00),
        (35.01, 139.01),
    ];
    let mut b = GraphBuilder::new();
    for &(lat, lon) in &nodes {
        b.add_node_deg(lat, lon);
    }
    let short = b.add_edge(EdgeSpec::new(0, 1).length_dm(1_000));
    let long = b.add_edge(EdgeSpec::new(2, 3).length_dm(900_000));
    let exactly = b.add_edge(EdgeSpec::new(0, 2).length_dm(MAX_ROUTE_DISTANCE_DM as u32));
    let graph = b.build();
    let topology = empty_topology(&graph);
    let one = |e: EdgeId, time: u64| node_path(&graph, vec![DirEdge::new(e, false)], time, 0);
    let p0 = one(short, 100);
    let kept = select_candidates(
        &graph,
        &topology,
        p0.clone(),
        p0.clone(),
        vec![one(long, 200), one(exactly, 300)],
        10_000,
    );
    assert_eq!(
        kept,
        vec![p0.clone(), one(exactly, 300)],
        "80,000mちょうどは残す"
    );
    // p0 は距離の検査の対象外（最短時間経路の80km超は RouteTooLong として別に扱う）。
    let long_p0 = one(long, 100);
    let kept = select_candidates(
        &graph,
        &topology,
        long_p0.clone(),
        long_p0.clone(),
        vec![],
        10_000,
    );
    assert_eq!(kept, vec![long_p0]);
}

// ---------------------------------------------------------------------------------------------
// 楕円の切り出し・縮小
// ---------------------------------------------------------------------------------------------

#[test]
fn ellipse_cut_keeps_the_optimal_solution_on_graphs_with_thousands_of_nodes() {
    // behavior.md 11章「楕円の安全性」：ノード数千のグラフで、楕円で切った部分グラフ上の最適解の
    // リスクと時間が、全体グラフ上の最適解と一致する。最短時間は別に実装した `fastest_path` で求める。
    let mut checked = 0;
    let mut reduced_cases = 0;
    let mut node_ratios = Vec::new();
    let mut seed = 0_u64;
    while checked < 12 {
        seed += 1;
        assert!(seed < 400, "照合できた件数が少ない: {checked}");
        let nodes = 1500 + (seed as usize * 97) % 1000;
        let config = RandomGraphConfig {
            lat_e7: (356_000_000, 356_030_000),
            lon_e7: (1_397_000_000, 1_397_030_000),
            extra_edges: nodes,
            oneway_prob: PerMode {
                walk: 0.02,
                bike: 0.1,
                car: 0.15,
            },
            deny_prob: PerMode {
                walk: 0.02,
                bike: 0.03,
                car: 0.02,
            },
            via_node_restrictions: nodes / 20,
            via_way_restrictions: nodes / 40,
            ..RandomGraphConfig::rich(nodes)
        };
        let mut builder = random_graph(seed, &config);
        builder.set_time_model(costly_time_model());
        let mode = Mode::ALL[(seed % 3) as usize];
        let world = World::new(&builder, mode);
        let g = &world.graph;
        let mut rng = Rng::new(seed ^ 0xe11);
        let (start, end) = (random_snap(&mut rng, g), random_snap(&mut rng, g));
        if same_place(g, &start, &end) {
            continue;
        }
        let ctx = world.ctx(unlimited_params());
        let cancel = CancelToken::unbounded();
        let Some(p0) = fastest_path(&ctx, &start, &end, &cancel).unwrap() else {
            continue;
        };
        assert_eq!(
            path_time_ds(g, mode, &start, &end, &p0.dedges),
            Some(p0.time_ds)
        );
        assert_eq!(check_path(g, &world.restrictions, mode, &p0.dedges), vec![]);
        let mut cost = TestCost::random(&mut rng, g);
        let detour = p0.time_ds * (5 + rng.below(21)) as u64 / 100;
        let budget = p0.time_ds + detour;

        let whole =
            whole_graph_subgraph(&ctx, &start, &end, p0.time_ds, detour, &mut cost, &cancel)
                .unwrap();
        let ellipse = ellipse_subgraph(&ctx, &start, &end, p0.time_ds, detour, &mut cost, &cancel)
            .expect("楕円を切り出せる");
        check_subgraph_invariants(g, mode, &ellipse, &mut cost);
        assert_eq!(ellipse.budget_ds, budget);
        assert_eq!(ellipse.fastest_ds, p0.time_ds);
        // 楕円の辺は、全体グラフの辺の部分集合。
        let whole_dedges: HashSet<DirEdge> = whole
            .arcs
            .iter()
            .filter(|a| a.dedge != NO_DEDGE)
            .map(|a| a.dedge)
            .collect();
        assert!(
            ellipse
                .arcs
                .iter()
                .filter(|a| a.dedge != NO_DEDGE)
                .all(|a| whole_dedges.contains(&a.dedge))
        );
        node_ratios.push(ellipse.nodes.len() as f64 / g.node_count() as f64);

        let solve = |sub: &Subgraph| {
            let lr = larac(&ctx, sub, 10, &cancel).unwrap();
            let rcsp = bounded_rcsp(&ctx, sub, &lr, &cancel).unwrap().unwrap();
            assert_eq!(rcsp.status, SearchStatus::Optimal);
            rcsp.path
        };
        let on_whole = solve(&whole);
        let on_ellipse = solve(&ellipse);
        assert_eq!(
            (on_ellipse.risk_fx, on_ellipse.time_ds),
            (on_whole.risk_fx, on_whole.time_ds),
            "seed={seed}: 楕円上の最適解が全体グラフ上と違う"
        );
        for path in [&on_whole, &on_ellipse] {
            assert!(path.time_ds <= budget);
            assert_eq!(
                check_path(g, &world.restrictions, mode, &path.dedges),
                vec![]
            );
            assert_eq!(
                path_time_ds(g, mode, &start, &end, &path.dedges),
                Some(path.time_ds)
            );
        }
        if on_whole.dedges != p0.dedges {
            reduced_cases += 1;
        }
        // 候補の整理まで通す：返す候補はすべて B 以下。
        let outcome = search_subgraph(&ctx, &ellipse, &p0, &cancel)
            .unwrap()
            .unwrap();
        let picked = select_candidates(
            g,
            &world.topology,
            p0.clone(),
            outcome.best,
            outcome.alternatives,
            budget,
        );
        assert!(picked.iter().all(|p| p.time_ds <= budget));
        checked += 1;
    }
    eprintln!(
        "seeds={seed} 平均ノード比={:.3} 最短時間経路と違う最適解={reduced_cases}",
        node_ratios.iter().sum::<f64>() / node_ratios.len() as f64
    );
    // 楕円が全体の一部に絞れている（切り出しが効いている）。
    assert!(
        node_ratios.iter().sum::<f64>() / (node_ratios.len() as f64) < 0.8,
        "{node_ratios:?}"
    );
    assert!(
        reduced_cases >= 3,
        "最短時間経路と違う最適解が少ない: {reduced_cases}"
    );
}

#[test]
fn ellipse_reports_too_large_and_unreachable() {
    let (mut builder, shape) = grid(30, 30, 100.0);
    builder.set_time_model(costly_time_model());
    let world = World::new(&builder, Mode::Car);
    let g = &world.graph;
    let cancel = CancelToken::unbounded();
    let mut cost = TestCost::with_risk(g, &[]);
    let first = builder.dedge(shape.node(0, 0), shape.node(0, 1));
    let last = builder.dedge(shape.node(29, 28), shape.node(29, 29));
    let start = SnapPoint::on_edge(g, first.edge(), 0);
    let end = SnapPoint::on_edge(g, last.edge(), g.edge(last.edge()).length_dm);
    let ctx = world.ctx(unlimited_params());
    let p0 = fastest_path(&ctx, &start, &end, &cancel).unwrap().unwrap();
    let detour = p0.time_ds / 4;
    let full =
        ellipse_subgraph(&ctx, &start, &end, p0.time_ds, detour, &mut cost, &cancel).unwrap();
    assert!(full.nodes.len() > 300, "{}", full.nodes.len());
    assert!(full.nodes.len() <= g.node_count());
    // ノード数の上限を超えると TooLarge。上限ちょうどなら切り出せる。
    let limited = |limit: u32| {
        world.ctx(SearchParams {
            ellipse_node_limit: limit,
            ..unlimited_params()
        })
    };
    let n = full.nodes.len() as u32;
    match ellipse_subgraph(
        &limited(n - 1),
        &start,
        &end,
        p0.time_ds,
        detour,
        &mut cost,
        &cancel,
    ) {
        Err(EllipseError::TooLarge { nodes }) => assert!(nodes > u64::from(n) - 1),
        other => panic!("{other:?}"),
    }
    let exact = ellipse_subgraph(
        &limited(n),
        &start,
        &end,
        p0.time_ds,
        detour,
        &mut cost,
        &cancel,
    )
    .unwrap();
    assert_eq!(exact.nodes, full.nodes);
    assert_eq!(exact.arcs, full.arcs);
    // 同じ位置の出発・到着、予算が最短時間の下界にも届かない場合は Unreachable。
    assert_eq!(
        ellipse_subgraph(&ctx, &start, &start, p0.time_ds, detour, &mut cost, &cancel).unwrap_err(),
        EllipseError::Unreachable
    );
    assert_eq!(
        ellipse_subgraph(&ctx, &start, &end, 0, 0, &mut cost, &cancel).unwrap_err(),
        EllipseError::Unreachable
    );
    // 中断。
    let cancelled = CancelToken::unbounded();
    cancelled.cancel();
    assert_eq!(
        ellipse_subgraph(
            &ctx, &start, &end, p0.time_ds, detour, &mut cost, &cancelled
        )
        .unwrap_err(),
        EllipseError::Interrupted(Interrupt::Cancelled)
    );
    // 出発と到着が同じ辺の途中：辺の中だけで着けば、ノードを持たない部分グラフになる。
    let a = SnapPoint::on_edge(g, first.edge(), 300);
    let b = SnapPoint::on_edge(g, first.edge(), 800);
    let direct = fastest_path(&ctx, &a, &b, &cancel).unwrap().unwrap();
    let sub = ellipse_subgraph(&ctx, &a, &b, direct.time_ds, 0, &mut cost, &cancel).unwrap();
    assert!(
        sub.arcs
            .iter()
            .any(|arc| arc.head == sub.target && arc.dedge == first)
    );
    let found = larac(&ctx, &sub, 10, &cancel).unwrap().best.unwrap();
    assert_eq!((found.dedges, found.time_ds), (vec![first], direct.time_ds));
}

#[test]
fn reduced_ellipse_keeps_only_nearby_edges_and_major_roads() {
    // 40×40の格子（間隔150 m＝約6 km四方）。行0と列39は primary（幹線）、ほかは residential。
    // 出発は左下、到着は右上。幹線を通る経路（行0→列39）は最短時間経路のひとつ。
    let n = 40;
    let (mut builder, shape) = grid(n, n, 150.0);
    for e in 0..builder.edge_count() as u32 {
        // 辺の端点から行・列を求める。
        let (tail, head) = {
            let spec = builder.edge_spec(e);
            (spec.tail, spec.head)
        };
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
    let cancel = CancelToken::unbounded();
    let mut rng = Rng::new(3);
    let mut cost = TestCost::random(&mut rng, g);
    let first = builder.dedge(shape.node(0, 0), shape.node(0, 1));
    let last = builder.dedge(shape.node(n - 1, n - 2), shape.node(n - 1, n - 1));
    let start = SnapPoint::on_edge(g, first.edge(), 0);
    let end = SnapPoint::on_edge(g, last.edge(), g.edge(last.edge()).length_dm);
    let ctx = world.ctx(unlimited_params());
    let p0 = fastest_path(&ctx, &start, &end, &cancel).unwrap().unwrap();
    let detour = p0.time_ds / 3;
    let full =
        ellipse_subgraph(&ctx, &start, &end, p0.time_ds, detour, &mut cost, &cancel).unwrap();
    let reduced =
        reduced_ellipse(&ctx, &start, &end, p0.time_ds, detour, &mut cost, &cancel).unwrap();
    check_subgraph_invariants(g, Mode::Car, &reduced, &mut cost);
    assert!(
        reduced.nodes.len() < full.nodes.len(),
        "{} < {}",
        reduced.nodes.len(),
        full.nodes.len()
    );
    assert_eq!(
        (reduced.fastest_ds, reduced.budget_ds),
        (full.fastest_ds, full.budget_ds)
    );
    // 辺は、楕円の辺のうち、幹線（class ≤ tertiary_link）か、出発・到着から3 km以内のノードに接するものだけ。
    let centers = [
        g.node_coord(shape.node(0, 0)),
        g.node_coord(shape.node(n - 1, n - 1)),
    ];
    let near = |node: u32| {
        centers.iter().any(|c| {
            straight_distance_m(g.node_coord(node).to_latlon(), c.to_latlon()) <= REDUCED_RADIUS_M
        })
    };
    let full_dedges: HashSet<DirEdge> = full
        .arcs
        .iter()
        .filter(|a| a.dedge != NO_DEDGE)
        .map(|a| a.dedge)
        .collect();
    let mut far_residential = 0;
    for (v, &node) in reduced.nodes.iter().enumerate() {
        for arc in reduced.arcs_of(v as u32) {
            if arc.dedge == NO_DEDGE || arc.head == reduced.target {
                continue;
            }
            assert!(full_dedges.contains(&arc.dedge));
            let class = g.edge(arc.dedge.edge()).class;
            assert!(
                class <= REDUCED_MAX_CLASS || near(node) || near(g.dedge_to(arc.dedge)),
                "遠くの生活道路を残している"
            );
        }
    }
    for &d in &full_dedges {
        let class = g.edge(d.edge()).class;
        if class > REDUCED_MAX_CLASS && !near(g.dedge_from(d)) && !near(g.dedge_to(d)) {
            far_residential += 1;
        }
    }
    assert!(
        far_residential > 100,
        "縮小が効く配置になっていない: {far_residential}"
    );
    // 縮小した部分グラフにも、幹線を通る最短時間経路があり、探索できる。
    let outcome = search_subgraph(&ctx, &reduced, &p0, &cancel)
        .unwrap()
        .unwrap();
    assert!(outcome.best.time_ds <= reduced.budget_ds);
    assert_eq!(
        check_path(g, &world.restrictions, Mode::Car, &outcome.best.dedges),
        vec![]
    );

    // 楕円が TooLarge になる上限でも、縮小なら切り出せる。縮小でも超えるなら TooLarge のまま。
    let limit = (reduced.nodes.len() + full.nodes.len()) as u32 / 2;
    let limited = world.ctx(SearchParams {
        ellipse_node_limit: limit,
        ..unlimited_params()
    });
    assert!(matches!(
        ellipse_subgraph(
            &limited, &start, &end, p0.time_ds, detour, &mut cost, &cancel
        ),
        Err(EllipseError::TooLarge { .. })
    ));
    let again = reduced_ellipse(
        &limited, &start, &end, p0.time_ds, detour, &mut cost, &cancel,
    )
    .unwrap();
    assert_eq!(again.nodes, reduced.nodes);
    let tiny = world.ctx(SearchParams {
        ellipse_node_limit: 10,
        ..unlimited_params()
    });
    assert!(matches!(
        reduced_ellipse(&tiny, &start, &end, p0.time_ds, detour, &mut cost, &cancel),
        Err(EllipseError::TooLarge { .. })
    ));
}

#[test]
fn fastest_path_agrees_with_and_without_alt_tables() {
    // 距離表（ALT）を使った下界でも、最短時間経路の時間と楕円の部分グラフは変わらない。
    let mut with_tables = 0;
    for seed in 0..80_u64 {
        let nodes = 30 + (seed as usize * 13) % 120;
        let config = RandomGraphConfig {
            lat_e7: (356_000_000, 356_015_000),
            lon_e7: (1_397_000_000, 1_397_015_000),
            extra_edges: nodes,
            ..RandomGraphConfig::rich(nodes)
        };
        let mut builder = random_graph(seed, &config);
        builder.set_time_model(costly_time_model());
        let mode = Mode::ALL[(seed % 3) as usize];
        let mut world = World::new(&builder, mode);
        let mut rng = Rng::new(seed ^ 0xa17);
        let g = &world.graph;
        let (start, end) = (random_snap(&mut rng, g), random_snap(&mut rng, g));
        if same_place(g, &start, &end) {
            continue;
        }
        let cancel = CancelToken::unbounded();
        let (plain, e_plain) = {
            let plain_ctx = world.ctx(unlimited_params());
            let plain = fastest_path(&plain_ctx, &start, &end, &cancel).unwrap();
            let e_plain = plain.as_ref().map(|p0| {
                let mut cost = TestCost::random(&mut Rng::new(seed), g);
                ellipse_subgraph(
                    &plain_ctx,
                    &start,
                    &end,
                    p0.time_ds,
                    p0.time_ds / 5 + 1,
                    &mut cost,
                    &cancel,
                )
                .unwrap()
            });
            (plain, e_plain)
        };
        let unit = 1 + rng.below(40) as u32;
        let table = farthest_landmark_table(g, mode, 1 + rng.below(6), 0, unit, rng.chance(0.5));
        let mut tables = [None, None, None];
        tables[accident_route_backend::domain::routing::risk::mode_index(mode)] = Some(table);
        world.landmarks = Landmarks::from_tables(unit, tables);
        let ctx = world.ctx(unlimited_params());
        let alt = fastest_path(&ctx, &start, &end, &cancel).unwrap();
        assert_eq!(
            plain.as_ref().map(|p| p.time_ds),
            alt.as_ref().map(|p| p.time_ds),
            "seed={seed}"
        );
        let (Some(p0), Some(e_plain)) = (plain, e_plain) else {
            continue;
        };
        with_tables += 1;
        // 楕円も同じ。
        let mut cost = TestCost::random(&mut Rng::new(seed), &world.graph);
        let e_alt = ellipse_subgraph(
            &ctx,
            &start,
            &end,
            p0.time_ds,
            p0.time_ds / 5 + 1,
            &mut cost,
            &cancel,
        )
        .unwrap();
        assert_eq!(e_plain.nodes, e_alt.nodes, "seed={seed}");
        assert_eq!(e_plain.arcs, e_alt.arcs, "seed={seed}");
    }
    assert!(with_tables >= 40, "{with_tables}");
}
