//! 最短時間経路（形式書18章）：ノード単位の双方向A*。直前の辺を状態に持つ探索（右左折禁止と
//! 右左折コストのため）。下界は `Landmarks::lower_bound_ds`。
// 骨組み：実装時に外す
#![allow(unused_variables)]

use crate::contract::{CancelToken, Interrupt};
use crate::domain::routing::snap::SnapPoint;

use super::Path;
use super::SearchContext;

/// 出発から到着までの最短時間経路。到達できなければ `None`。
/// 返す `Path` の `risk_fx` は0（リスクは別に計算する）。
pub fn fastest_path(
    ctx: &SearchContext<'_>,
    start: &SnapPoint,
    end: &SnapPoint,
    cancel: &CancelToken,
) -> Result<Option<Path>, Interrupt> {
    todo!("形式書18章：最短時間経路")
}
