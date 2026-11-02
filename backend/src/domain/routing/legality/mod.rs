//! 経路の合法性の検査（形式書18章、作業書の合法性試験）。
//! 探索の結果が、その手段で通れる有向辺の連なりで、右左折禁止と通過できないノードに違反しないことを確かめる。

use std::collections::{BTreeSet, HashSet};

use crate::contract::Mode;
use crate::domain::routing::graph::restrictions::Restrictions;
use crate::domain::routing::graph::{DirEdge, Graph, access};

/// 違反の種類。`index` は `dedges` の添字。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Violation {
    /// その手段で通れない辺（`access` が0、またはその手段の通行不可）。
    NotPassable { index: usize },
    /// 一方通行の逆走など、その向きには通れない。
    WrongDirection { index: usize },
    /// 右左折禁止の列を含む。`index` は列の末尾の有向辺。
    ForbiddenTurn { index: usize },
    /// 隣り合う有向辺がつながっていない（`index` は後ろの有向辺）。
    Disconnected { index: usize },
    /// その手段が通過できないノードを途中で通っている（`index` はそのノードに入る有向辺）。
    BarrierNode { index: usize },
}

/// 有向辺の並びを検査し、違反をすべて返す（なければ空）。
///
/// 返す順序は `index` の昇順で、同じ `index` では `NotPassable`／`WrongDirection`、`Disconnected`、
/// `BarrierNode`、`ForbiddenTurn` の順。検査は探索の照合器（`TurnAutomaton`）に依存せず、
/// 禁止の列を素朴な部分列照合で調べる。
///
/// - 通行：辺番号が範囲外、または辺がその手段をどちらの向きにも許さないなら `NotPassable`。
///   逆向きだけ許す（一方通行の逆走）なら `WrongDirection`。
/// - つながり：`dedges[i-1]` の到着ノードと `dedges[i]` の出発ノードが違えば、`Disconnected { index: i }`。
/// - 通過できないノード：つながっている途中のノード（`dedges[i]` と `dedges[i+1]` の間）が、その手段で
///   通過不可なら `BarrierNode { index: i }`。経路の最初の出発ノードと最後の到着ノードは対象外。
/// - 右左折禁止：その手段の禁止の列が連続した部分列として現れたら、列の末尾の位置で
///   `ForbiddenTurn { index }`（同じ末尾に複数の列が合っても1件）。
pub fn check_path(
    graph: &Graph,
    restrictions: &Restrictions,
    mode: Mode,
    dedges: &[DirEdge],
) -> Vec<Violation> {
    let edge_count = graph.edge_count();
    let known = |d: DirEdge| (d.edge() as usize) < edge_count;
    let forbidden_ends = forbidden_turn_ends(restrictions, mode, dedges);
    let mut violations = Vec::new();
    for (i, &d) in dedges.iter().enumerate() {
        if !known(d) {
            violations.push(Violation::NotPassable { index: i });
            continue;
        }
        if !graph.passable(d, mode) {
            let record = graph.edge(d.edge());
            if record.access & access::both(mode) == 0 {
                violations.push(Violation::NotPassable { index: i });
            } else {
                violations.push(Violation::WrongDirection { index: i });
            }
        }
        if i > 0 && known(dedges[i - 1]) && graph.dedge_to(dedges[i - 1]) != graph.dedge_from(d) {
            violations.push(Violation::Disconnected { index: i });
        }
        if let Some(&next) = dedges.get(i + 1)
            && known(next)
        {
            let node = graph.dedge_to(d);
            if node == graph.dedge_from(next) && graph.node_blocked(node, mode) {
                violations.push(Violation::BarrierNode { index: i });
            }
        }
        if forbidden_ends.contains(&i) {
            violations.push(Violation::ForbiddenTurn { index: i });
        }
    }
    violations
}

/// その手段の禁止の列が連続した部分列として現れる、列の末尾の位置。
/// 経路に現れる有向辺で始まる列だけを取り出してから、位置ごとに素朴に照合する。
fn forbidden_turn_ends(
    restrictions: &Restrictions,
    mode: Mode,
    dedges: &[DirEdge],
) -> BTreeSet<usize> {
    let mask = mode.mask();
    let on_path: HashSet<DirEdge> = dedges.iter().copied().collect();
    let candidates: Vec<&[DirEdge]> = restrictions
        .items
        .iter()
        .filter(|r| r.modes & mask != 0)
        .filter_map(|r| {
            r.seq
                .first()
                .filter(|d| on_path.contains(d))
                .map(|_| r.seq.as_slice())
        })
        .collect();
    let mut ends = BTreeSet::new();
    for start in 0..dedges.len() {
        for seq in &candidates {
            if dedges[start..].starts_with(seq) {
                ends.insert(start + seq.len() - 1);
            }
        }
    }
    ends
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::routing::graph::synthetic::{
        ALL_MODES_MASK, EdgeSpec, GraphBuilder, GridShape, grid,
    };
    use crate::domain::routing::graph::{node_flags, restrictions::Restriction};

    /// 3×3の格子（ノード番号は `3×行 + 列`）に条件を足したもの。
    /// - (0,0)→(0,1) は car・bike が一方通行（逆走できない）
    /// - (0,1)-(0,2) は car が通れない
    /// - (1,1) は car が通過できない
    struct World {
        builder: GraphBuilder,
        shape: GridShape,
    }

    impl World {
        fn new() -> World {
            let (mut builder, shape) = grid(3, 3, 100.0);
            let n = |r, c| shape.node(r, c);
            let one_way = builder.dedge(n(0, 0), n(0, 1)).edge();
            let spec = builder.edge_spec(one_way).clone();
            *builder.edge_spec_mut(one_way) = spec.oneway();
            let no_car = builder.dedge(n(0, 1), n(0, 2)).edge();
            let spec = builder.edge_spec(no_car).clone();
            *builder.edge_spec_mut(no_car) = spec.deny(Mode::Car);
            builder.add_node_flags(n(1, 1), node_flags::block_bit(Mode::Car));
            World { builder, shape }
        }

        fn path(&self, nodes: &[(usize, usize)]) -> Vec<DirEdge> {
            let ids: Vec<u32> = nodes.iter().map(|&(r, c)| self.shape.node(r, c)).collect();
            self.builder.dedges(&ids)
        }

        fn check(&self, mode: Mode, dedges: &[DirEdge]) -> Vec<Violation> {
            let (graph, restrictions) = self.builder.build_all();
            check_path(&graph, &restrictions, mode, dedges)
        }
    }

    #[test]
    fn a_legal_path_has_no_violations() {
        let w = World::new();
        // car：(0,0)→(0,1)→(1,1)... は (1,1) が通過不可なので別の道。(1,0)→(2,0)→(2,1)→(2,2)
        let p = w.path(&[(1, 0), (2, 0), (2, 1), (2, 2)]);
        for mode in Mode::ALL {
            assert_eq!(w.check(mode, &p), vec![], "{mode:?}");
        }
        // 経路が空、有向辺が1本だけ。
        assert_eq!(w.check(Mode::Car, &[]), vec![]);
        assert_eq!(w.check(Mode::Car, &p[..1]), vec![]);
        // 端点が通過不可のノードでもよい（出発・到着の辺の端点）。
        let into_blocked = w.path(&[(1, 0), (1, 1)]);
        assert_eq!(w.check(Mode::Car, &into_blocked), vec![]);
        let out_of_blocked = w.path(&[(1, 1), (1, 2)]);
        assert_eq!(w.check(Mode::Car, &out_of_blocked), vec![]);
        // 一方通行を順方向に進む。
        assert_eq!(w.check(Mode::Car, &w.path(&[(0, 0), (0, 1)])), vec![]);
    }

    #[test]
    fn not_passable_and_wrong_direction_are_distinguished() {
        let w = World::new();
        // car が通れない辺（どちらの向きも）
        let forward = w.path(&[(0, 1), (0, 2)]);
        let backward = w.path(&[(0, 2), (0, 1)]);
        assert_eq!(
            w.check(Mode::Car, &forward),
            vec![Violation::NotPassable { index: 0 }]
        );
        assert_eq!(
            w.check(Mode::Car, &backward),
            vec![Violation::NotPassable { index: 0 }]
        );
        // bike・walk は通れる。
        assert_eq!(w.check(Mode::Bike, &forward), vec![]);
        assert_eq!(w.check(Mode::Walk, &backward), vec![]);
        // 一方通行の逆走：car・bike は WrongDirection、walk は通れる。
        let against = w.path(&[(0, 1), (0, 0)]);
        assert_eq!(
            w.check(Mode::Car, &against),
            vec![Violation::WrongDirection { index: 0 }]
        );
        assert_eq!(
            w.check(Mode::Bike, &against),
            vec![Violation::WrongDirection { index: 0 }]
        );
        assert_eq!(w.check(Mode::Walk, &against), vec![]);
        // 添字は経路の位置
        let long = w.path(&[(2, 2), (2, 1), (1, 1), (0, 1), (0, 0)]);
        let violations = w.check(Mode::Bike, &long);
        assert_eq!(violations, vec![Violation::WrongDirection { index: 3 }]);
    }

    #[test]
    fn disconnected_dedges_are_reported_at_the_later_one() {
        let w = World::new();
        let mut p = w.path(&[(1, 0), (2, 0)]);
        p.extend(w.path(&[(2, 1), (2, 2)]));
        assert_eq!(
            w.check(Mode::Walk, &p),
            vec![Violation::Disconnected { index: 1 }]
        );
        // 同じ辺の逆向きを続けた引き返しはつながっている。
        let back = w.path(&[(1, 0), (2, 0)]);
        let uturn = vec![back[0], back[0].reversed()];
        assert_eq!(w.check(Mode::Walk, &uturn), vec![]);
        // 向きが合わない（終点を出発点にしてしまう）
        let flipped = vec![back[0], back[0]];
        assert_eq!(
            w.check(Mode::Walk, &flipped),
            vec![Violation::Disconnected { index: 1 }]
        );
    }

    #[test]
    fn blocked_nodes_are_forbidden_only_in_the_middle_of_a_path() {
        let w = World::new();
        // (1,0)→(1,1)→(1,2)：(1,1) は car が通過できない。
        let through = w.path(&[(1, 0), (1, 1), (1, 2)]);
        assert_eq!(
            w.check(Mode::Car, &through),
            vec![Violation::BarrierNode { index: 0 }]
        );
        assert_eq!(w.check(Mode::Bike, &through), vec![]);
        assert_eq!(w.check(Mode::Walk, &through), vec![]);
        // 3本続けて2回通過
        let mut b = World::new();
        b.builder
            .add_node_flags(b.shape.node(1, 2), node_flags::block_bit(Mode::Car));
        let two = b.path(&[(1, 0), (1, 1), (1, 2), (2, 2)]);
        assert_eq!(
            b.check(Mode::Car, &two),
            vec![
                Violation::BarrierNode { index: 0 },
                Violation::BarrierNode { index: 1 }
            ]
        );
        // つながっていない箇所は障害物の判定をしない（Disconnected だけ）。
        let mut gap = w.path(&[(1, 0), (1, 1)]);
        gap.extend(w.path(&[(2, 1), (2, 2)]));
        assert_eq!(
            w.check(Mode::Car, &gap),
            vec![Violation::Disconnected { index: 1 }]
        );
    }

    #[test]
    fn forbidden_turns_are_found_at_the_last_dedge_of_the_sequence() {
        let mut w = World::new();
        let n = |r, c| w.shape.node(r, c);
        let a = [n(2, 0), n(2, 1), n(2, 2)]; // via-node
        let b = [n(1, 2), n(2, 2), n(2, 1), n(2, 0)]; // via-way（3本）
        w.builder.forbid_nodes(&a, Mode::Car.mask());
        w.builder.forbid_nodes(&b, ALL_MODES_MASK);
        let via_node = w.path(&[(2, 0), (2, 1), (2, 2)]);
        let ok_node = w.path(&[(2, 0), (2, 1), (1, 1)]);
        assert_eq!(
            w.check(Mode::Car, &via_node),
            vec![Violation::ForbiddenTurn { index: 1 }]
        );
        // 手段が違えば対象外
        assert_eq!(w.check(Mode::Bike, &via_node), vec![]);
        assert_eq!(w.check(Mode::Walk, &via_node), vec![]);
        // 別の曲がり方は対象外
        assert_eq!(w.check(Mode::Walk, &ok_node), vec![]);
        // via-way：全手段が対象。列の途中までなら通れる。
        let via_way = w.path(&[(1, 2), (2, 2), (2, 1), (2, 0)]);
        for mode in Mode::ALL {
            let v = w.check(mode, &via_way);
            assert!(
                v.contains(&Violation::ForbiddenTurn { index: 2 }),
                "{mode:?} {v:?}"
            );
        }
        let partial = w.path(&[(1, 2), (2, 2), (2, 1)]);
        assert_eq!(w.check(Mode::Walk, &partial), vec![]);
        // 長い経路の途中に現れる。末尾の添字を返す。
        let long = w.path(&[(0, 2), (1, 2), (2, 2), (2, 1), (2, 0), (1, 0)]);
        assert_eq!(
            w.check(Mode::Walk, &long),
            vec![Violation::ForbiddenTurn { index: 3 }]
        );
        // 途中に別の有向辺が挟まると連続していないので違反ではない。
        let detour = w.path(&[(1, 2), (2, 2), (1, 2), (2, 2), (2, 1), (2, 0)]);
        assert_eq!(
            w.check(Mode::Walk, &detour),
            vec![Violation::ForbiddenTurn { index: 4 }]
        );
    }

    #[test]
    fn overlapping_and_repeated_matches_are_each_reported_once() {
        let (mut b, s) = grid(2, 3, 100.0);
        let n = |r, c| s.node(r, c);
        // (0,0)→(0,1)→(0,0) と (0,1)→(0,0)→(0,1) の往復。
        b.forbid_nodes(&[n(0, 0), n(0, 1), n(0, 0)], Mode::Walk.mask());
        b.forbid_nodes(&[n(0, 1), n(0, 0), n(0, 1)], Mode::Walk.mask());
        // 同じ末尾で終わる別の列（長さ3）。
        b.forbid_nodes(&[n(1, 0), n(0, 0), n(0, 1), n(0, 0)], Mode::Walk.mask());
        let (g, r) = b.build_all();
        let p = b.dedges(&[n(1, 0), n(0, 0), n(0, 1), n(0, 0), n(0, 1), n(0, 0)]);
        assert_eq!(
            check_path(&g, &r, Mode::Walk, &p),
            vec![
                Violation::ForbiddenTurn { index: 2 }, // (1,0)(0,0)(0,1)(0,0)：2本の列が同じ末尾
                Violation::ForbiddenTurn { index: 3 },
                Violation::ForbiddenTurn { index: 4 },
            ]
        );
    }

    #[test]
    fn all_violations_are_returned_in_index_order() {
        let mut w = World::new();
        let n = |r, c| w.shape.node(r, c);
        w.builder
            .forbid_nodes(&[n(2, 0), n(2, 1), n(2, 2)], Mode::Car.mask());
        // 0: 一方通行の逆走 (0,1)→(0,0)、1: つながらない (2,0)→(2,1)、2: (2,1)→(2,2) で禁止
        let mut p = w.path(&[(0, 1), (0, 0)]);
        p.extend(w.path(&[(2, 0), (2, 1), (2, 2)]));
        assert_eq!(
            w.check(Mode::Car, &p),
            vec![
                Violation::WrongDirection { index: 0 },
                Violation::Disconnected { index: 1 },
                Violation::ForbiddenTurn { index: 2 },
            ]
        );
        // 同じ添字の中は 通行→つながり→障害物→禁止 の順。(1,1)→... を使う。
        let mut w = World::new();
        let n = |r, c| w.shape.node(r, c);
        w.builder
            .forbid_nodes(&[n(1, 0), n(1, 1), n(1, 2)], Mode::Car.mask());
        w.builder
            .forbid_nodes(&[n(1, 1), n(1, 2), n(2, 2)], Mode::Car.mask());
        let q = w.path(&[(1, 0), (1, 1), (1, 2), (2, 2)]);
        assert_eq!(
            w.check(Mode::Car, &q),
            vec![
                Violation::BarrierNode { index: 0 },
                Violation::ForbiddenTurn { index: 1 },
                Violation::ForbiddenTurn { index: 2 },
            ]
        );
    }

    #[test]
    fn unknown_dedges_are_not_passable_and_do_not_panic() {
        let w = World::new();
        let (graph, restrictions) = w.builder.build_all();
        let outside = DirEdge::new(graph.edge_count() as u32, false);
        let ok = w.path(&[(2, 0), (2, 1)])[0];
        assert_eq!(
            check_path(&graph, &restrictions, Mode::Walk, &[ok, outside]),
            vec![Violation::NotPassable { index: 1 }]
        );
        assert_eq!(
            check_path(&graph, &restrictions, Mode::Walk, &[outside, ok]),
            vec![Violation::NotPassable { index: 0 }]
        );
    }

    #[test]
    fn a_loop_edge_and_parallel_edges_work() {
        let mut b = GraphBuilder::new();
        let a = b.add_node_deg(35.0, 139.0);
        let c = b.add_node_deg(35.0, 139.001);
        let e0 = b.add_road(a, c);
        let e1 = b.add_edge(EdgeSpec::new(a, c).oneway());
        let lp = b.add_road(c, c);
        let (g, r) = b.build_all();
        use crate::domain::routing::graph::synthetic::{bwd, fwd};
        // 並行辺を使った往復 a→c（e0）, c→a（e1 の逆走）
        assert_eq!(
            check_path(&g, &r, Mode::Car, &[fwd(e0), bwd(e1)]),
            vec![Violation::WrongDirection { index: 1 }]
        );
        assert_eq!(check_path(&g, &r, Mode::Car, &[fwd(e1), bwd(e0)]), vec![]);
        // ループ辺を通って戻る。ループの両向きはどちらもcから出てcに入る。
        assert_eq!(
            check_path(&g, &r, Mode::Car, &[fwd(e0), fwd(lp), bwd(lp), bwd(e0)]),
            vec![]
        );
        assert_eq!(
            check_path(&g, &r, Mode::Car, &[fwd(e0), bwd(lp), fwd(lp)]),
            vec![]
        );
    }

    #[test]
    fn restrictions_given_in_any_order_are_matched() {
        // check_path は並び順に依存しない（検査器は探索と独立）。
        let w = World::new();
        let (graph, _) = w.builder.build_all();
        let n = |r, c| w.shape.node(r, c);
        let early = Restriction {
            seq: w.builder.dedges(&[n(2, 1), n(2, 2), n(1, 2)]),
            modes: 4,
            kind: 0,
        };
        let late = Restriction {
            seq: w.builder.dedges(&[n(2, 0), n(2, 1), n(2, 2)]),
            modes: 4,
            kind: 0,
        };
        let r = Restrictions {
            items: vec![early, late],
            ..Restrictions::empty()
        };
        let p = w.path(&[(2, 0), (2, 1), (2, 2), (1, 2)]);
        assert_eq!(
            check_path(&graph, &r, Mode::Car, &p),
            vec![
                Violation::ForbiddenTurn { index: 1 },
                Violation::ForbiddenTurn { index: 2 }
            ]
        );
    }
}
