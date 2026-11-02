//! LARAC（ラグランジュ緩和による制約付き最短路の近似、形式書18章）。
//! 時間の予算内でリスクが最小の経路の下界と、候補の経路（凸包の点）を返す。
// 骨組み：実装時に外す
#![allow(unused_variables)]

use crate::contract::{CancelToken, Interrupt};

use super::Path;
use super::ellipse::Subgraph;

/// 凸包の点（時間とリスクの組）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HullPoint {
    pub time_ds: u64,
    pub risk_fx: u64,
}

/// LARACの結果。
#[derive(Debug, Clone, PartialEq)]
pub struct LaracResult {
    /// 時間の予算内で見つかったリスク最小の経路。最短時間経路が予算を超える場合は `None`。
    pub best: Option<Path>,
    /// 予算内の最小リスクの下界（固定小数点）。
    pub lower_bound_fx: u64,
    /// 緩和の途中で得た (時間, リスク) の凸包の点。時間の昇順。
    pub hull: Vec<HullPoint>,
}

/// 部分グラフで、`sub.budget_ds` 以内の時間で最小のリスクの経路を近似する。
/// 反復は `iterations` 回まで。
pub fn larac(
    sub: &Subgraph,
    iterations: u32,
    cancel: &CancelToken,
) -> Result<LaracResult, Interrupt> {
    todo!("形式書18章：LARAC")
}
