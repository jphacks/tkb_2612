//! 探索範囲が大きすぎる場合（計画書6.4節、behavior.md 5章「探索範囲が大きすぎる場合」）：楕円を縮小した
//! 部分グラフを作る。
//!
//! 楕円の確定ノード数が上限を超えたら、出発地・目的地から直線距離 [`REDUCED_RADIUS_M`] 以内の辺と、
//! `class` が tertiary_link 以上（形式書6.5節の値が [`REDUCED_MAX_CLASS`] 以下）の辺だけで、楕円の切り出しを
//! やり直す。それでも上限を超えたら、呼び出し側が候補を最短時間経路だけにする（`SearchStatus::FastestOnly`）。
//!
//! 「出発地・目的地から3km以内の辺」は、辺の始点か終点のノードが、出発・到着の吸着点から直線距離（`contract::geo`
//! の局所近似）で3km以内にある辺とする（辺は交差点で分けてあり、長い辺は少ないため。折れ線の途中だけが
//! 近い辺は数えない）。縮小した部分グラフの `fastest_ds`・`budget_ds` は楕円と同じ（全体グラフでの最短時間と
//! 上限）。縮小で最短時間経路が切れたときは、部分グラフの最短時間が予算を超えることがある。

use crate::contract::CancelToken;
use crate::contract::geo::straight_distance_m;
use crate::domain::routing::graph::{Coord, DirEdge, NodeId};
use crate::domain::routing::snap::{SnapPoint, point_on_edge};

use super::ArcCost;
use super::SearchContext;
use super::ellipse::{EllipseError, Subgraph, cut};

/// 出発地・目的地の周辺で、すべての辺を残す半径（m）。
pub const REDUCED_RADIUS_M: f64 = 3000.0;
/// 周辺の外で残す辺の `class` の上限（tertiary_link = 9）。
pub const REDUCED_MAX_CLASS: u8 = 9;

/// `ellipse_subgraph` が `TooLarge` になったときに、最短時間経路の周辺に絞った部分グラフを作る。
/// 結果の状態は `SearchStatus::Reduced`（呼び出し側が付ける）。まだ `TooLarge` なら、呼び出し側が
/// 候補を最短時間経路だけにする。
pub fn reduced_ellipse(
    ctx: &SearchContext<'_>,
    start: &SnapPoint,
    end: &SnapPoint,
    fastest_ds: u64,
    detour_ds: u64,
    cost: &mut dyn ArcCost,
    cancel: &CancelToken,
) -> Result<Subgraph, EllipseError> {
    let graph = ctx.graph;
    let centers: [Coord; 2] = [
        point_on_edge(graph, start.edge, start.offset_dm),
        point_on_edge(graph, end.edge, end.offset_dm),
    ];
    let near = |node: NodeId| {
        let at = graph.node_coord(node).to_latlon();
        centers
            .iter()
            .any(|c| straight_distance_m(at, c.to_latlon()) <= REDUCED_RADIUS_M)
    };
    let allow = |d: DirEdge| {
        graph.edge(d.edge()).class <= REDUCED_MAX_CLASS
            || near(graph.dedge_from(d))
            || near(graph.dedge_to(d))
    };
    cut(ctx, start, end, fastest_ds, detour_ds, cost, cancel, &allow)
}
