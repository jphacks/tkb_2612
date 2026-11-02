//! 最短時間経路（behavior.md 5章 手順2、形式書8章・18章）。
//!
//! 2段階で、右左折禁止・ノードの加算・右左折コスト・通過できないノードを守る合法な最短時間経路を求める。
//!
//! 1. **ノード単位の双方向A\***：辺の時間だけの最短時間 `T_lb`（右左折禁止・ノードの加算・右左折コストを
//!    含まないので、本当の最短時間の下界）と、始点からの下界 `df`、終点までの下界 `db` を求める。
//!    前向きと後ろ向きの2本のA*を交互に進め、どちらかの先頭のキーが出会いの最良値以上になったら `T_lb` が決まる。
//!    続けて、`T_lb + m` までキーが届くよう両方を再開して進める（キーが `T_lb + m` 以下のノードは、
//!    `df`・`db` が正確に確定している）。
//! 2. **状態つきの探索**（[`turn_dijkstra`](super::turn_dijkstra)）：`df(v) + db(v) ≤ T_lb + m` のノードだけで、
//!    「直前の有向辺＋照合器の状態」を状態にしたDijkstraを行う。見つかった時間が `T_lb + m` 以下なら、
//!    それが全体での合法な最短時間（それより速い合法な経路は、すべてのノードがこの条件を満たすため）。
//!    そうでなければ `m` を2倍にして1.の続きからやり直す。`m` の初期値は `max(600デシ秒, T_lb / 10)`。
//!    両方のA*が空になった（全ノードを確定した）ら、その探索の結果が最終。
//!    部分のノード数が `SearchParams::ellipse_node_limit` を超えたら、全体グラフで同じ探索
//!    （下界つきのA*）に切り替える。
//!
//! # 下界
//!
//! ノードから目的の位置までの直線距離を、その手段の最高速度で割った時間（デシ秒）に安全率
//! [`STRAIGHT_FACTOR`] を掛けた値を使う。安全率は、辺の長さの丸め（0.1m）と局所平面近似の誤差があっても、
//! 有向辺ごとに無矛盾（`h(u) ≤ w(u,v) + h(v)`）になる余裕である。`Landmarks` に距離表がある手段では、
//! [`Landmarks::lower_bound_ds`](super::alt::Landmarks::lower_bound_ds) による下界との大きい方を使う
//! （差し替え点は [`Potential::landmark_bound`]）。
//!
//! # 作業領域
//!
//! 全国グラフ（1,500万ノード）でも、N個の配列は確保しない。ノードのラベルも、状態つきの探索のラベルも、
//! 触れた数に比例するハッシュ表（[`FastestWork`]）に持つ。`fastest_path` は呼ぶたびに空の作業領域を作る。
//! 繰り返し呼ぶ側は `fastest_path_with` で作業領域を再利用できる（容量は残る。大きな探索のあとは
//! [`FastestWork::release`]）。

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::contract::geo::METERS_PER_DEGREE;
use crate::contract::{CancelToken, Interrupt, Mode};
use crate::domain::routing::graph::time::edge_time_ds;
use crate::domain::routing::graph::{Coord, DirEdge, NodeId};
use crate::domain::routing::snap::{SnapPoint, point_on_edge};

use super::endpoints::{Endpoints, Placement, path_time_ds};
use super::turn_dijkstra::{IdMap, TimeOnly, TurnSearch, TurnWork, turn_dijkstra};
use super::{Path, SearchContext};

/// 直線距離の下界に掛ける安全率。
pub const STRAIGHT_FACTOR: f64 = 0.8;

/// `m` の初期値の下限（デシ秒）。
const MIN_SLACK_DS: u64 = 600;

/// ノード単位のA*の状態：ラベル（確定前のものを含む最短時間の上界）と、キュー。
#[derive(Debug, Default)]
struct NodeSearch {
    dist: IdMap<NodeId, u64>,
    /// （キー = 時間 + 下界, 時間, ノード）。小さい順。
    heap: BinaryHeap<Reverse<(u64, u64, NodeId)>>,
}

impl NodeSearch {
    fn clear(&mut self) {
        self.dist.clear();
        self.heap.clear();
    }

    fn release(&mut self) {
        self.dist = IdMap::default();
        self.heap = BinaryHeap::new();
    }

    /// 時間 `g` のラベルが今より良ければ更新してキューに入れる。
    fn relax(&mut self, v: NodeId, g: u64, h: u64) -> bool {
        if self.dist.get(&v).is_some_and(|&old| old <= g) {
            return false;
        }
        self.dist.insert(v, g);
        self.heap.push(Reverse((g + h, g, v)));
        true
    }

    /// 古い項目を捨てた、先頭のキー。空なら `None`。
    fn peek_key(&mut self) -> Option<u64> {
        while let Some(&Reverse((key, g, v))) = self.heap.peek() {
            if self.dist.get(&v) == Some(&g) {
                return Some(key);
            }
            self.heap.pop();
        }
        None
    }

    fn pop(&mut self) -> Option<(u64, NodeId)> {
        self.peek_key()?;
        self.heap.pop().map(|Reverse((_, g, v))| (g, v))
    }
}

/// 最短時間経路の作業領域。触れたノード・状態の数に比例する。
#[derive(Debug, Default)]
pub struct FastestWork {
    forward: NodeSearch,
    backward: NodeSearch,
    states: TurnWork<u64>,
}

impl FastestWork {
    pub fn new() -> Self {
        Self::default()
    }

    /// 中身を捨てる（容量は残す）。`fastest_path_with` は呼び出しの最初に呼ぶ。
    pub fn clear(&mut self) {
        self.forward.clear();
        self.backward.clear();
        self.states.clear();
    }

    /// 確保した容量を手放す。大きな探索のあとに呼び、常駐のメモリを増やさない。
    pub fn release(&mut self) {
        self.forward.release();
        self.backward.release();
        self.states.release();
    }

    /// 直近の探索で触れたノードの数（前向きと後ろ向きの合計）と、状態つきの探索で触れた状態の数。
    /// 全体グラフでの探索に切り替えたあとは、ノードの数は0。
    pub fn touched(&self) -> (usize, usize) {
        (
            self.forward.dist.len() + self.backward.dist.len(),
            self.states.touched_states(),
        )
    }
}

/// 約1024回の拡張ごとに取り消しを確かめる。
struct Ticker(u32);

impl Ticker {
    fn tick(&mut self, cancel: &CancelToken) -> Result<(), Interrupt> {
        self.0 = self.0.wrapping_add(1);
        if self.0 & 1023 == 0 {
            cancel.check()
        } else {
            Ok(())
        }
    }
}

/// ノードから目標までの時間の下界（デシ秒、辺の時間だけ）。
struct Potential<'a> {
    ctx: &'a SearchContext<'a>,
    /// 目標の位置。前向きなら到着、後ろ向きなら出発。
    target: Coord,
    /// 1e-7度あたりのm（経度方向・緯度方向）。
    mx: f64,
    my: f64,
    /// 1mあたりのデシ秒（安全率込み）。最高速度が0なら0。
    ds_per_m: f64,
    /// 目標に着く直前のノードと、そこから目標までの時間。距離表の下界に使う。
    anchors: Vec<(NodeId, u64)>,
    forward: bool,
}

/// その手段の最高速度（km/h）。carは `maxspeed` による上振れも含める。
fn max_speed_kmh(ctx: &SearchContext<'_>) -> f64 {
    let model = ctx.graph.time_model();
    let base = model
        .speed_kmh
        .get(ctx.mode)
        .iter()
        .copied()
        .fold(0.0_f64, f64::max);
    if ctx.mode == Mode::Car {
        base * model.car_maxspeed_cap_ratio.max(1.0)
    } else {
        base
    }
}

impl<'a> Potential<'a> {
    fn new(
        ctx: &'a SearchContext<'a>,
        target: Coord,
        anchors: Vec<(NodeId, u64)>,
        forward: bool,
    ) -> Self {
        let my = METERS_PER_DEGREE / 1e7;
        let cos_lat = (f64::from(target.lat_e7) / 1e7)
            .to_radians()
            .cos()
            .max(0.05);
        let speed = max_speed_kmh(ctx);
        Potential {
            ctx,
            target,
            mx: my * cos_lat,
            my,
            ds_per_m: if speed > 0.0 {
                STRAIGHT_FACTOR * 36.0 / speed
            } else {
                0.0
            },
            anchors,
            forward,
        }
    }

    /// 距離表（ALT）による下界。表がない手段は `None`。
    ///
    /// 差し替え点：`Landmarks::lower_bound_ds(graph, mode, v, t)` は、ノードvから目的地tまでの最短時間
    /// （辺の時間だけ）の下界。前向きは目標が到着側なので `(v, 到着直前のノード)`、後ろ向きは
    /// 出発側から `v` までなので `(出発直後のノード, v)` の向きで引き、目標までの残りの時間を足して
    /// 最小を取る。
    fn landmark_bound(&self, v: NodeId) -> Option<u64> {
        self.ctx.landmarks.table(self.ctx.mode)?;
        self.anchors
            .iter()
            .map(|&(u, extra)| {
                let lb = if self.forward {
                    self.ctx
                        .landmarks
                        .lower_bound_ds(self.ctx.graph, self.ctx.mode, v, u)
                } else {
                    self.ctx
                        .landmarks
                        .lower_bound_ds(self.ctx.graph, self.ctx.mode, u, v)
                };
                u64::from(lb) + extra
            })
            .min()
    }

    fn at(&self, v: NodeId) -> u64 {
        let c = self.ctx.graph.node_coord(v);
        let dx = (f64::from(c.lon_e7) - f64::from(self.target.lon_e7)) * self.mx;
        let dy = (f64::from(c.lat_e7) - f64::from(self.target.lat_e7)) * self.my;
        let straight = (dx.hypot(dy) * self.ds_per_m).floor() as u64;
        match self.landmark_bound(v) {
            Some(alt) => straight.max(alt),
            None => straight,
        }
    }
}

/// 吸着点の位置（辺の途中なら辺上の点、ノード上ならそのノードの座標）。`SnapPoint::position` は信用せず、
/// 辺と `offset_dm` から作る。
fn anchor_position(ctx: &SearchContext<'_>, p: &SnapPoint, placement: Placement) -> Coord {
    match placement {
        Placement::Inside => point_on_edge(ctx.graph, p.edge, p.offset_dm),
        Placement::AtNode(n) => ctx.graph.node_coord(n),
    }
}

/// ノードvを拡張する：前向きなら出る有向辺、後ろ向きなら入る有向辺（逆向きにたどる）を1つ進める。
/// 反対側にもラベルがあるノードに着いたら、出会いの最良値 `meet` を更新する。
#[allow(clippy::too_many_arguments)]
fn expand(
    ctx: &SearchContext<'_>,
    me: &mut NodeSearch,
    other: &NodeSearch,
    meet: &mut u64,
    potential: &Potential<'_>,
    forward: bool,
    v: NodeId,
    g: u64,
) {
    let graph = ctx.graph;
    for &d in graph.out_dedges(v) {
        let arc = if forward { d } else { d.reversed() };
        let Some(w_ds) = edge_time_ds(graph, arc, ctx.mode) else {
            continue;
        };
        let u = graph.dedge_to(d);
        let next = g + u64::from(w_ds);
        if me.relax(u, next, potential.at(u))
            && let Some(&og) = other.dist.get(&u)
        {
            *meet = (*meet).min(next + og);
        }
    }
}

/// 先頭のキーが `limit` 以下のあいだ、拡張を進める。
#[allow(clippy::too_many_arguments)]
fn extend(
    ctx: &SearchContext<'_>,
    me: &mut NodeSearch,
    other: &NodeSearch,
    meet: &mut u64,
    potential: &Potential<'_>,
    forward: bool,
    limit: u64,
    ticker: &mut Ticker,
    cancel: &CancelToken,
) -> Result<(), Interrupt> {
    while me.peek_key().is_some_and(|key| key <= limit) {
        ticker.tick(cancel)?;
        let (g, v) = me.pop().expect("peek_keyで確かめた");
        expand(ctx, me, other, meet, potential, forward, v, g);
    }
    Ok(())
}

/// 出発から到着までの最短時間経路。到達できなければ `None`。
/// 返す `Path` の `risk_fx` は0（リスクは別に計算する）。
///
/// 吸着点が不正、または2点が同じ位置のときも `None`（呼び出し側は先に
/// [`is_same_position`](super::endpoints::is_same_position) で `NoRoute` にしてよい）。
/// 作業領域は呼び出しごとに作る。繰り返し呼ぶなら [`fastest_path_with`]。
pub fn fastest_path(
    ctx: &SearchContext<'_>,
    start: &SnapPoint,
    end: &SnapPoint,
    cancel: &CancelToken,
) -> Result<Option<Path>, Interrupt> {
    fastest_path_with(ctx, start, end, cancel, &mut FastestWork::new())
}

/// [`fastest_path`] を、作業領域を渡して呼ぶ。作業領域は最初に空にされる。
pub fn fastest_path_with(
    ctx: &SearchContext<'_>,
    start: &SnapPoint,
    end: &SnapPoint,
    cancel: &CancelToken,
    work: &mut FastestWork,
) -> Result<Option<Path>, Interrupt> {
    cancel.check()?;
    work.clear();
    let graph = ctx.graph;
    let Some(endpoints) = Endpoints::new(graph, ctx.mode, start, end) else {
        return Ok(None);
    };
    let found = search(ctx, &endpoints, cancel, work)?;
    Ok(found.map(|(time_ds, dedges)| {
        debug_assert_eq!(
            path_time_ds(graph, ctx.mode, start, end, &dedges),
            Some(time_ds),
            "探索の時間とpath_time_dsが食い違った"
        );
        Path {
            dedges,
            start: *start,
            end: *end,
            time_ds,
            risk_fx: 0,
        }
    }))
}

/// 手順2.1・2.2。見つかった（時間, 有向辺の並び）。
fn search(
    ctx: &SearchContext<'_>,
    ep: &Endpoints,
    cancel: &CancelToken,
    work: &mut FastestWork,
) -> Result<Option<(u64, Vec<DirEdge>)>, Interrupt> {
    let FastestWork {
        forward,
        backward,
        states,
    } = work;
    let graph = ctx.graph;

    // 後ろ向きは出発側へ、前向きは到着側へ向かう。
    let end_pos = anchor_position(ctx, &ep.end, ep.end_placement);
    let start_pos = anchor_position(ctx, &ep.start, ep.start_placement);
    let to_end: Vec<(NodeId, u64)> = match ep.end_node() {
        Some(n) => vec![(n, 0)],
        None => ep
            .arrivals
            .iter()
            .map(|a| (a.from, u64::from(a.time_ds)))
            .collect(),
    };
    let from_start: Vec<(NodeId, u64)> = match ep.start_node() {
        Some(n) => vec![(n, 0)],
        None => ep
            .departures
            .iter()
            .map(|a| (a.to, u64::from(a.time_ds)))
            .collect(),
    };
    let pot_f = Potential::new(ctx, end_pos, to_end.clone(), true);
    let pot_b = Potential::new(ctx, start_pos, from_start.clone(), false);

    // --- 手順2.1：出発側・到着側のラベルの種をまく ---
    for &(n, ds) in &from_start {
        forward.relax(n, ds, pot_f.at(n));
    }
    for &(n, ds) in &to_end {
        backward.relax(n, ds, pot_b.at(n));
    }
    let mut meet = u64::MAX;
    for (v, &g) in &forward.dist {
        if let Some(&og) = backward.dist.get(v) {
            meet = meet.min(g + og);
        }
    }
    let direct_ds = ep.direct.map_or(u64::MAX, |a| u64::from(a.time_ds));
    let mut ticker = Ticker(0);

    loop {
        let cap = meet.min(direct_ds);
        let (Some(kf), Some(kb)) = (forward.peek_key(), backward.peek_key()) else {
            break;
        };
        if kf >= cap || kb >= cap {
            break;
        }
        ticker.tick(cancel)?;
        if kf <= kb {
            let (g, v) = forward.pop().expect("peek_keyで確かめた");
            expand(ctx, forward, backward, &mut meet, &pot_f, true, v, g);
        } else {
            let (g, v) = backward.pop().expect("peek_keyで確かめた");
            expand(ctx, backward, forward, &mut meet, &pot_b, false, v, g);
        }
    }
    let t_lb = meet.min(direct_ds);
    if t_lb == u64::MAX {
        return Ok(None);
    }

    // --- 手順2.2：df + db ≤ T_lb + m のノードだけで状態つきの探索 ---
    let limit = ctx.params.ellipse_node_limit as usize;
    let mut slack = MIN_SLACK_DS.max(t_lb / 10);
    loop {
        let bound = t_lb.saturating_add(slack);
        extend(
            ctx,
            forward,
            backward,
            &mut meet,
            &pot_f,
            true,
            bound,
            &mut ticker,
            cancel,
        )?;
        extend(
            ctx,
            backward,
            forward,
            &mut meet,
            &pot_b,
            false,
            bound,
            &mut ticker,
            cancel,
        )?;
        let exhausted = forward.peek_key().is_none() && backward.peek_key().is_none();
        let r = (!exhausted).then_some(bound);

        let in_part = |a: u64, b: u64| r.is_none_or(|r| a.saturating_add(b) <= r);
        if forward.dist.len().min(backward.dist.len()) > limit {
            let (small, big) = if forward.dist.len() <= backward.dist.len() {
                (&forward.dist, &backward.dist)
            } else {
                (&backward.dist, &forward.dist)
            };
            let nodes = small
                .iter()
                .filter(|&(v, &a)| big.get(v).is_some_and(|&b| in_part(a, b)))
                .count();
            if nodes > limit {
                return full_graph(ctx, ep, &pot_f, cancel, forward, backward, states);
            }
        }

        let member = |v: NodeId| match (forward.dist.get(&v), backward.dist.get(&v)) {
            (Some(&a), Some(&b)) => in_part(a, b),
            _ => false,
        };
        let found = turn_dijkstra(
            &mut TurnSearch {
                graph,
                automaton: ctx.automaton,
                mode: ctx.mode,
                endpoints: ep,
                pricing: TimeOnly,
                filter: Some(&member),
                potential: None,
                bound: r,
            },
            states,
            cancel,
        )?;
        match found {
            Some(path) => return Ok(Some((path.cost, path.dedges))),
            None if exhausted => return Ok(None),
            None => slack = slack.saturating_mul(2),
        }
    }
}

/// 部分のノード数が上限を超えたときの、全体グラフでの同じ探索（到着までの下界つきのA*）。
fn full_graph(
    ctx: &SearchContext<'_>,
    ep: &Endpoints,
    potential: &Potential<'_>,
    cancel: &CancelToken,
    forward: &mut NodeSearch,
    backward: &mut NodeSearch,
    states: &mut TurnWork<u64>,
) -> Result<Option<(u64, Vec<DirEdge>)>, Interrupt> {
    forward.release();
    backward.release();
    let h = |v: NodeId| potential.at(v);
    let found = turn_dijkstra(
        &mut TurnSearch {
            graph: ctx.graph,
            automaton: ctx.automaton,
            mode: ctx.mode,
            endpoints: ep,
            pricing: TimeOnly,
            filter: None,
            potential: Some(&h),
            bound: None,
        },
        states,
        cancel,
    )?;
    Ok(found.map(|p| (p.cost, p.dedges)))
}
