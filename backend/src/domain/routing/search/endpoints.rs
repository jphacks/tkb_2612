//! 出発・到着の扱いと、経路の時間・距離の正本の計算（形式書8章・11章、behavior.md 5章）。
//!
//! 最短時間経路（`fastest`）と、リスクを考える探索（M3a）が共有する。出発地・目的地は辺の途中の
//! 吸着点（`SnapPoint`）なので、経路の最初の有向辺は出発の吸着点から辺の端点まで、最後の有向辺は
//! 辺の端点から到着の吸着点までの**部分有向辺**になる。部分有向辺の `DirEdge` は、その辺の有向辺
//! そのもの（右左折禁止の照合器・`legality::check_path` にそのまま入れる）。
//!
//! # 経路の表し方
//!
//! `dedges` は、出発から到着までの有向辺の並び。隣り合う有向辺は、前の到着ノードと後ろの出発ノードが
//! 同じ。最初と最後の有向辺の通る長さは、吸着点の位置で決まる。
//!
//! | 吸着点 | 位置（`offset_dm`） | 最初（最後）の有向辺 |
//! |---|---|---|
//! | 辺の途中（[`Placement::Inside`]） | 0より大きく `length_dm` 未満 | その辺の有向辺。出発なら吸着点から進行方向の端点まで、到着なら進行方向の手前の端点から吸着点まで |
//! | ノード上（[`Placement::AtNode`]） | 0（始点）か `length_dm`（終点） | 辺は問わない。出発ならそのノードから出る有向辺を全長、到着ならそのノードに入る有向辺を全長通る |
//!
//! ノード上の吸着点には、長さ0の部分有向辺を付けない。付けると、出発地が信号のノードにちょうど
//! 乗っているだけで信号の加算が掛かり、実際には存在しない右左折禁止・右左折コストが掛かるため。
//! 出発と到着が同じ辺の途中にあるときは、1本の有向辺（吸着点から吸着点まで）で直接行ける場合がある
//! （[`Endpoints::direct`]）。行けない向きなら、辺の端点まで出て戻る形（最初と最後が同じ辺の有向辺）になる。
//!
//! # 時間と長さの正本
//!
//! [`path_time_ds`] が経路の時間の正本：辺の時間（最初と最後は通った部分。`partial_edge_time_ds`）
//! ＋途中のノードの加算（`node_penalty_ds`。隣り合う有向辺の間のノードすべて）＋右左折コスト
//! （`turn_cost_ds`。隣り合う有向辺の組すべて）。最初の有向辺の出発ノードと最後の有向辺の到着ノード
//! には加算しない。[`path_distance_dm`] は通った長さ（最初と最後は通った部分）。
//!
//! 同じ位置（[`is_same_position`]）の2点は経路にならない。呼び出し側が `NoRoute` にする。

use crate::contract::Mode;
use crate::domain::routing::graph::time::{node_penalty_ds, partial_edge_time_ds, turn_cost_ds};
use crate::domain::routing::graph::{DirEdge, Graph, NodeId};
use crate::domain::routing::snap::SnapPoint;

/// 吸着点が辺の途中か、ノードの上か。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// 辺の途中（`0 < offset_dm < length_dm`）。
    Inside,
    /// ノードの上（`offset_dm` が0なら辺の始点、`length_dm` なら終点）。
    AtNode(NodeId),
}

/// 吸着点の位置の種別。辺が範囲外、または `offset_dm` が辺の長さを超える不正な吸着点は `None`。
pub fn placement(graph: &Graph, p: &SnapPoint) -> Option<Placement> {
    if p.edge as usize >= graph.edge_count() {
        return None;
    }
    let record = graph.edge(p.edge);
    if p.offset_dm > record.length_dm {
        None
    } else if p.offset_dm == 0 {
        Some(Placement::AtNode(record.tail))
    } else if p.offset_dm == record.length_dm {
        Some(Placement::AtNode(record.head))
    } else {
        Some(Placement::Inside)
    }
}

/// 2つの吸着点が同じ位置か。同じ辺の同じ `offset_dm`、または同じノードの上。
/// 不正な吸着点は偽。同じ位置の出発と到着は経路にならない（呼び出し側が `NoRoute` にする）。
pub fn is_same_position(graph: &Graph, a: &SnapPoint, b: &SnapPoint) -> bool {
    match (placement(graph, a), placement(graph, b)) {
        (Some(Placement::AtNode(x)), Some(Placement::AtNode(y))) => x == y,
        (Some(Placement::Inside), Some(Placement::Inside)) => {
            a.edge == b.edge && a.offset_dm == b.offset_dm
        }
        _ => false,
    }
}

/// 出発・到着で通る有向辺（部分有向辺）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartialArc {
    /// その辺の有向辺（照合器・`check_path` にそのまま入れる）。
    pub dedge: DirEdge,
    /// 有向辺の出発ノード。
    pub from: NodeId,
    /// 有向辺の到着ノード。
    pub to: NodeId,
    /// 通る長さ（デシメートル）。
    pub travelled_dm: u32,
    /// 通る時間（デシ秒。`partial_edge_time_ds`）。
    pub time_ds: u32,
}

/// 有向辺 `d` を `travelled_dm` だけ通る部分有向辺。その手段で通れない、時間が決まらない組は `None`。
fn partial_arc(graph: &Graph, mode: Mode, d: DirEdge, travelled_dm: u32) -> Option<PartialArc> {
    let time_ds = partial_edge_time_ds(graph, d, mode, travelled_dm)?;
    Some(PartialArc {
        dedge: d,
        from: graph.dedge_from(d),
        to: graph.dedge_to(d),
        travelled_dm,
        time_ds,
    })
}

/// 出発の部分有向辺（出発の吸着点から先の最初の有向辺）。その手段で通れるものだけ。
///
/// - 辺の途中：辺の2つの向き。向き0は吸着点から終点まで（`length_dm - offset_dm`）、
///   向き1は始点まで（`offset_dm`）。
/// - ノード上：そのノードから出る有向辺を全長（辺は問わない）。
pub fn departure_arcs(graph: &Graph, mode: Mode, start: &SnapPoint) -> Vec<PartialArc> {
    match placement(graph, start) {
        None => Vec::new(),
        Some(Placement::Inside) => {
            let length = graph.edge(start.edge).length_dm;
            [
                (DirEdge::new(start.edge, false), length - start.offset_dm),
                (DirEdge::new(start.edge, true), start.offset_dm),
            ]
            .into_iter()
            .filter_map(|(d, travelled)| partial_arc(graph, mode, d, travelled))
            .collect()
        }
        Some(Placement::AtNode(n)) => graph
            .out_dedges(n)
            .iter()
            .filter_map(|&d| partial_arc(graph, mode, d, graph.edge(d.edge()).length_dm))
            .collect(),
    }
}

/// 到着の部分有向辺（到着の吸着点に入る最後の有向辺）。その手段で通れるものだけ。
///
/// - 辺の途中：辺の2つの向き。向き0は始点から吸着点まで（`offset_dm`）、
///   向き1は終点から吸着点まで（`length_dm - offset_dm`）。
/// - ノード上：空（そのノードに着いた時点で到着。最後の有向辺は普通の有向辺）。
pub fn arrival_arcs(graph: &Graph, mode: Mode, end: &SnapPoint) -> Vec<PartialArc> {
    match placement(graph, end) {
        Some(Placement::Inside) => {
            let length = graph.edge(end.edge).length_dm;
            [
                (DirEdge::new(end.edge, false), end.offset_dm),
                (DirEdge::new(end.edge, true), length - end.offset_dm),
            ]
            .into_iter()
            .filter_map(|(d, travelled)| partial_arc(graph, mode, d, travelled))
            .collect()
        }
        _ => Vec::new(),
    }
}

/// 出発と到着が同じ辺の途中にあるとき、1本の有向辺で直接行ける場合のその有向辺。
/// 到着が出発より終点側なら向き0、始点側なら向き1（その向きに通れなければ `None`）。
/// 同じ位置、辺が違う、どちらかがノード上の場合は `None`。
pub fn direct_arc(
    graph: &Graph,
    mode: Mode,
    start: &SnapPoint,
    end: &SnapPoint,
) -> Option<PartialArc> {
    if start.edge != end.edge
        || placement(graph, start)? != Placement::Inside
        || placement(graph, end)? != Placement::Inside
    {
        return None;
    }
    if end.offset_dm > start.offset_dm {
        partial_arc(
            graph,
            mode,
            DirEdge::new(start.edge, false),
            end.offset_dm - start.offset_dm,
        )
    } else if end.offset_dm < start.offset_dm {
        partial_arc(
            graph,
            mode,
            DirEdge::new(start.edge, true),
            start.offset_dm - end.offset_dm,
        )
    } else {
        None
    }
}

/// 1回の探索の出発・到着の扱い。探索の始まりと終わりの選択肢を、吸着点から1度だけ作る。
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub start: SnapPoint,
    pub end: SnapPoint,
    pub start_placement: Placement,
    pub end_placement: Placement,
    /// 出発の部分有向辺（[`departure_arcs`]）。
    pub departures: Vec<PartialArc>,
    /// 到着の部分有向辺（[`arrival_arcs`]）。到着がノード上なら空。
    pub arrivals: Vec<PartialArc>,
    /// 同じ辺の途中で1本の有向辺で直接行ける場合（[`direct_arc`]）。
    pub direct: Option<PartialArc>,
}

impl Endpoints {
    /// 出発・到着の選択肢を作る。吸着点が不正、または2点が同じ位置（[`is_same_position`]）なら `None`。
    pub fn new(graph: &Graph, mode: Mode, start: &SnapPoint, end: &SnapPoint) -> Option<Endpoints> {
        let start_placement = placement(graph, start)?;
        let end_placement = placement(graph, end)?;
        if is_same_position(graph, start, end) {
            return None;
        }
        Some(Endpoints {
            start: *start,
            end: *end,
            start_placement,
            end_placement,
            departures: departure_arcs(graph, mode, start),
            arrivals: arrival_arcs(graph, mode, end),
            direct: direct_arc(graph, mode, start, end),
        })
    }

    /// 出発がノード上なら、そのノード。
    pub fn start_node(&self) -> Option<NodeId> {
        match self.start_placement {
            Placement::AtNode(n) => Some(n),
            Placement::Inside => None,
        }
    }

    /// 到着がノード上なら、そのノード。
    pub fn end_node(&self) -> Option<NodeId> {
        match self.end_placement {
            Placement::AtNode(n) => Some(n),
            Placement::Inside => None,
        }
    }
}

/// 経路の各有向辺が通る長さ（デシメートル）。経路の形が正しくなければ `None`。
///
/// 正しい形：`dedges` が空でない、辺番号が範囲内、隣り合う有向辺がつながっている、出発がノード上なら
/// 最初の有向辺の出発ノードがそのノード、辺の途中なら最初の有向辺が出発の辺の有向辺、到着も同様
/// （最後の有向辺の到着ノード、または到着の辺）、通る長さが負にならない。通れるかどうかは見ない
/// （`legality::check_path` の仕事）。
pub fn travelled_lengths_dm(
    graph: &Graph,
    start: &SnapPoint,
    end: &SnapPoint,
    dedges: &[DirEdge],
) -> Option<Vec<u32>> {
    let (&first, &last) = (dedges.first()?, dedges.last()?);
    let start_placement = placement(graph, start)?;
    let end_placement = placement(graph, end)?;
    if dedges
        .iter()
        .any(|d| d.edge() as usize >= graph.edge_count())
    {
        return None;
    }
    if dedges
        .windows(2)
        .any(|w| graph.dedge_to(w[0]) != graph.dedge_from(w[1]))
    {
        return None;
    }
    match start_placement {
        Placement::Inside if first.edge() != start.edge => return None,
        Placement::AtNode(n) if graph.dedge_from(first) != n => return None,
        _ => {}
    }
    match end_placement {
        Placement::Inside if last.edge() != end.edge => return None,
        Placement::AtNode(n) if graph.dedge_to(last) != n => return None,
        _ => {}
    }
    let mut lengths = Vec::with_capacity(dedges.len());
    for (i, &d) in dedges.iter().enumerate() {
        let length = i64::from(graph.edge(d.edge()).length_dm);
        // 有向辺の進行方向に見た、通り始めと通り終わりの位置（始点からの距離）。
        let (mut from, mut to) = if d.is_reverse() {
            (length, 0)
        } else {
            (0, length)
        };
        if i == 0 && start_placement == Placement::Inside {
            from = i64::from(start.offset_dm);
        }
        if i + 1 == dedges.len() && end_placement == Placement::Inside {
            to = i64::from(end.offset_dm);
        }
        let travelled = if d.is_reverse() { from - to } else { to - from };
        lengths.push(u32::try_from(travelled).ok()?);
    }
    Some(lengths)
}

/// 経路の時間の正本（デシ秒）：辺の時間（最初と最後は通った部分）＋途中のノードの加算＋右左折コスト。
///
/// 隣り合う有向辺 `d_i`・`d_{i+1}` の間のノード（`d_i` の到着ノード）に `node_penalty_ds`、
/// 組 `(d_i, d_{i+1})` に `turn_cost_ds` を足す。形が正しくない（[`travelled_lengths_dm`] が `None`）、
/// その手段で通れない有向辺を含む場合は `None`。右左折禁止・通過できないノードは見ない。
pub fn path_time_ds(
    graph: &Graph,
    mode: Mode,
    start: &SnapPoint,
    end: &SnapPoint,
    dedges: &[DirEdge],
) -> Option<u64> {
    let lengths = travelled_lengths_dm(graph, start, end, dedges)?;
    let mut total = 0_u64;
    for (i, &d) in dedges.iter().enumerate() {
        total += u64::from(partial_edge_time_ds(graph, d, mode, lengths[i])?);
        if let Some(&next) = dedges.get(i + 1) {
            total += u64::from(node_penalty_ds(graph, graph.dedge_to(d), mode));
            total += u64::from(turn_cost_ds(graph, d, next, mode));
        }
    }
    Some(total)
}

/// 経路の通った長さ（デシメートル）。最初と最後の有向辺は通った部分の長さ。
/// 形が正しくなければ `None`。
pub fn path_distance_dm(
    graph: &Graph,
    start: &SnapPoint,
    end: &SnapPoint,
    dedges: &[DirEdge],
) -> Option<u64> {
    let lengths = travelled_lengths_dm(graph, start, end, dedges)?;
    Some(lengths.iter().map(|&l| u64::from(l)).sum())
}
