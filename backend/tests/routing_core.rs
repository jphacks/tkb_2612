//! 自前経路探索の基盤（M1の共通部分）の結合試験：`Topology::validate`、合成グラフ（`synthetic`）の公開API、
//! `check_path` と `TurnAutomaton` と時間関数のあいだの整合。
//! 各部品の単体試験は、それぞれのモジュールの `#[cfg(test)]` にある。

use accident_route_backend::contract::Mode;
use accident_route_backend::domain::routing::NONE;
use accident_route_backend::domain::routing::graph::restrictions::{Restrictions, TurnAutomaton};
use accident_route_backend::domain::routing::graph::synthetic::{
    ALL_MODES_MASK, EdgeSpec, GraphBuilder, RandomGraphConfig, Rng, grid, random_graph,
};
use accident_route_backend::domain::routing::graph::time::{
    TimeModel, edge_time_ds, node_penalty_ds, partial_edge_time_ds, turn_cost_ds,
};
use accident_route_backend::domain::routing::graph::{DirEdge, Graph, edge_flags, node_flags};
use accident_route_backend::domain::routing::legality::{Violation, check_path};
use accident_route_backend::domain::routing::topology::{
    ClusterRecord, SidewalkParent, Topology, TwinPair, cluster_flags,
};

// ---------------------------------------------------------------------------------------------
// Topology::validate
// ---------------------------------------------------------------------------------------------

/// 3×3の格子（N=9、E=12）に合う、正しい `topology.bin` の中身。
fn valid_topology() -> (Graph, Topology) {
    let graph = grid(3, 3, 100.0).0.build();
    assert_eq!((graph.node_count(), graph.edge_count()), (9, 12));
    let topology = Topology {
        params: serde_json::Value::Null,
        snap_car: vec![0xfff],
        snap_bike: vec![0b1010],
        snap_walk: vec![0],
        node_cluster: vec![NONE, NONE, NONE, NONE, 0, 0, NONE, 1, 1],
        clusters: vec![
            ClusterRecord {
                lat_e7: 0,
                lon_e7: 0,
                node_start: 0,
                node_count: 2,
                top_class: 4,
                branches: 3,
                flags: cluster_flags::SIGNAL,
                pad: [0; 3],
            },
            ClusterRecord {
                lat_e7: 0,
                lon_e7: 0,
                node_start: 2,
                node_count: 2,
                top_class: 6,
                branches: 2,
                flags: cluster_flags::SIGNAL | cluster_flags::ROUNDABOUT,
                pad: [0; 3],
            },
        ],
        cluster_nodes: vec![4, 5, 7, 8],
        twins: vec![
            TwinPair { a: 2, b: 8 },
            TwinPair { a: 3, b: 5 },
            TwinPair { a: 3, b: 6 },
            TwinPair { a: 5, b: 3 },
            TwinPair { a: 6, b: 3 },
            TwinPair { a: 8, b: 2 },
        ],
        sidewalk_parents: vec![
            SidewalkParent {
                sidewalk: 1,
                parent: 0,
            },
            SidewalkParent {
                sidewalk: 4,
                parent: 0,
            },
            SidewalkParent {
                sidewalk: 9,
                parent: 7,
            },
        ],
    };
    (graph, topology)
}

#[test]
fn topology_validate_accepts_valid_files() {
    let (graph, topology) = valid_topology();
    assert_eq!(topology.validate(&graph), Ok(()));

    // クラスタ・双子・歩道がない、最小の形
    let minimal = Topology {
        params: serde_json::Value::Null,
        snap_car: vec![0],
        snap_bike: vec![0],
        snap_walk: vec![0],
        node_cluster: vec![NONE; 9],
        clusters: vec![],
        cluster_nodes: vec![],
        twins: vec![],
        sidewalk_parents: vec![],
    };
    assert_eq!(minimal.validate(&graph), Ok(()));

    // 空のグラフ
    let empty = GraphBuilder::new().build();
    let none = Topology {
        params: serde_json::Value::Null,
        snap_car: vec![],
        snap_bike: vec![],
        snap_walk: vec![],
        node_cluster: vec![],
        clusters: vec![],
        cluster_nodes: vec![],
        twins: vec![],
        sidewalk_parents: vec![],
    };
    assert_eq!(none.validate(&empty), Ok(()));
}

#[test]
fn topology_validate_handles_word_boundaries() {
    // Eがちょうど64、65、128のグラフ：ビット列の長さと余りのビット。
    for edges in [64_usize, 65, 128] {
        let mut b = GraphBuilder::new();
        let a = b.add_node_deg(35.0, 139.0);
        let c = b.add_node_deg(35.0, 139.001);
        for _ in 0..edges {
            b.add_road(a, c);
        }
        let graph = b.build();
        let words = edges.div_ceil(64);
        let mut t = Topology {
            params: serde_json::Value::Null,
            snap_car: vec![u64::MAX; words],
            snap_bike: vec![0; words],
            snap_walk: vec![0; words],
            node_cluster: vec![NONE; 2],
            clusters: vec![],
            cluster_nodes: vec![],
            twins: vec![],
            sidewalk_parents: vec![],
        };
        // 余りのビットが立っている（64の倍数のときは余りがない）。
        let has_padding = edges % 64 != 0;
        assert_eq!(t.validate(&graph).is_err(), has_padding, "E={edges}");
        let tail = edges % 64;
        if has_padding {
            t.snap_car[words - 1] = (1_u64 << tail) - 1;
        }
        assert_eq!(t.validate(&graph), Ok(()), "E={edges}");
        t.snap_bike.push(0);
        assert!(t.validate(&graph).is_err());
    }
}

#[test]
fn topology_validate_rejects_each_violation() {
    type Mutation = fn(&mut Topology);
    let cases: Vec<(&str, Mutation, &str)> = vec![
        ("SNAPCARが長い", |t| t.snap_car.push(0), "SNAPCAR"),
        ("SNAPBIKが短い", |t| t.snap_bike.clear(), "SNAPBIK"),
        ("SNAPFOTが長い", |t| t.snap_walk.extend([0, 0]), "SNAPFOT"),
        (
            "SNAPCARの余りのビット",
            |t| t.snap_car[0] |= 1 << 12,
            "SNAPCAR",
        ),
        (
            "SNAPFOTの余りのビット",
            |t| t.snap_walk[0] |= 1 << 63,
            "SNAPFOT",
        ),
        ("NODECLSの件数", |t| t.node_cluster.push(NONE), "NODECLS"),
        ("NODECLSがC以上", |t| t.node_cluster[0] = 2, "NODECLS[0]"),
        (
            "NODECLSがCを超えるが、NONEではない",
            |t| t.node_cluster[8] = NONE - 1,
            "NODECLS[8]",
        ),
        (
            "クラスタの範囲がCLSNODEを超える",
            |t| t.clusters[1].node_start = 3,
            "CLSNODE",
        ),
        (
            "クラスタのノードが昇順でない",
            |t| t.cluster_nodes.swap(0, 1),
            "昇順",
        ),
        ("クラスタのノードが重複", |t| t.cluster_nodes[1] = 4, "昇順"),
        ("クラスタのノードがN以上", |t| t.cluster_nodes[3] = 9, "N=9"),
        (
            "CLSNODEのノードのNODECLSが違う",
            |t| t.node_cluster[4] = 1,
            "NODECLS",
        ),
        (
            "NODECLSにあるがCLSNODEにない",
            |t| t.node_cluster[0] = 0,
            "CLSNODE",
        ),
        (
            "CLSNODEに余りがある",
            |t| t.cluster_nodes.push(0),
            "CLSNODE",
        ),
        (
            "クラスタが同じノードを重ねて持つ",
            |t| {
                t.clusters[1].node_start = 0;
            },
            "NODECLS",
        ),
        ("padが0でない", |t| t.clusters[0].pad = [0, 1, 0], "pad"),
        (
            "flagsに未定義のビット",
            |t| t.clusters[0].flags = 0x04,
            "flags",
        ),
        (
            "TWINの辺番号がE以上",
            |t| t.twins[3] = TwinPair { a: 8, b: 12 },
            "TWIN[3]",
        ),
        (
            "TWINが同じ辺の組",
            |t| t.twins[0] = TwinPair { a: 2, b: 2 },
            "TWIN[0]",
        ),
        ("TWINが(a,b)の昇順でない", |t| t.twins.swap(0, 1), "TWIN"),
        (
            "TWINの同じ組が重複",
            |t| t.twins[2] = TwinPair { a: 3, b: 5 },
            "TWIN",
        ),
        ("TWINの同じaでbが昇順でない", |t| t.twins.swap(1, 2), "TWIN"),
        (
            "TWINのaが複数のbを持ち、片方の逆向きがない",
            |t| {
                t.twins.remove(4);
            },
            "逆向き",
        ),
        (
            "TWINの逆向きがない",
            |t| {
                t.twins.remove(3);
            },
            "逆向き",
        ),
        (
            "SWPARENTの辺番号がE以上",
            |t| t.sidewalk_parents[1].parent = 12,
            "SWPARENT[1]",
        ),
        (
            "SWPARENTが同じ辺",
            |t| t.sidewalk_parents[0].parent = 1,
            "SWPARENT[0]",
        ),
        (
            "SWPARENTがsidewalkの昇順でない",
            |t| t.sidewalk_parents.swap(0, 2),
            "SWPARENT",
        ),
        (
            "SWPARENTのsidewalkが重複",
            |t| t.sidewalk_parents[1].sidewalk = 1,
            "SWPARENT",
        ),
    ];
    for (label, mutate, needle) in cases {
        let (graph, mut topology) = valid_topology();
        mutate(&mut topology);
        let error = topology
            .validate(&graph)
            .expect_err(&format!("{label}は拒否されるはず"));
        let message = error.to_string();
        assert!(
            message.starts_with("topology.binが不正です") && message.contains(needle),
            "{label}: {message}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 合成グラフ
// ---------------------------------------------------------------------------------------------

#[test]
fn synthetic_rng_and_random_graphs_are_pinned_to_their_seeds() {
    // 乱数列とグラフの生成は、シードを使うテスト（600通りの全列挙との比較など）の再現性の前提。
    // 生成の規則を変えるときは、このテストの値も意図して更新する。
    let mut rng = Rng::new(0);
    assert_eq!(
        [rng.next_u64(), rng.next_u64(), rng.next_u64()],
        PINNED_RNG_SEED_0
    );

    let g = random_graph(7, &RandomGraphConfig::new(4)).build();
    let coords: Vec<(i32, i32)> = (0..4)
        .map(|v| {
            let c = g.node_coord(v);
            (c.lat_e7, c.lon_e7)
        })
        .collect();
    assert_eq!(coords, PINNED_COORDS_SEED_7);
    assert_eq!(g.edge_count(), PINNED_EDGE_COUNT_SEED_7);
}

/// 独立に実装したxorshift64*（splitmix64でシードを拡散）で確かめた値。
const PINNED_RNG_SEED_0: [u64; 3] = [
    8_916_199_331_640_804_048,
    16_032_783_972_208_265_725,
    12_954_103_179_475_586_193,
];
const PINNED_COORDS_SEED_7: [(i32, i32); 4] = [
    (356_023_563, 1_397_065_182),
    (356_014_069, 1_397_007_694),
    (356_018_178, 1_397_026_037),
    (356_001_891, 1_397_077_594),
];
const PINNED_EDGE_COUNT_SEED_7: usize = 5;

#[test]
fn synthetic_builder_covers_the_documented_usage() {
    let mut b = GraphBuilder::new();
    let a = b.add_node_deg(35.0, 139.0);
    let c = b.add_node_deg(35.0, 139.001);
    let d = b.add_node_deg(35.001, 139.001);
    let e = b.add_node_deg(35.001, 139.0);
    let ac = b.add_edge(EdgeSpec::new(a, c).length_dm(1000));
    b.add_edge(EdgeSpec::new(c, d).length_dm(1000).oneway());
    b.add_edge(EdgeSpec::new(d, e).length_dm(1000).maxspeed(60));
    b.add_edge(
        EdgeSpec::new(e, a)
            .length_dm(1000)
            .flags(edge_flags::TUNNEL),
    );
    b.add_node_flags(c, node_flags::SIGNAL);
    b.forbid_nodes(&[a, c, d], Mode::Car.mask());
    b.only_nodes(&[c, d], e, ALL_MODES_MASK);
    let (graph, restrictions) = b.build_all();
    assert_eq!(restrictions.validate(&graph), Ok(()));
    assert_eq!(graph.edge(ac).length_dm, 1000);
    // 同じ builder から何度でも同じものが作れる。
    let (graph2, restrictions2) = b.build_all();
    assert_eq!(graph.parts(), graph2.parts());
    assert_eq!(restrictions, restrictions2);
    // 節に戻して from_parts で作り直しても同じ。
    let rebuilt = Graph::from_parts(graph.parts().clone()).unwrap();
    assert_eq!(rebuilt.parts(), graph.parts());
    // forbid（car）と only（全手段、c→d の後は d→e だけ。d→c への引き返しは禁止）
    let by_mode = |mode: Mode| TurnAutomaton::build(&restrictions, mode);
    let acd = b.dedges(&[a, c, d]);
    let cd_dc = b.dedges(&[c, d, c]);
    for mode in Mode::ALL {
        let automaton = by_mode(mode);
        let mut state = automaton.start();
        let mut blocked_at = None;
        for (i, &dedge) in acd.iter().enumerate() {
            match automaton.step(state, dedge) {
                Some(next) => state = next,
                None => {
                    blocked_at = Some(i);
                    break;
                }
            }
        }
        assert_eq!(blocked_at, (mode == Mode::Car).then_some(1), "{mode:?}");
        // c→d→c は only 由来で全手段が禁止。
        let automaton = by_mode(mode);
        let mut state = automaton.start();
        let mut blocked = false;
        for &dedge in &cd_dc {
            match automaton.step(state, dedge) {
                Some(next) => state = next,
                None => blocked = true,
            }
        }
        assert!(blocked, "{mode:?}");
    }
}

#[test]
fn a_few_thousand_node_graph_builds_quickly_and_validly() {
    let config = RandomGraphConfig {
        extra_edges: 3000,
        shape_points_max: 2,
        via_node_restrictions: 200,
        via_way_restrictions: 100,
        ..RandomGraphConfig::rich(4000)
    };
    let (graph, restrictions) = random_graph(99, &config).build_all();
    assert_eq!(graph.node_count(), 4000);
    assert!(graph.edge_count() > 6000);
    assert_eq!(restrictions.validate(&graph), Ok(()));
    assert!(restrictions.items.len() > 200);
}

// ---------------------------------------------------------------------------------------------
// 部品間の整合
// ---------------------------------------------------------------------------------------------

/// 手段でその有向辺を通れるか、その手段で通過できないノードを途中で通っていないか、
/// 禁止の列を含まないか、を `check_path` とは別に調べる（照合器と各アクセサだけを使う）。
fn oracle_is_legal(graph: &Graph, automaton: &TurnAutomaton, mode: Mode, path: &[DirEdge]) -> bool {
    if path.iter().any(|&d| !graph.passable(d, mode)) {
        return false;
    }
    if path.windows(2).any(|w| {
        let node = graph.dedge_to(w[0]);
        node != graph.dedge_from(w[1]) || graph.node_blocked(node, mode)
    }) {
        return false;
    }
    let mut state = automaton.start();
    for &d in path {
        match automaton.step(state, d) {
            Some(next) => state = next,
            None => return false,
        }
    }
    true
}

/// ランダムなつながった有向辺の列（通れない辺も混ぜる）。
fn random_walk(rng: &mut Rng, graph: &Graph, mode: Mode, max_len: usize) -> Vec<DirEdge> {
    let mut node = rng.below(graph.node_count()) as u32;
    let mut walk = Vec::new();
    for _ in 0..1 + rng.below(max_len) {
        let outs = graph.out_dedges(node);
        let passable: Vec<DirEdge> = outs
            .iter()
            .copied()
            .filter(|&d| graph.passable(d, mode))
            .collect();
        let pool: &[DirEdge] = if !passable.is_empty() && rng.chance(0.85) {
            &passable
        } else {
            outs
        };
        if pool.is_empty() {
            break;
        }
        let d = *rng.pick(pool);
        walk.push(d);
        node = graph.dedge_to(d);
    }
    walk
}

#[test]
fn check_path_agrees_with_an_independent_oracle_on_random_graphs() {
    let mut rng = Rng::new(31337);
    let (mut legal, mut illegal) = (0, 0);
    // 通行（NotPassable・WrongDirection）、右左折禁止、通過できないノードの違反が、それぞれ出ている。
    let mut seen_kinds = [false; 3];
    for seed in 0..300_u64 {
        let nodes = 3 + (seed % 10) as usize;
        let config = RandomGraphConfig {
            via_node_restrictions: nodes,
            via_way_restrictions: nodes / 2,
            block_prob: accident_route_backend::domain::routing::graph::time::PerMode {
                walk: 0.15,
                bike: 0.15,
                car: 0.15,
            },
            ..RandomGraphConfig::rich(nodes)
        };
        let (graph, restrictions) = random_graph(seed, &config).build_all();
        for mode in Mode::ALL {
            let automaton = TurnAutomaton::build(&restrictions, mode);
            for _ in 0..25 {
                let walk = random_walk(&mut rng, &graph, mode, 8);
                let violations = check_path(&graph, &restrictions, mode, &walk);
                let is_legal = oracle_is_legal(&graph, &automaton, mode, &walk);
                assert_eq!(
                    violations.is_empty(),
                    is_legal,
                    "seed={seed} {mode:?} {walk:?} {violations:?}"
                );
                // 禁止の列：照合器が最初に止まる位置が、最初のForbiddenTurnの位置と一致する。
                let mut state = automaton.start();
                let mut first_block = None;
                for (i, &d) in walk.iter().enumerate() {
                    match automaton.step(state, d) {
                        Some(next) => state = next,
                        None => {
                            first_block = Some(i);
                            break;
                        }
                    }
                }
                let first_forbidden = violations.iter().find_map(|v| match v {
                    Violation::ForbiddenTurn { index } => Some(*index),
                    _ => None,
                });
                assert_eq!(
                    first_block, first_forbidden,
                    "seed={seed} {mode:?} {walk:?}"
                );
                // 経路はつながっているので Disconnected は出ない。
                assert!(
                    !violations
                        .iter()
                        .any(|v| matches!(v, Violation::Disconnected { .. }))
                );
                for v in &violations {
                    match v {
                        Violation::NotPassable { .. } | Violation::WrongDirection { .. } => {
                            seen_kinds[0] = true
                        }
                        Violation::ForbiddenTurn { .. } => seen_kinds[1] = true,
                        Violation::BarrierNode { .. } => seen_kinds[2] = true,
                        Violation::Disconnected { .. } => unreachable!("経路はつながっている"),
                    }
                }
                if is_legal {
                    legal += 1;
                } else {
                    illegal += 1;
                }
            }
        }
    }
    assert!(seen_kinds.iter().all(|&s| s), "{seen_kinds:?}");
    assert!(
        legal > 500 && illegal > 500,
        "legal={legal} illegal={illegal}"
    );
}

#[test]
fn time_functions_are_consistent_on_random_graphs() {
    let model = TimeModel::synthetic();
    for seed in 0..60_u64 {
        let config = RandomGraphConfig {
            shape_points_max: 3,
            ..RandomGraphConfig::rich(12)
        };
        let graph = random_graph(seed, &config).build();
        for e in 0..graph.edge_count() as u32 {
            for reverse in [false, true] {
                let d = DirEdge::new(e, reverse);
                for mode in Mode::ALL {
                    let time = edge_time_ds(&graph, d, mode);
                    // 合成モデルは全種類に正の速度があるので、通れる有向辺はちょうど時間を持つ。
                    assert_eq!(time.is_some(), graph.passable(d, mode));
                    let Some(time) = time else { continue };
                    let speed = *model.speed_kmh.get(mode).first().unwrap();
                    let exact = f64::from(graph.edge(e).length_dm) * 3.6 / speed;
                    assert!(
                        (f64::from(time) - exact).abs() < 1.0 && f64::from(time) >= exact - 1e-6,
                        "{time} {exact}"
                    );
                    let length = graph.edge(e).length_dm;
                    assert_eq!(partial_edge_time_ds(&graph, d, mode, length), Some(time));
                    assert_eq!(partial_edge_time_ds(&graph, d, mode, 0), Some(0));
                    let half = partial_edge_time_ds(&graph, d, mode, length / 2).unwrap();
                    assert!(half <= time);
                }
            }
        }
        // 合成モデルでは、ノードの加算も右左折のコストも0。
        for v in 0..graph.node_count() as u32 {
            for mode in Mode::ALL {
                assert_eq!(node_penalty_ds(&graph, v, mode), 0);
                for &out in graph.out_dedges(v) {
                    assert_eq!(turn_cost_ds(&graph, out.reversed(), out, mode), 0);
                }
            }
        }
    }
}

#[test]
fn turn_costs_follow_the_bearing_difference_on_eight_spokes() {
    // 実際の係数に近いモデル：右折 > 左折。Uターンが最も重い。
    let mut b = GraphBuilder::new();
    let center = b.add_node_deg(35.0, 139.0);
    let mut model = TimeModel::synthetic();
    for mode in Mode::ALL {
        let p = match mode {
            Mode::Walk => &mut model.turn.walk,
            Mode::Bike => &mut model.turn.bike,
            Mode::Car => &mut model.turn.car,
        };
        p.right_s = 6.0;
        p.left_s = 2.0;
        p.u_turn_s = 12.0;
    }
    b.set_time_model(model);
    // 中心の周りに8方向のノードを置く。
    let cos_lat = 35.0_f64.to_radians().cos();
    let spokes: Vec<u32> = (0..8)
        .map(|k| {
            let a = f64::from(k) * 45.0_f64.to_radians();
            let n = b.add_node_deg(35.0 + 0.001 * a.cos(), 139.0 + 0.001 * a.sin() / cos_lat);
            b.add_road(center, n);
            n
        })
        .collect();
    let graph = b.build();
    for (i, &from) in spokes.iter().enumerate() {
        let inbound = dedge_between(&graph, from, center);
        for (j, &to) in spokes.iter().enumerate() {
            let outbound = dedge_between(&graph, center, to);
            let cost = turn_cost_ds(&graph, inbound, outbound, Mode::Car);
            // 入ってくる方角は from の反対向き。from と to が同じ放射線なら引き返し。
            let expected = if i == j {
                120
            } else {
                // 入る向きは from → center。from が北(0°)なら南向き(180°)に進んでいる。
                // 出る向きは center → to の方位 45°×j。変化角（時計回り）= 方位(out) - 方位(in)
                let bearing_in = (45.0 * i as f64 + 180.0) % 360.0;
                let bearing_out = 45.0 * j as f64;
                let mut turn = (bearing_out - bearing_in) % 360.0;
                if turn > 180.0 {
                    turn -= 360.0;
                } else if turn <= -180.0 {
                    turn += 360.0;
                }
                if turn.abs() <= 30.0 {
                    0
                } else if turn.abs() >= 150.0 {
                    120
                } else if turn > 0.0 {
                    60
                } else {
                    20
                }
            };
            assert_eq!(cost, expected, "{i}->{j}");
        }
    }
}

fn dedge_between(graph: &Graph, from: u32, to: u32) -> DirEdge {
    *graph
        .out_dedges(from)
        .iter()
        .find(|&&d| graph.dedge_to(d) == to)
        .expect("辺がある")
}

#[test]
fn empty_restrictions_never_block_anything() {
    let (graph, restrictions) = grid(3, 3, 50.0).0.build_all();
    assert_eq!(restrictions, Restrictions::empty());
    for mode in Mode::ALL {
        let a = TurnAutomaton::build(&restrictions, mode);
        assert_eq!(a.state_count(), 1);
    }
    let mut rng = Rng::new(3);
    for _ in 0..200 {
        let walk = random_walk(&mut rng, &graph, Mode::Walk, 12);
        assert_eq!(check_path(&graph, &restrictions, Mode::Walk, &walk), vec![]);
    }
}
