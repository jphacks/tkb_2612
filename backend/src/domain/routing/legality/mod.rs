//! 経路の合法性の検査（形式書18章、作業書の合法性試験）。
//! 探索の結果が、その手段で通れる有向辺の連なりで、右左折禁止と通過できないノードに違反しないことを確かめる。
// 骨組み：実装時に外す
#![allow(unused_variables)]

use crate::contract::Mode;
use crate::domain::routing::graph::restrictions::Restrictions;
use crate::domain::routing::graph::{DirEdge, Graph};

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
pub fn check_path(
    graph: &Graph,
    restrictions: &Restrictions,
    mode: Mode,
    dedges: &[DirEdge],
) -> Vec<Violation> {
    todo!("経路の合法性の検査")
}
