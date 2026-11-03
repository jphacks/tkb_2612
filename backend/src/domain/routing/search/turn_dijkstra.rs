//! 「直前の有向辺＋右左折禁止の照合器の状態」を状態に持つ最短路探索（behavior.md 5章 手順2.2）。
//!
//! 右左折禁止（via-way型を含む）、通過できないノード、ノードの加算、角度による右左折コストを守る。
//! 出発・到着の吸着点は [`Endpoints`] で与え、最初と最後の有向辺は部分有向辺として照合器に入れる
//! （`legality::check_path` と同じ並びで禁止を判定する）。
//!
//! コストの型は [`Pricing`] で差し替えられる。1回の遷移（最初の有向辺、途中の有向辺、最後の有向辺）ごとに
//! [`Transition`] を渡し、コストを返してもらう。時間だけなら [`TimeOnly`]。辞書順の `(リスク, 時間)` や
//! `時間 + λ × リスク` は、`Add + Ord` を満たす型を作って実装する。探索は、状態の持つコストが最小のものから
//! 確定するDijkstra（`potential` を渡すとA*）。コストは0以上で、`Add` について単調であること。
//!
//! 作業領域は [`TurnWork`]（ハッシュ表）で、触れた状態の数に比例する。全国グラフ（N個）の配列は
//! 確保しない。`clear` で再利用できる。

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::hash::{BuildHasherDefault, Hasher};
use std::ops::Add;

use crate::contract::{CancelToken, Interrupt, Mode};
use crate::domain::routing::graph::restrictions::TurnAutomaton;
use crate::domain::routing::graph::time::{edge_time_ds, node_penalty_ds, turn_cost_ds};
use crate::domain::routing::graph::{DirEdge, Graph, NodeId};

use super::endpoints::Endpoints;

/// 番号をまとめて混ぜるだけの軽いハッシュ（ノード番号・状態の番号は攻撃者が選べない内部の値）。
#[derive(Default, Clone, Copy)]
pub struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn write_u32(&mut self, x: u32) {
        self.write_u64(u64::from(x));
    }

    fn write_u64(&mut self, x: u64) {
        let mut h = x ^ (x >> 32);
        h = h.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        self.0 = h ^ (h >> 29);
    }
}

/// [`IdHasher`] を使う `HashMap`。
pub type IdMap<K, V> = HashMap<K, V, BuildHasherDefault<IdHasher>>;

/// 遷移の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Leg {
    /// 出発と到着が同じ辺の途中で、1本の有向辺で直接行く（`Endpoints::direct`）。
    Direct,
    /// 最初の有向辺（出発の部分有向辺、または出発のノードから出る有向辺）。
    Departure,
    /// 途中の有向辺（直前の有向辺のあと、ノードを通って入る）。
    Interior,
    /// 最後の有向辺（到着の部分有向辺）。
    Arrival,
}

/// 1回の遷移：直前の有向辺 `prev` で `node` に着き、有向辺 `out` を通る。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    pub leg: Leg,
    /// 直前の有向辺。`Direct`・`Departure`、および出発がノード上で最初の有向辺のすぐ後に到着の部分有向辺が
    /// 続く場合の `Arrival` では `None`。
    pub prev: Option<DirEdge>,
    /// `prev` と `out` の間のノード（`prev` が `None` なら `None`）。
    pub node: Option<NodeId>,
    /// 通る有向辺。`Departure`・`Arrival`・`Direct` では部分有向辺のこともある。
    pub out: DirEdge,
    /// この遷移の時間（デシ秒）：途中の遷移はノードの加算＋右左折コスト＋辺の時間、
    /// 最初・最後・直接の遷移は（通る部分の）辺の時間。最後の遷移には直前のノードの加算と右左折コストも含む。
    pub time_ds: u64,
}

/// 遷移のコストの付け方。
pub trait Pricing {
    /// 経路のコスト。0以上で、足し算について単調（`a + b ≥ a`）。
    type Cost: Copy + Ord + Add<Output = Self::Cost>;
    /// 経路の始まりのコスト（空の経路）。
    const ZERO: Self::Cost;
    /// 遷移のコスト。
    fn price(&mut self, t: &Transition) -> Self::Cost;
}

/// 時間（デシ秒）だけをコストにする。
#[derive(Debug, Clone, Copy, Default)]
pub struct TimeOnly;

impl Pricing for TimeOnly {
    type Cost = u64;
    const ZERO: u64 = 0;

    fn price(&mut self, t: &Transition) -> u64 {
        t.time_ds
    }
}

#[derive(Clone, Copy)]
struct Label<C> {
    cost: C,
    /// 直前の状態（出発の状態なら `NO_PARENT`）。
    parent: u64,
}

const NO_PARENT: u64 = u64::MAX;

/// 探索の作業領域。ラベルの表と優先度つきキュー。触れた状態の数に比例する。
#[derive(Debug)]
pub struct TurnWork<C> {
    labels: IdMap<u64, Label<C>>,
    /// （キー = コスト + 下界, コスト, 状態）。小さい順。
    heap: BinaryHeap<Reverse<(C, C, u64)>>,
}

impl<C: std::fmt::Debug> std::fmt::Debug for Label<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Label({:?}, parent {})", self.cost, self.parent)
    }
}

impl<C: Ord + Copy> Default for TurnWork<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: Ord + Copy> TurnWork<C> {
    pub fn new() -> Self {
        Self {
            labels: IdMap::default(),
            heap: BinaryHeap::new(),
        }
    }

    /// 中身を捨てる（確保した容量は残す）。
    pub fn clear(&mut self) {
        self.labels.clear();
        self.heap.clear();
    }

    /// 直近の探索で触れた状態の数。
    pub fn touched_states(&self) -> usize {
        self.labels.len()
    }

    /// 確保した容量を手放す。大きな探索のあとに呼び、常駐のメモリを増やさない。
    pub fn release(&mut self) {
        self.labels = IdMap::default();
        self.heap = BinaryHeap::new();
    }
}

/// 探索の入力。
pub struct TurnSearch<'a, P: Pricing> {
    pub graph: &'a Graph,
    /// この手段の禁止の照合器。
    pub automaton: &'a TurnAutomaton,
    pub mode: Mode,
    pub endpoints: &'a Endpoints,
    pub pricing: P,
    /// 通ってよいノード。`None` なら全ノード。途中のノードと、最初の有向辺の到着ノードに使う
    /// （出発のノードには使わない）。
    pub filter: Option<&'a dyn Fn(NodeId) -> bool>,
    /// ノードから到着までのコストの下界（A*）。許容的で、有向辺ごとに無矛盾であること
    /// （遷移のコストは辺の時間以上）。`None` なら0。
    pub potential: Option<&'a dyn Fn(NodeId) -> P::Cost>,
    /// コストの上限。これを超える経路は見つけない（なければ `None` を返す）。
    pub bound: Option<P::Cost>,
}

/// 見つけた経路。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnPath<C> {
    pub cost: C,
    /// 最初の部分有向辺から最後の部分有向辺までの有向辺の並び（`Path::dedges` と同じ並び）。
    pub dedges: Vec<DirEdge>,
    /// 確定した状態の数。
    pub settled: usize,
}

fn pack(d: DirEdge, automaton_state: u32) -> u64 {
    (u64::from(d.0) << 32) | u64::from(automaton_state)
}

fn unpack(state: u64) -> (DirEdge, u32) {
    (DirEdge((state >> 32) as u32), state as u32)
}

/// 終わり方：状態 `state`（なければ空の出発）に、最後の有向辺 `last`（なければ状態のノードで終わり）を足す。
#[derive(Clone, Copy)]
struct Finish {
    state: Option<u64>,
    last: Option<DirEdge>,
}

fn offer<C: Ord + Copy>(best: &mut Option<(C, Finish)>, bound: Option<C>, cost: C, finish: Finish) {
    if bound.is_some_and(|b| cost > b) {
        return;
    }
    if best.as_ref().is_none_or(|(c, _)| cost < *c) {
        *best = Some((cost, finish));
    }
}

/// ラベルを更新できれば更新してキューに入れる。
fn improve<C: Ord + Copy>(
    work: &mut TurnWork<C>,
    state: u64,
    cost: C,
    parent: u64,
    key: C,
) -> bool {
    if work.labels.get(&state).is_some_and(|l| l.cost <= cost) {
        return false;
    }
    work.labels.insert(state, Label { cost, parent });
    work.heap.push(Reverse((key, cost, state)));
    true
}

/// 「直前の有向辺＋照合器の状態」を状態に持つ最短路探索。
///
/// 出発の部分有向辺（[`Endpoints::departures`]）から始め、最後の部分有向辺（[`Endpoints::arrivals`]）か、
/// 到着がノード上ならそのノードに着いた時点で終わる。同じ辺の途中なら直接行く経路
/// （[`Endpoints::direct`]）も候補にする。経路がなければ（`bound` を超える場合を含む）`None`。
/// 取り消しは約1024回の拡張ごとに確かめる。
pub fn turn_dijkstra<P: Pricing>(
    search: &mut TurnSearch<'_, P>,
    work: &mut TurnWork<P::Cost>,
    cancel: &CancelToken,
) -> Result<Option<TurnPath<P::Cost>>, Interrupt> {
    cancel.check()?;
    work.clear();
    let graph = search.graph;
    let mode = search.mode;
    let automaton = search.automaton;
    let endpoints = search.endpoints;
    let bound = search.bound;
    let filter = search.filter;
    let potential = search.potential;
    let h = |v: NodeId| potential.map_or(P::ZERO, |f| f(v));
    let admit = |v: NodeId| filter.is_none_or(|f| f(v));
    let a0 = automaton.start();

    let mut best: Option<(P::Cost, Finish)> = None;

    if let Some(arc) = endpoints.direct {
        let cost = P::ZERO
            + search.pricing.price(&Transition {
                leg: Leg::Direct,
                prev: None,
                node: None,
                out: arc.dedge,
                time_ds: u64::from(arc.time_ds),
            });
        offer(
            &mut best,
            bound,
            cost,
            Finish {
                state: None,
                last: Some(arc.dedge),
            },
        );
    }
    for arc in &endpoints.departures {
        let Some(a1) = automaton.step(a0, arc.dedge) else {
            continue;
        };
        if !admit(arc.to) {
            continue;
        }
        let cost = P::ZERO
            + search.pricing.price(&Transition {
                leg: Leg::Departure,
                prev: None,
                node: None,
                out: arc.dedge,
                time_ds: u64::from(arc.time_ds),
            });
        let key = cost + h(arc.to);
        if bound.is_some_and(|b| key > b) {
            continue;
        }
        improve(work, pack(arc.dedge, a1), cost, NO_PARENT, key);
    }
    // 出発がノード上で、そのノードから到着の部分有向辺に直接入る。
    if let Some(n) = endpoints.start_node() {
        for arc in endpoints.arrivals.iter().filter(|a| a.from == n) {
            if automaton.step(a0, arc.dedge).is_none() {
                continue;
            }
            let cost = P::ZERO
                + search.pricing.price(&Transition {
                    leg: Leg::Arrival,
                    prev: None,
                    node: None,
                    out: arc.dedge,
                    time_ds: u64::from(arc.time_ds),
                });
            offer(
                &mut best,
                bound,
                cost,
                Finish {
                    state: None,
                    last: Some(arc.dedge),
                },
            );
        }
    }

    let mut settled = 0_usize;
    while let Some(Reverse((key, cost, state))) = work.heap.pop() {
        if best.as_ref().is_some_and(|(c, _)| key >= *c) || bound.is_some_and(|b| key > b) {
            break;
        }
        if work.labels.get(&state).is_none_or(|l| l.cost != cost) {
            continue;
        }
        settled += 1;
        if settled & 1023 == 0 {
            cancel.check()?;
        }
        let (d, a) = unpack(state);
        let v = graph.dedge_to(d);

        if endpoints.end_node() == Some(v) {
            offer(
                &mut best,
                bound,
                cost,
                Finish {
                    state: Some(state),
                    last: None,
                },
            );
        }
        // 通過できないノードは、途中のノードにはなれない（到着のノードにはなれる：上で処理済み）。
        if graph.node_blocked(v, mode) {
            continue;
        }
        let penalty = u64::from(node_penalty_ds(graph, v, mode));

        for arc in endpoints.arrivals.iter().filter(|x| x.from == v) {
            if automaton.step(a, arc.dedge).is_none() {
                continue;
            }
            let time_ds = penalty
                + u64::from(turn_cost_ds(graph, d, arc.dedge, mode))
                + u64::from(arc.time_ds);
            let total = cost
                + search.pricing.price(&Transition {
                    leg: Leg::Arrival,
                    prev: Some(d),
                    node: Some(v),
                    out: arc.dedge,
                    time_ds,
                });
            offer(
                &mut best,
                bound,
                total,
                Finish {
                    state: Some(state),
                    last: Some(arc.dedge),
                },
            );
        }

        for &o in graph.out_dedges(v) {
            let Some(edge_ds) = edge_time_ds(graph, o, mode) else {
                continue;
            };
            let Some(a2) = automaton.step(a, o) else {
                continue;
            };
            let u = graph.dedge_to(o);
            if !admit(u) {
                continue;
            }
            let time_ds = penalty + u64::from(turn_cost_ds(graph, d, o, mode)) + u64::from(edge_ds);
            let next = cost
                + search.pricing.price(&Transition {
                    leg: Leg::Interior,
                    prev: Some(d),
                    node: Some(v),
                    out: o,
                    time_ds,
                });
            let next_key = next + h(u);
            if bound.is_some_and(|b| next_key > b) {
                continue;
            }
            improve(work, pack(o, a2), next, state, next_key);
        }
    }

    let Some((cost, finish)) = best else {
        return Ok(None);
    };
    let mut dedges = Vec::new();
    let mut cursor = finish.state;
    while let Some(state) = cursor {
        dedges.push(unpack(state).0);
        let parent = work.labels[&state].parent;
        cursor = (parent != NO_PARENT).then_some(parent);
    }
    dedges.reverse();
    dedges.extend(finish.last);
    Ok(Some(TurnPath {
        cost,
        dedges,
        settled,
    }))
}
