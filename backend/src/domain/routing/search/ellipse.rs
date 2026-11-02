//! 楕円の切り出し（形式書8章・18章、計画書3.4節）。
//!
//! 辺の時間だけの最短時間（ノードの加算と右左折コストを除く）は本当の最短時間の下界になる。
//! 出発からの下界＋到着までの下界が `最短時間＋許容時間` 以下のノードだけを部分グラフにする。
// 骨組み：実装時に外す
#![allow(unused_variables)]

use crate::contract::{CancelToken, Interrupt};
use crate::domain::routing::graph::{DirEdge, NodeId};
use crate::domain::routing::snap::SnapPoint;

use super::SearchContext;

/// 部分グラフの有向辺（形式書18章）。`head` は部分グラフの局所番号。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubArc {
    /// 到着ノードの局所番号。
    pub head: u32,
    /// 元のグラフの有向辺。
    pub dedge: DirEdge,
    /// 辺の時間（デシ秒、ノードの加算・右左折コストを除く）。
    pub time_ds: u32,
    /// 辺のリスク（固定小数点）。
    pub risk_fx: u64,
}

/// 楕円の部分グラフ。局所番号で持つCSR形式（計画書7.2節）。
///
/// 出発・到着の吸着点は仮想ノードとして末尾に足す（`source`・`target`）。
/// 骨組みの段階の形で、実装時に探索側（LARAC・制約付き最短路）の必要に合わせて詰める。
#[derive(Debug, Clone)]
pub struct Subgraph {
    pub start: SnapPoint,
    pub end: SnapPoint,
    /// 局所番号→元のノード番号。仮想ノードは含まない。
    pub nodes: Vec<NodeId>,
    /// 長さ=ノード数（仮想ノードを含む）+1。
    pub node_ofs: Vec<u32>,
    pub arcs: Vec<SubArc>,
    /// 出発の仮想ノードの局所番号。
    pub source: u32,
    /// 到着の仮想ノードの局所番号。
    pub target: u32,
    /// 最短時間（デシ秒）。
    pub fastest_ds: u64,
    /// 許容する最大の時間（デシ秒）＝最短時間＋許容時間。
    pub budget_ds: u64,
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

/// 楕円の部分グラフを切り出す。`fastest_ds` は最短時間、`detour_ds` は許容する追加時間。
pub fn ellipse_subgraph(
    ctx: &SearchContext<'_>,
    start: &SnapPoint,
    end: &SnapPoint,
    fastest_ds: u64,
    detour_ds: u64,
    cancel: &CancelToken,
) -> Result<Subgraph, EllipseError> {
    todo!("形式書18章：楕円の切り出し")
}
