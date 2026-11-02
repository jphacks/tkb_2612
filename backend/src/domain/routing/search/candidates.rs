//! 候補の整理（形式書18章、behavior.md 5章）：重複除去・上限6本・所要時間の昇順。
// 骨組み：実装時に外す
#![allow(unused_variables)]

use crate::domain::routing::graph::Graph;

use super::Path;

/// 重複とみなす共有の割合（短い方の長さに対して80%以上を共有）。
pub const OVERLAP_THRESHOLD: f64 = 0.8;

/// 2本の経路が共有する長さの割合（短い方の長さに対する、同じ辺を通る長さ）。
pub fn overlap_ratio(graph: &Graph, a: &Path, b: &Path) -> f64 {
    todo!("共有する辺の長さの割合")
}

/// 経路の集まりから、80%以上重なるものを除き、所要時間の昇順に並べて、上限
/// （`contract::engine::MAX_CANDIDATES` = 6本）までにする。同じ所要時間では入力順を保つ。
pub fn select_candidates(graph: &Graph, paths: Vec<Path>) -> Vec<Path> {
    todo!("候補の整理")
}
