//! 探索範囲が大きすぎる場合（計画書6.4節）：楕円を縮小した部分グラフを作る。
// 骨組み：実装時に外す
#![allow(unused_variables)]

use crate::contract::CancelToken;
use crate::domain::routing::snap::SnapPoint;

use super::ArcCost;
use super::SearchContext;
use super::ellipse::{EllipseError, Subgraph};

/// `ellipse_subgraph` が `TooLarge` になったときに、最短時間経路の周辺に絞った部分グラフを作る。
/// 結果の状態は `SearchStatus::Reduced`（呼び出し側が付ける）。
pub fn reduced_ellipse(
    ctx: &SearchContext<'_>,
    start: &SnapPoint,
    end: &SnapPoint,
    fastest_ds: u64,
    detour_ds: u64,
    cost: &mut dyn ArcCost,
    cancel: &CancelToken,
) -> Result<Subgraph, EllipseError> {
    todo!("計画書6.4節：探索範囲の縮小")
}
