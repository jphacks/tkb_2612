//! 制約付き最短路（形式書18章）：LARACの結果を初期解・下界にして、時間の予算の中で厳密に近づける。
// 骨組み：実装時に外す
#![allow(unused_variables)]

use crate::contract::{CancelToken, Interrupt};

use super::Path;
use super::SearchContext;
use super::SearchStatus;
use super::ellipse::Subgraph;
use super::larac::LaracResult;

/// 制約付き最短路の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct RcspResult {
    /// 見つかった経路（予算内の最小リスク、見つからなければ最短時間経路）。
    pub path: Path,
    /// `Optimal`（最適性を確認）または `Bounded`（時間予算で打ち切り）など。
    pub status: SearchStatus,
    /// 下界との相対差（0以上。`Optimal` なら0）。
    pub gap: f64,
}

/// 部分グラフで、`ctx.params.rcsp_budget_ms` の間、予算内の最小リスクの経路を探す。
pub fn bounded_rcsp(
    ctx: &SearchContext<'_>,
    sub: &Subgraph,
    larac: &LaracResult,
    cancel: &CancelToken,
) -> Result<RcspResult, Interrupt> {
    todo!("形式書18章：制約付き最短路")
}
