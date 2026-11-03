//! 部分グラフ上の探索の共通部分：状態空間と、1つの目的関数を最小にする探索（behavior.md 5章 手順5・6）。
//!
//! 探索の状態は「局所ノード、直前の有向辺、照合器の状態」。右左折コストと右左折禁止は直前の有向辺に
//! 依存するので、この3つを状態から落とさない。状態は探索ごとに、見つけたものから番号を振る
//! （作業領域は部分グラフの大きさに比例し、全体グラフの大きさには依存しない）。
//!
//! 辺をつなぐときに足す量（モジュール `ellipse` のドキュメントと同じ規則）：
//!
//! - 直前の有向辺を持つノードから出る：そのノードが通過不可なら出られない。出られるなら
//!   `node_penalty_ds + turn_cost_ds(直前, 次)` を時間に足す。
//! - 照合器が禁止の列の末尾に達する遷移は通れない。
//! - 出発の仮想ノードから出る辺と、`NO_DEDGE` の辺（ノードで出発・到着するだけ）には、これらを足さない。
//!
//! 時間とリスクは足し算で、どちらも0以上、辺の時間は1以上なので、同じ状態を2回通る歩道は最適解に
//! ならない。ダイクストラ法は状態の数で必ず終わる。

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::hash::{BuildHasherDefault, Hasher};

use crate::contract::{CancelToken, Interrupt};
use crate::domain::routing::graph::DirEdge;
use crate::domain::routing::graph::time::{node_penalty_ds, turn_cost_ds};

use super::Path;
use super::SearchContext;
use super::ellipse::{NO_DEDGE, SubArc, Subgraph};

/// 出発の状態（番号0）。
pub(super) const SOURCE_STATE: u32 = 0;
/// 到着の状態（番号1）。到着の仮想ノードに着いたことを表し、辺は出ない。
pub(super) const TERMINAL_STATE: u32 = 1;
/// 直前の有向辺がない（出発ノードにいる）ことを表す印。
pub(super) const NO_LAST: u32 = u32::MAX;
/// `cancel.check()` を呼ぶ間隔（ラベルや状態の取り出し回数）。2の冪。
pub(super) const CANCEL_CHECK_INTERVAL: u32 = 1024;
/// 距離が決まっていないことを表す値。
pub(super) const INF: u64 = u64::MAX;

const NOT_BUILT: u32 = u32::MAX;

/// 状態の表の簡易なハッシュ（Fx系）。整数の組だけを引くので、衝突耐性は要らない。
#[derive(Default)]
pub(super) struct FxHasher(u64);

const FX_SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(5) ^ u64::from(b)).wrapping_mul(FX_SEED);
        }
    }

    fn write_u32(&mut self, i: u32) {
        self.0 = (self.0.rotate_left(5) ^ u64::from(i)).wrapping_mul(FX_SEED);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

type StateKey = (u32, u32, u32);
type StateIds = HashMap<StateKey, u32, BuildHasherDefault<FxHasher>>;

/// 状態から出る遷移。
#[derive(Debug, Clone, Copy)]
pub(super) struct Trans {
    /// `Subgraph::arcs` の添字。
    pub arc: u32,
    /// 辺の時間に加える量（デシ秒）：ノードの加算＋右左折コスト。
    pub extra_ds: u32,
    /// 遷移先の状態。
    pub to: u32,
}

/// 状態空間。状態は「局所ノード・直前の有向辺・照合器の状態」で、遷移は最初に必要になったときに作る。
pub(super) struct StateSpace<'a> {
    ctx: SearchContext<'a>,
    sub: &'a Subgraph,
    ids: StateIds,
    node: Vec<u32>,
    last: Vec<u32>,
    aut: Vec<u32>,
    /// 状態ごとの遷移の範囲 `(開始位置, 長さ)`。開始位置が `NOT_BUILT` なら未作成。
    trans_range: Vec<(u32, u32)>,
    trans: Vec<Trans>,
}

impl<'a> StateSpace<'a> {
    pub(super) fn new(ctx: &SearchContext<'a>, sub: &'a Subgraph) -> Self {
        let mut space = StateSpace {
            ctx: *ctx,
            sub,
            ids: StateIds::default(),
            node: Vec::new(),
            last: Vec::new(),
            aut: Vec::new(),
            trans_range: Vec::new(),
            trans: Vec::new(),
        };
        let source = space.intern(sub.source, NO_LAST, ctx.automaton.start());
        let terminal = space.intern(sub.target, NO_LAST, 0);
        debug_assert_eq!((source, terminal), (SOURCE_STATE, TERMINAL_STATE));
        space
    }

    pub(super) fn state_count(&self) -> usize {
        self.node.len()
    }

    /// 状態の局所ノード。
    pub(super) fn node_of(&self, state: u32) -> u32 {
        self.node[state as usize]
    }

    pub(super) fn trans_at(&self, index: usize) -> Trans {
        self.trans[index]
    }

    fn intern(&mut self, node: u32, last: u32, aut: u32) -> u32 {
        if let Some(&id) = self.ids.get(&(node, last, aut)) {
            return id;
        }
        let id = self.node.len() as u32;
        self.ids.insert((node, last, aut), id);
        self.node.push(node);
        self.last.push(last);
        self.aut.push(aut);
        self.trans_range.push((NOT_BUILT, 0));
        id
    }

    /// 状態 `s` から出る遷移の範囲（`trans_at` の添字）。最初に呼ばれたときに作る。
    /// 遷移の先の状態は、ここで番号を振る（`state_count` が増える）。
    pub(super) fn transitions(&mut self, s: u32) -> std::ops::Range<usize> {
        let (start, len) = self.trans_range[s as usize];
        if start != NOT_BUILT {
            return start as usize..(start + len) as usize;
        }
        let sub = self.sub;
        let graph = self.ctx.graph;
        let mode = self.ctx.mode;
        let automaton = self.ctx.automaton;
        let node = self.node[s as usize];
        let last = self.last[s as usize];
        let aut = self.aut[s as usize];
        let begin = self.trans.len();
        let is_source = node == sub.source;
        let global = if is_source {
            None
        } else {
            Some(sub.nodes[node as usize])
        };
        // 直前の有向辺を持つノードが通過不可なら、そこから先の辺には出られない（そこで終わるのは構わない）。
        let blocked = global.is_some_and(|g| last != NO_LAST && graph.node_blocked(g, mode));
        for ai in sub.node_ofs[node as usize]..sub.node_ofs[node as usize + 1] {
            let arc = &sub.arcs[ai as usize];
            if arc.dedge == NO_DEDGE {
                if is_source {
                    // 出発ノードにつなぐ（直前の有向辺なし）。
                    let to = self.intern(arc.head, NO_LAST, automaton.start());
                    self.trans.push(Trans {
                        arc: ai,
                        extra_ds: 0,
                        to,
                    });
                } else if last != NO_LAST {
                    // 到着ノードで終わる。
                    self.trans.push(Trans {
                        arc: ai,
                        extra_ds: 0,
                        to: TERMINAL_STATE,
                    });
                }
                continue;
            }
            if blocked {
                continue;
            }
            let Some(next_aut) = automaton.step(aut, arc.dedge) else {
                continue;
            };
            let extra_ds = match global {
                Some(g) if last != NO_LAST => {
                    node_penalty_ds(graph, g, mode)
                        + turn_cost_ds(graph, DirEdge(last), arc.dedge, mode)
                }
                _ => 0,
            };
            let to = if arc.head == sub.target {
                TERMINAL_STATE
            } else {
                self.intern(arc.head, arc.dedge.0, next_aut)
            };
            self.trans.push(Trans {
                arc: ai,
                extra_ds,
                to,
            });
        }
        let range = begin..self.trans.len();
        self.trans_range[s as usize] = (begin as u32, (range.end - range.start) as u32);
        range
    }
}

/// 探索の目的関数。値は小さいほどよい。どれも辺ごとの足し算で、2つの値の辞書順で比べる。
#[derive(Clone, Copy)]
pub(super) enum Objective<'a> {
    /// (リスク, 時間)：リスク最小、同じなら時間最小。
    RiskThenTime,
    /// (時間, リスク)：時間最小、同じならリスク最小。
    TimeThenRisk,
    /// (`alpha`×時間 + `beta`×リスク, 時間)。`risk` があれば、辺ごとのリスクの代わりに使う
    /// （ペナルティ法で、一部の辺のリスクを増やす）。
    Weighted {
        alpha: u64,
        beta: u64,
        risk: Option<&'a [u64]>,
    },
}

impl Objective<'_> {
    /// 遷移1本の増分 `(第1キー, 第2キー)`。`time` は辺の時間にノードの加算と右左折コストを足した値。
    fn increment(&self, time: u64, arc_index: u32, arc: &SubArc) -> (u128, u64) {
        match *self {
            Objective::RiskThenTime => (u128::from(arc.risk_fx), time),
            Objective::TimeThenRisk => (u128::from(time), arc.risk_fx),
            Objective::Weighted { alpha, beta, risk } => {
                let r = risk.map_or(arc.risk_fx, |v| v[arc_index as usize]);
                (
                    u128::from(alpha) * u128::from(time) + u128::from(beta) * u128::from(r),
                    time,
                )
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Entry {
    key: (u128, u64),
    time: u64,
    risk: u64,
    parent: u32,
    arc: u32,
}

const UNSEEN: Entry = Entry {
    key: (u128::MAX, u64::MAX),
    time: 0,
    risk: 0,
    parent: NO_LAST,
    arc: NO_LAST,
};

/// 出発から到着までの、`objective` が最小の経路（ダイクストラ法）。到着できなければ `None`。
/// 経路の時間にはノードの加算と右左折コストを含み、リスクは `SubArc::risk_fx` の和
/// （`Objective::Weighted` の `risk` で替えても、返す経路のリスクは本来の値）。
pub(super) fn shortest_path(
    ctx: &SearchContext<'_>,
    sub: &Subgraph,
    objective: Objective<'_>,
    cancel: &CancelToken,
) -> Result<Option<Path>, Interrupt> {
    cancel.check()?;
    let mut space = StateSpace::new(ctx, sub);
    let mut entries: Vec<Entry> = vec![UNSEEN; space.state_count()];
    entries[SOURCE_STATE as usize] = Entry {
        key: (0, 0),
        time: 0,
        risk: 0,
        parent: NO_LAST,
        arc: NO_LAST,
    };
    let mut heap: BinaryHeap<Reverse<(u128, u64, u32)>> = BinaryHeap::new();
    heap.push(Reverse((0, 0, SOURCE_STATE)));
    let mut pops: u32 = 0;
    while let Some(Reverse((k1, k2, s))) = heap.pop() {
        pops = pops.wrapping_add(1);
        if pops & (CANCEL_CHECK_INTERVAL - 1) == 0 {
            cancel.check()?;
        }
        let here = entries[s as usize];
        if here.key != (k1, k2) {
            continue;
        }
        if s == TERMINAL_STATE {
            return Ok(Some(build_path(sub, &entries, s)));
        }
        let range = space.transitions(s);
        entries.resize(space.state_count(), UNSEEN);
        for index in range {
            let tr = space.trans_at(index);
            let arc = &sub.arcs[tr.arc as usize];
            let time = u64::from(tr.extra_ds) + u64::from(arc.time_ds);
            let (d1, d2) = objective.increment(time, tr.arc, arc);
            let key = (here.key.0 + d1, here.key.1 + d2);
            let slot = &mut entries[tr.to as usize];
            if key < slot.key {
                *slot = Entry {
                    key,
                    time: here.time + time,
                    risk: here.risk + arc.risk_fx,
                    parent: s,
                    arc: tr.arc,
                };
                heap.push(Reverse((key.0, key.1, tr.to)));
            }
        }
    }
    Ok(None)
}

fn build_path(sub: &Subgraph, entries: &[Entry], terminal: u32) -> Path {
    let mut arcs = Vec::new();
    let mut s = terminal;
    while entries[s as usize].parent != NO_LAST {
        arcs.push(entries[s as usize].arc);
        s = entries[s as usize].parent;
    }
    arcs.reverse();
    let dedges = path_dedges(sub, arcs.into_iter());
    Path {
        dedges,
        start: sub.start,
        end: sub.end,
        time_ds: entries[terminal as usize].time,
        risk_fx: entries[terminal as usize].risk,
    }
}

/// 辺の添字の並びから、`NO_DEDGE` を除いた有向辺の並びを作る。
pub(super) fn path_dedges(sub: &Subgraph, arcs: impl Iterator<Item = u32>) -> Vec<DirEdge> {
    arcs.map(|a| sub.arcs[a as usize].dedge)
        .filter(|&d| d != NO_DEDGE)
        .collect()
}

/// 到着の仮想ノードから逆向きにたどった、各局所ノードから到着までの辺の重みの和の最小
/// （ノードの加算・右左折コスト・禁止・通過不可を無視した下界。到達できなければ `INF`）。
/// `weight` に辺の時間を渡せば時間の下界、`risk_fx` を渡せばリスクの下界になる。
pub(super) fn costs_to_target(
    sub: &Subgraph,
    weight: impl Fn(&SubArc) -> u64,
    cancel: &CancelToken,
) -> Result<Vec<u64>, Interrupt> {
    let n = sub.node_count();
    // 逆向きの隣接（到着ノードごとに、入ってくる辺の (出発ノード, 辺の添字)）。
    let mut count = vec![0_u32; n + 1];
    for arc in &sub.arcs {
        count[arc.head as usize + 1] += 1;
    }
    for i in 0..n {
        count[i + 1] += count[i];
    }
    let mut fill = count.clone();
    let mut incoming = vec![(0_u32, 0_u32); sub.arcs.len()];
    for v in 0..n as u32 {
        for ai in sub.node_ofs[v as usize]..sub.node_ofs[v as usize + 1] {
            let head = sub.arcs[ai as usize].head as usize;
            incoming[fill[head] as usize] = (v, ai);
            fill[head] += 1;
        }
    }
    let mut dist = vec![INF; n];
    let mut heap: BinaryHeap<Reverse<(u64, u32)>> = BinaryHeap::new();
    dist[sub.target as usize] = 0;
    heap.push(Reverse((0, sub.target)));
    let mut pops: u32 = 0;
    while let Some(Reverse((d, v))) = heap.pop() {
        pops = pops.wrapping_add(1);
        if pops & (CANCEL_CHECK_INTERVAL - 1) == 0 {
            cancel.check()?;
        }
        if d > dist[v as usize] {
            continue;
        }
        for &(u, ai) in &incoming[count[v as usize] as usize..count[v as usize + 1] as usize] {
            let nd = d.saturating_add(weight(&sub.arcs[ai as usize]));
            if nd < dist[u as usize] {
                dist[u as usize] = nd;
                heap.push(Reverse((nd, u)));
            }
        }
    }
    Ok(dist)
}
