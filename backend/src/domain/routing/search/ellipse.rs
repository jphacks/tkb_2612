//! 楕円の部分グラフ（形式書8章・18章、計画書3.4節、behavior.md 5章 手順4）。
//!
//! 辺の時間だけの最短時間（ノードの加算と右左折コストを除く）は本当の最短時間の下界になる。
//! 出発からの下界＋到着までの下界が `最短時間＋許容時間` 以下の有向辺だけを部分グラフにする。
//!
//! # 部分グラフの意味（探索側との約束）
//!
//! `Subgraph` は局所番号のCSRで、実ノード `0..n` のあとに出発の仮想ノード `source = n` と到着の
//! 仮想ノード `target = n + 1` を足す。探索の状態は「局所ノード、直前の有向辺、照合器の状態」で、
//! 次は **部分グラフに入れず、探索が辺をつなぐときに `ctx.graph`・`ctx.automaton` から加える**。
//!
//! - `SubArc::time_ds`：辺の時間だけ（出発・到着の辺は通る部分だけ）。ノードの加算
//!   （`node_penalty_ds`）と右左折コスト（`turn_cost_ds`）は、直前の有向辺を持つノードから
//!   次の辺で出るときに足す。
//! - 右左折禁止：照合器が禁止の列の末尾に達する遷移は通れない。
//! - 通過できないノード（`Graph::node_blocked`）：直前の有向辺を持つノードからは出られない
//!   （経路の途中のノードになるため）。出発・到着のノードとしては使える。
//!
//! # 出発・到着の仮想ノード（部分有向辺）
//!
//! 出発の吸着点（辺 `e`、辺の始点からの距離 `s`、辺の長さ `L`）について、`source` から出る辺は：
//!
//! - `0 < s < L`：その手段で通れる向きごとに1本。`dedge` は `e` の有向辺で、`time_ds`・`risk_fx` は
//!   出発点から辺の終端までの部分（向き0なら `[s, L]`、向き1なら `[0, s]`）。`head` は有向辺の
//!   到着ノード。到着の吸着点も同じ辺の `0 < t < L` で、`t` が進行方向にあるときは、`target` へ直接つなぐ
//!   辺（`dedge` は同じ有向辺、区間は `s` と `t` のあいだ）も足す。
//! - `s = 0` または `s = L`：その端のノードから出発するだけなので、`dedge = NO_DEDGE`、時間・リスク0の
//!   辺でそのノードにつなぐ（直前の有向辺を持たない状態。ノードの加算も右左折コストも足さない）。
//!   そのノードから出る辺はすべて通常の辺として使える。
//!
//! 到着の吸着点（辺 `e`、距離 `t`）について、`target` に入る辺は：
//!
//! - `0 < t < L`：その手段で通れる向きごとに、有向辺の出発ノードの辺の集まりに1本。`dedge` は `e` の
//!   有向辺で、区間は向き0なら `[0, t]`、向き1なら `[t, L]`。
//! - `t = 0` または `t = L`：その端のノードの辺の集まりに、`dedge = NO_DEDGE`、時間・リスク0の辺を
//!   1本。直前の有向辺があるときだけ使え、そのノードで終わる（ノードの加算も右左折コストも足さない）。
//!
//! 部分的に通る辺の時間とリスクは `partial_edge_time_ds`・`ArcCost::partial_risk_fx` の値で、
//! 通る長さは1以上なので時間は正になる。端のノードの吸着は、ノードから出る別の辺へ進めるように
//! するためである（辺の途中だけを仮想ノードにすると、端の吸着点から他の辺に出られない）。
//!
//! `Path::dedges` は `NO_DEDGE` を含まない。出発・到着の辺の部分は `Path` のドキュメントの規則
//! （先頭の有向辺が出発の辺なら出発点から、最後の有向辺が到着の辺なら到着点まで）で決まる。

use crate::contract::{CancelToken, Interrupt};
use crate::domain::routing::graph::time::{edge_time_ds, partial_edge_time_ds};
use crate::domain::routing::graph::{DirEdge, NodeId};
use crate::domain::routing::snap::SnapPoint;

use super::ArcCost;
use super::SearchContext;

/// 直前の有向辺を持たない出発・到着を表す印（`SubArc::dedge`）。実在の有向辺の番号とは重ならない。
pub const NO_DEDGE: DirEdge = DirEdge(u32::MAX);

/// 部分グラフの有向辺（形式書18章）。`head` は部分グラフの局所番号。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubArc {
    /// 到着ノードの局所番号（到着の仮想ノードのこともある）。
    pub head: u32,
    /// 元のグラフの有向辺。ノードで出発・到着するだけの辺は `NO_DEDGE`。
    pub dedge: DirEdge,
    /// 辺の時間（デシ秒、ノードの加算・右左折コストを除く）。出発・到着の辺は通る部分だけ。
    pub time_ds: u32,
    /// 辺のリスク（固定小数点）。出発・到着の辺は通る部分だけ。クラスタに入るときの加算を含む。
    pub risk_fx: u64,
}

impl SubArc {
    /// ノードで出発・到着するだけの辺か（`dedge` が `NO_DEDGE`）。
    pub fn is_node_arc(&self) -> bool {
        self.dedge == NO_DEDGE
    }
}

/// 楕円の部分グラフ。局所番号で持つCSR形式（計画書7.2節）。意味はモジュールのドキュメントのとおり。
///
/// 出発・到着の吸着点は仮想ノードとして末尾に足す（`source`・`target`）。到着の仮想ノードから出る
/// 辺はない。
#[derive(Debug, Clone)]
pub struct Subgraph {
    pub start: SnapPoint,
    pub end: SnapPoint,
    /// 局所番号→元のノード番号。仮想ノードは含まない。
    pub nodes: Vec<NodeId>,
    /// 長さ=ノード数（仮想ノードを含む）+1。局所ノード `v` の辺は `arcs[node_ofs[v]..node_ofs[v+1]]`。
    pub node_ofs: Vec<u32>,
    pub arcs: Vec<SubArc>,
    /// 出発の仮想ノードの局所番号（`nodes.len()`）。
    pub source: u32,
    /// 到着の仮想ノードの局所番号（`nodes.len() + 1`）。
    pub target: u32,
    /// 最短時間（デシ秒）。ノードの加算と右左折コストを含む、合法な最短時間経路の時間。
    pub fastest_ds: u64,
    /// 許容する最大の時間（デシ秒）＝最短時間＋許容時間。
    pub budget_ds: u64,
}

impl Subgraph {
    /// 仮想ノードを含むノード数。
    pub fn node_count(&self) -> usize {
        self.nodes.len() + 2
    }

    /// 局所ノード（仮想ノードを含む）から出る辺。
    pub fn arcs_of(&self, node: u32) -> &[SubArc] {
        let v = node as usize;
        &self.arcs[self.node_ofs[v] as usize..self.node_ofs[v + 1] as usize]
    }
}

/// 楕円の切り出しの失敗。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EllipseError {
    /// ノード数が `SearchParams::ellipse_node_limit` を超えた（`reduced_ellipse` で縮小する）。
    TooLarge {
        nodes: u64,
    },
    /// 出発から到着へ到達できない。
    Unreachable,
    Interrupted(Interrupt),
}

impl From<Interrupt> for EllipseError {
    fn from(value: Interrupt) -> Self {
        EllipseError::Interrupted(value)
    }
}

/// 出発・到着の仮想ノードにつなぐ辺の素。`head`・`tail` は元のグラフのノード番号。
struct EndpointArcs {
    /// 出発の仮想ノードから出る辺。`Some(node)` は元のグラフの到着ノード、`None` は到着の仮想ノードへ直接。
    from_source: Vec<(Option<NodeId>, Proto)>,
    /// 元のグラフのノードから到着の仮想ノードへ入る辺。
    into_target: Vec<(NodeId, Proto)>,
}

#[derive(Clone, Copy)]
struct Proto {
    dedge: DirEdge,
    time_ds: u32,
    risk_fx: u64,
}

impl Proto {
    fn node_arc() -> Proto {
        Proto {
            dedge: NO_DEDGE,
            time_ds: 0,
            risk_fx: 0,
        }
    }
}

/// 出発・到着の吸着点から、仮想ノードにつなぐ辺を作る（モジュールのドキュメントの規則）。
fn endpoint_arcs(
    ctx: &SearchContext<'_>,
    start: &SnapPoint,
    end: &SnapPoint,
    cost: &mut dyn ArcCost,
) -> EndpointArcs {
    let graph = ctx.graph;
    let mode = ctx.mode;
    let mut from_source = Vec::new();
    let mut into_target = Vec::new();

    let start_record = graph.edge(start.edge);
    let start_len = start_record.length_dm;
    let s = start.offset_dm.min(start_len);
    let end_record = graph.edge(end.edge);
    let end_len = end_record.length_dm;
    let t = end.offset_dm.min(end_len);
    let start_interior = s > 0 && s < start_len;
    let end_interior = t > 0 && t < end_len;

    if s == 0 {
        from_source.push((Some(start_record.tail), Proto::node_arc()));
    } else if s == start_len {
        from_source.push((Some(start_record.head), Proto::node_arc()));
    } else {
        for reverse in [false, true] {
            let d = DirEdge::new(start.edge, reverse);
            let (from_dm, to_dm) = if reverse { (0, s) } else { (s, start_len) };
            let Some(time_ds) = partial_edge_time_ds(graph, d, mode, to_dm - from_dm) else {
                continue;
            };
            let risk_fx = cost
                .partial_risk_fx(d, from_dm, to_dm)
                .saturating_add(cost.cluster_entry_risk_fx(d));
            from_source.push((
                Some(graph.dedge_to(d)),
                Proto {
                    dedge: d,
                    time_ds,
                    risk_fx,
                },
            ));
        }
    }

    if t == 0 {
        into_target.push((end_record.tail, Proto::node_arc()));
    } else if t == end_len {
        into_target.push((end_record.head, Proto::node_arc()));
    } else {
        for reverse in [false, true] {
            let d = DirEdge::new(end.edge, reverse);
            let (from_dm, to_dm) = if reverse { (t, end_len) } else { (0, t) };
            let Some(time_ds) = partial_edge_time_ds(graph, d, mode, to_dm - from_dm) else {
                continue;
            };
            let risk_fx = cost.partial_risk_fx(d, from_dm, to_dm);
            into_target.push((
                graph.dedge_from(d),
                Proto {
                    dedge: d,
                    time_ds,
                    risk_fx,
                },
            ));
        }
    }

    // 同じ辺の途中から途中へ、辺の中だけで着く場合。
    if start.edge == end.edge && start_interior && end_interior && s != t {
        let reverse = t < s;
        let d = DirEdge::new(start.edge, reverse);
        let (from_dm, to_dm) = if reverse { (t, s) } else { (s, t) };
        if let Some(time_ds) = partial_edge_time_ds(graph, d, mode, to_dm - from_dm) {
            let risk_fx = cost.partial_risk_fx(d, from_dm, to_dm);
            from_source.push((
                None,
                Proto {
                    dedge: d,
                    time_ds,
                    risk_fx,
                },
            ));
        }
    }

    EndpointArcs {
        from_source,
        into_target,
    }
}

/// 部分グラフを組み立てる。楕円の切り出しと縮小と全体グラフが共有する。
///
/// `nodes` は局所番号の順の元のノード、`local_of` は元のノードから局所番号（含まれなければ `None`）、
/// `keep` は残す有向辺（その手段で通れる有向辺のうち）。辺の時間とリスクを `cost` から付ける。
#[allow(clippy::too_many_arguments)]
pub(super) fn assemble(
    ctx: &SearchContext<'_>,
    start: &SnapPoint,
    end: &SnapPoint,
    fastest_ds: u64,
    budget_ds: u64,
    nodes: Vec<NodeId>,
    local_of: &dyn Fn(NodeId) -> Option<u32>,
    keep: &dyn Fn(DirEdge) -> bool,
    cost: &mut dyn ArcCost,
    cancel: &CancelToken,
) -> Result<Subgraph, Interrupt> {
    let graph = ctx.graph;
    let n = nodes.len() as u32;
    let source = n;
    let target = n + 1;
    let endpoints = endpoint_arcs(ctx, start, end, cost);

    let mut into_target: Vec<(u32, SubArc)> = endpoints
        .into_target
        .into_iter()
        .filter_map(|(node, proto)| {
            local_of(node).map(|l| {
                (
                    l,
                    SubArc {
                        head: target,
                        dedge: proto.dedge,
                        time_ds: proto.time_ds,
                        risk_fx: proto.risk_fx,
                    },
                )
            })
        })
        .collect();
    into_target.sort_by_key(|&(l, _)| l);

    let mut node_ofs = Vec::with_capacity(nodes.len() + 3);
    let mut arcs: Vec<SubArc> = Vec::new();
    let mut next_target_arc = 0;
    for (l, &g) in nodes.iter().enumerate() {
        if l & 1023 == 0 {
            cancel.check()?;
        }
        node_ofs.push(arcs.len() as u32);
        for &d in graph.out_dedges(g) {
            if !keep(d) {
                continue;
            }
            let Some(time_ds) = edge_time_ds(graph, d, ctx.mode) else {
                continue;
            };
            let Some(head) = local_of(graph.dedge_to(d)) else {
                continue;
            };
            let risk_fx = cost
                .risk_fx(d)
                .saturating_add(cost.cluster_entry_risk_fx(d));
            arcs.push(SubArc {
                head,
                dedge: d,
                time_ds,
                risk_fx,
            });
        }
        while next_target_arc < into_target.len() && into_target[next_target_arc].0 == l as u32 {
            arcs.push(into_target[next_target_arc].1);
            next_target_arc += 1;
        }
    }
    // 出発の仮想ノードの辺。
    node_ofs.push(arcs.len() as u32);
    for (head, proto) in endpoints.from_source {
        let head = match head {
            Some(node) => match local_of(node) {
                Some(l) => l,
                None => continue,
            },
            None => target,
        };
        arcs.push(SubArc {
            head,
            dedge: proto.dedge,
            time_ds: proto.time_ds,
            risk_fx: proto.risk_fx,
        });
    }
    // 到着の仮想ノード（辺なし）と末尾。
    node_ofs.push(arcs.len() as u32);
    node_ofs.push(arcs.len() as u32);

    Ok(Subgraph {
        start: *start,
        end: *end,
        nodes,
        node_ofs,
        arcs,
        source,
        target,
        fastest_ds,
        budget_ds,
    })
}

/// 楕円で切らずに、全体グラフをそのまま部分グラフにする（局所番号は元のノード番号と同じ）。
/// 部分グラフ上の探索のテストと、楕円と比べる基準に使う。ノード数が大きいグラフでは重い。
/// `fastest_ds` は合法な最短時間、`detour_ds` は許容する追加時間。
pub fn whole_graph_subgraph(
    ctx: &SearchContext<'_>,
    start: &SnapPoint,
    end: &SnapPoint,
    fastest_ds: u64,
    detour_ds: u64,
    cost: &mut dyn ArcCost,
    cancel: &CancelToken,
) -> Result<Subgraph, Interrupt> {
    let n = ctx.graph.node_count() as u32;
    assemble(
        ctx,
        start,
        end,
        fastest_ds,
        fastest_ds + detour_ds,
        (0..n).collect(),
        &Some,
        &|_| true,
        cost,
        cancel,
    )
}

/// 楕円の部分グラフを切り出す。`fastest_ds` は最短時間、`detour_ds` は許容する追加時間。
///
/// 第2段階で実装する（`search::endpoints` と `fastest_path` の統合後）。
#[allow(unused_variables)]
pub fn ellipse_subgraph(
    ctx: &SearchContext<'_>,
    start: &SnapPoint,
    end: &SnapPoint,
    fastest_ds: u64,
    detour_ds: u64,
    cost: &mut dyn ArcCost,
    cancel: &CancelToken,
) -> Result<Subgraph, EllipseError> {
    todo!("形式書18章：楕円の切り出し")
}
