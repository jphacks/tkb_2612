//! 探索の結合（behavior.md 5章 手順2〜8）：吸着後の最短時間経路から、楕円・LARAC・RCSP・ペナルティ法
//! （`pipeline::search_subgraph`）を通し、候補の整理までを1回で行う。
//!
//! リスクは [`ArcCost`] で外から受け取る（実装は `RiskView` を包むアダプタ。M3bの別タスク）。
//! 探索の失敗は [`RouteError`]（behavior.md 5章「エラー」）、段階ごとの計測は [`RouteStats`] に残す。
//! 利用者への警告は出さない（縮小・最短時間経路だけの場合も同様）。

use std::time::Instant;

use crate::contract::{CancelToken, Interrupt};
use crate::domain::routing::graph::Graph;
use crate::domain::routing::snap::SnapPoint;

use super::ArcCost;
use super::Path;
use super::SearchContext;
use super::SearchStatus;
use super::candidates::{MAX_ROUTE_DISTANCE_DM, select_candidates};
use super::ellipse::{EllipseError, ellipse_subgraph};
use super::endpoints::{Placement, is_same_position, path_distance_dm, placement};
use super::fastest::fastest_path;
use super::pipeline::search_subgraph;
use super::reduced::reduced_ellipse;

/// 探索の失敗（behavior.md 5章「エラー」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteError {
    /// 2点が同じ位置、最短時間経路がない。
    NoRoute,
    /// 最短時間経路の距離が80,000m超。
    RouteTooLong,
    /// 取り消し・期限切れ。
    Interrupted,
}

impl From<Interrupt> for RouteError {
    fn from(_value: Interrupt) -> Self {
        RouteError::Interrupted
    }
}

/// 段階ごとの記録（計測と内部ログ用。利用者には出さない）。
#[derive(Debug, Clone)]
pub struct RouteStats {
    /// 最短時間経路（吸着点の検査と距離の検査を含む）。
    pub fastest_ms: f64,
    /// 楕円の切り出し（縮小のやり直しを含む）。
    pub ellipse_ms: f64,
    /// `search_subgraph`（LARAC・RCSP・ペナルティ法）。
    pub search_ms: f64,
    /// 候補の整理。
    pub select_ms: f64,
    /// 使った部分グラフのノード数（仮想ノードを含む。作れなかったら `None`）。
    pub ellipse_nodes: Option<usize>,
    /// 使った部分グラフの有向辺の数（作れなかったら `None`）。
    pub ellipse_arcs: Option<usize>,
    /// 6.4節の1.（縮小）に入った。
    pub reduced: bool,
    /// 6.4節の2.（縮小でも大きすぎる）か、部分グラフで経路が見つからず p0 だけにした。
    pub fastest_only: bool,
    /// `best` の状態。縮小した場合でも `search_subgraph` の状態（Optimal/Bounded/FastestOnly）を
    /// そのまま入れ、縮小は `reduced` で表す。p0 だけにした場合は `FastestOnly`。
    pub status: SearchStatus,
    /// 下界との相対差（`Optimal` なら0）。
    pub gap: f64,
    /// `status == Optimal`。
    pub rcsp_completed: bool,
}

/// 結合した探索の結果。
#[derive(Debug, Clone)]
pub struct RouteSearch {
    /// 最短時間経路（`risk_fx` を計算済み）。
    pub p0: Path,
    /// 事故考慮経路（p0 だけの場合は p0 と同じ）。
    pub best: Path,
    /// 時間の上限 B（デシ秒）。
    pub budget_ds: u64,
    /// 候補（`select_candidates` の結果。所要時間の昇順、最大6本、すべて `time_ds ≤ B`）。
    pub candidates: Vec<Path>,
    pub stats: RouteStats,
}

/// 経過時間（ミリ秒）。
fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

/// 確定した経路のリスク（固定小数点）を、[`ArcCost`] の規則で足し合わせる。
///
/// 出発の辺は `partial_risk_fx + cluster_entry_risk_fx`、途中の辺は `risk_fx + cluster_entry_risk_fx`、
/// 到着の辺は `partial_risk_fx` だけ、1本だけなら2点のあいだの `partial_risk_fx`。吸着点が辺の端の
/// ノードにあるときは、その辺を全長で通る通常の辺として扱う（その辺を通らないこともある）。
/// 部分グラフの組み立て（`ellipse::assemble`）と同じ値になる。
pub fn path_risk_fx(graph: &Graph, cost: &mut dyn ArcCost, path: &Path) -> u64 {
    let n = path.dedges.len();
    debug_assert!(n > 0, "経路の有向辺が空");
    let start_inside = placement(graph, &path.start) == Some(Placement::Inside);
    let end_inside = placement(graph, &path.end) == Some(Placement::Inside);
    let mut total = 0_u64;
    for (i, &d) in path.dedges.iter().enumerate() {
        let length = graph.edge(d.edge()).length_dm;
        // 向きによらず辺の始点からの距離で表した、通る区間。
        let (mut lo, mut hi) = (0, length);
        let start_partial = i == 0 && start_inside && d.edge() == path.start.edge;
        let end_partial = i + 1 == n && end_inside && d.edge() == path.end.edge;
        if start_partial {
            let offset = path.start.offset_dm.min(length);
            if d.is_reverse() {
                hi = offset;
            } else {
                lo = offset;
            }
        }
        if end_partial {
            let offset = path.end.offset_dm.min(length);
            if d.is_reverse() {
                lo = lo.max(offset);
            } else {
                hi = hi.min(offset);
            }
        }
        let edge_risk = if lo == 0 && hi == length {
            cost.risk_fx(d)
        } else {
            // 形が正しくない経路でも、負の区間を作らずに0を返す。
            if hi <= lo {
                0
            } else {
                cost.partial_risk_fx(d, lo, hi)
            }
        };
        total = total.saturating_add(edge_risk);
        // 到着の辺の手前まで（到着の辺では、到着ノードのクラスタに入らない）。
        if !end_partial {
            total = total.saturating_add(cost.cluster_entry_risk_fx(d));
        }
    }
    total
}

/// behavior.md 5章 手順2〜8を1回で行う。`max_detour_min` は遠回りの上限（分）。
///
/// 1. 2点が同じ位置なら [`RouteError::NoRoute`]。
/// 2. 最短時間経路 p0 を求める。到達できなければ [`RouteError::NoRoute`]、距離が80,000m超なら
///    [`RouteError::RouteTooLong`]。
/// 3. p0 のリスクを [`path_risk_fx`] で計算する。
/// 4. `B = T0 + 600 × max_detour_min`。
/// 5. 楕円の部分グラフを作る。大きすぎれば縮小し、それでも大きすぎる（または到達できない）なら
///    p0 だけにする。
/// 6. 部分グラフがあれば [`search_subgraph`] で事故考慮経路と代替経路を求める。見つからなければ
///    p0 だけにする。
/// 7. [`select_candidates`] で候補を整理する。
/// 8. すべての候補が `time_ds ≤ B` であることを `debug_assert!` で確かめる。
pub fn find_routes(
    ctx: &SearchContext<'_>,
    cost: &mut dyn ArcCost,
    start: &SnapPoint,
    end: &SnapPoint,
    max_detour_min: u32,
    cancel: &CancelToken,
) -> Result<RouteSearch, RouteError> {
    let graph = ctx.graph;
    if is_same_position(graph, start, end) {
        return Err(RouteError::NoRoute);
    }

    // 手順2・3：最短時間経路と、そのリスク。
    let fastest_start = Instant::now();
    let mut p0 = fastest_path(ctx, start, end, cancel)?.ok_or(RouteError::NoRoute)?;
    let distance_dm = path_distance_dm(graph, start, end, &p0.dedges).ok_or(RouteError::NoRoute)?;
    if distance_dm > MAX_ROUTE_DISTANCE_DM {
        return Err(RouteError::RouteTooLong);
    }
    p0.risk_fx = path_risk_fx(graph, cost, &p0);
    let fastest_ms = elapsed_ms(fastest_start);
    cancel.check()?;

    // 手順4：時間の上限。
    let detour_ds = 600_u64.saturating_mul(u64::from(max_detour_min));
    let budget_ds = p0.time_ds.saturating_add(detour_ds);

    // 手順5：楕円の切り出しと縮小。
    let ellipse_start = Instant::now();
    let mut reduced = false;
    let mut fastest_only = false;
    let sub = match ellipse_subgraph(ctx, start, end, p0.time_ds, detour_ds, cost, cancel) {
        Ok(sub) => Some(sub),
        Err(EllipseError::TooLarge { .. }) => {
            reduced = true;
            match reduced_ellipse(ctx, start, end, p0.time_ds, detour_ds, cost, cancel) {
                Ok(sub) => Some(sub),
                Err(EllipseError::TooLarge { .. } | EllipseError::Unreachable) => {
                    fastest_only = true;
                    None
                }
                Err(EllipseError::Interrupted(i)) => return Err(RouteError::from(i)),
            }
        }
        Err(EllipseError::Unreachable) => {
            // p0 が見つかっているので本来起こらない。p0 だけにする。
            fastest_only = true;
            None
        }
        Err(EllipseError::Interrupted(i)) => return Err(RouteError::from(i)),
    };
    let ellipse_ms = elapsed_ms(ellipse_start);
    cancel.check()?;
    let ellipse_nodes = sub.as_ref().map(|s| s.node_count());
    let ellipse_arcs = sub.as_ref().map(|s| s.arcs.len());

    // 手順6：部分グラフ上の探索。
    let search_start = Instant::now();
    let mut best = p0.clone();
    let mut status = SearchStatus::FastestOnly;
    let mut gap = 0.0;
    let mut alternatives = Vec::new();
    if let Some(sub) = &sub {
        match search_subgraph(ctx, sub, &p0, cancel)? {
            Some(found) => {
                best = found.best;
                status = found.status;
                gap = found.gap;
                alternatives = found.alternatives;
            }
            None => {
                fastest_only = true;
                best = p0.clone();
            }
        }
    }
    let search_ms = elapsed_ms(search_start);
    cancel.check()?;

    // 手順7：候補の整理。p0 だけにする場合は、p0 と同じ経路を1本だけ残す。
    let select_start = Instant::now();
    let candidates = if fastest_only {
        select_candidates(
            graph,
            ctx.topology,
            p0.clone(),
            p0.clone(),
            Vec::new(),
            budget_ds,
        )
    } else {
        select_candidates(
            graph,
            ctx.topology,
            p0.clone(),
            best.clone(),
            alternatives,
            budget_ds,
        )
    };
    let select_ms = elapsed_ms(select_start);

    // 手順8：候補の上限。合法性は `SearchContext` が `Restrictions` を持たないため、ここでは
    // `legality::check_path` を呼べない（結合試験と、③の検査で確かめる）。
    for candidate in &candidates {
        debug_assert!(
            candidate.time_ds <= budget_ds,
            "候補の時間が上限を超えた: {} > {budget_ds}",
            candidate.time_ds
        );
    }

    Ok(RouteSearch {
        p0,
        best,
        budget_ds,
        candidates,
        stats: RouteStats {
            fastest_ms,
            ellipse_ms,
            search_ms,
            select_ms,
            ellipse_nodes,
            ellipse_arcs,
            reduced,
            fastest_only,
            status,
            gap,
            rcsp_completed: status == SearchStatus::Optimal,
        },
    })
}
