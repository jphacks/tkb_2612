//! 部分グラフ上の探索の組み合わせ（behavior.md 5章 手順5〜7）と、ペナルティ法の代替経路
//! （同「候補の整理」1.）。候補の整理（`candidates::select_candidates`）の素材を作る。
//!
//! 1. リスク最小の経路 pc。時間が予算以下なら、pc が事故考慮経路（最適）。
//! 2. そうでなければ LARAC で、予算内の経路 pd と下界 lb、凸包の点を求める。pd のリスクが下界に
//!    等しければ（リスクの最適性が確認できれば）pd を事故考慮経路とする。
//! 3. そうでなければ、pd を暫定解、lb を下界として時間予算付きの制約付き最短路（RCSP）を行う。
//! 4. 事故考慮経路 best の辺のリスクを1.3倍・1.5倍にして、LARACの最後の重みで探索した代替経路
//!    （予算内、最大2本）。

use std::collections::HashSet;

use crate::contract::{CancelToken, Interrupt};

use super::Path;
use super::SearchContext;
use super::SearchStatus;
use super::ellipse::{NO_DEDGE, Subgraph};
use super::larac::{LaracResult, larac};
use super::rcsp::bounded_rcsp;
use super::subgraph_search::{Objective, shortest_path};

/// 部分グラフ上の探索の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct SubgraphSearch {
    /// 事故考慮経路。時間は `sub.budget_ds` 以下（予算内の経路がなければ最短時間経路で、`FastestOnly`）。
    pub best: Path,
    /// `Optimal`（最適性を確認）、`Bounded`（時間予算・ラベル数の上限で打ち切り）、`FastestOnly`。
    pub status: SearchStatus,
    /// 下界との相対差（`Optimal` なら0）。
    pub gap: f64,
    /// 候補の素材：凸包上の解のうち時間が予算以下のもの（時間の昇順）、ペナルティ法の代替経路の順。
    /// `best` と同じものを含むことがある。重複の除去と上限は `candidates::select_candidates`。
    pub alternatives: Vec<Path>,
}

/// 事故考慮経路の探索の結果。代替経路を集めるのに使う LARAC の結果も持つ。
#[derive(Debug, Clone, PartialEq)]
pub struct BestSearch {
    /// 事故考慮経路。時間は `sub.budget_ds` 以下（予算内の経路がなければ最短時間経路で、`FastestOnly`）。
    pub best: Path,
    /// `Optimal`（最適性を確認）、`Bounded`（時間予算・ラベル数の上限で打ち切り）、`FastestOnly`。
    pub status: SearchStatus,
    /// 下界との相対差（`Optimal` なら0）。
    pub gap: f64,
    /// この経路を求めた LARAC の結果（凸包の点と最後の重み）。
    pub larac: LaracResult,
}

/// 部分グラフで、予算内でリスク最小の経路（事故考慮経路）を探す。`search_subgraph` の手順1〜3。
///
/// LARAC で予算内の経路と下界を求め、リスクの最適性が確認できなければ RCSP で絞り込む。
/// 出発から到着へ到達できなければ `None`。
pub fn search_best(
    ctx: &SearchContext<'_>,
    sub: &Subgraph,
    cancel: &CancelToken,
) -> Result<Option<BestSearch>, Interrupt> {
    let larac = larac(ctx, sub, ctx.params.larac_iterations, cancel)?;
    if larac.min_risk.is_none() {
        return Ok(None);
    }
    let (best, status, gap) = match &larac.best {
        None => {
            // 予算内の経路がない（部分グラフの最短時間が予算を超える）。
            let Some(fastest) = larac.fastest.clone() else {
                return Ok(None);
            };
            (fastest, SearchStatus::FastestOnly, 0.0)
        }
        Some(pd) if larac.exact => (pd.clone(), SearchStatus::Optimal, 0.0),
        Some(_) => match bounded_rcsp(ctx, sub, &larac, cancel)? {
            Some(result) => (result.path, result.status, result.gap),
            None => return Ok(None),
        },
    };
    Ok(Some(BestSearch {
        best,
        status,
        gap,
        larac,
    }))
}

/// 部分グラフで、予算内でリスク最小の経路を探し、代替経路を集める。`p0` は最短時間経路
/// （`best` が `p0` と同じ有向辺の列ならペナルティ法の代替経路は求めない）。
/// 出発から到着へ到達できなければ `None`。
pub fn search_subgraph(
    ctx: &SearchContext<'_>,
    sub: &Subgraph,
    p0: &Path,
    cancel: &CancelToken,
) -> Result<Option<SubgraphSearch>, Interrupt> {
    let Some(found) = search_best(ctx, sub, cancel)? else {
        return Ok(None);
    };
    let BestSearch {
        best,
        status,
        gap,
        larac,
    } = found;

    let mut alternatives: Vec<Path> = larac
        .hull
        .iter()
        .filter(|p| p.time_ds <= sub.budget_ds)
        .cloned()
        .collect();
    if status != SearchStatus::FastestOnly && best.dedges != p0.dedges {
        alternatives.extend(penalty_alternatives(
            ctx,
            sub,
            &best,
            larac.weights,
            cancel,
        )?);
    }
    Ok(Some(SubgraphSearch {
        best,
        status,
        gap,
        alternatives,
    }))
}

/// ペナルティ法の代替経路（behavior.md 5章「候補の整理」1.）：`best` の辺（両方向）のリスクを1.3倍、
/// 1.5倍にした部分グラフで、`weights`（LARACの最後の重み `(alpha, beta)`。`None` なら
/// リスクだけ）で「`alpha`×時間 + `beta`×リスク」が最小の経路を求め、時間が予算以下で、`best` と
/// すでに得たものとは違う経路を最大2本返す。返す経路の時間とリスクは、ペナルティを掛ける前の値。
pub fn penalty_alternatives(
    ctx: &SearchContext<'_>,
    sub: &Subgraph,
    best: &Path,
    weights: Option<(u64, u64)>,
    cancel: &CancelToken,
) -> Result<Vec<Path>, Interrupt> {
    let (alpha, beta) = weights.unwrap_or((0, 1));
    let on_best: HashSet<u32> = best.dedges.iter().map(|d| d.edge()).collect();
    let mut found: Vec<Path> = Vec::new();
    // 1.3倍と1.5倍（十分の一の単位の整数で、四捨五入）。
    for tenths in [13_u128, 15] {
        cancel.check()?;
        let risk: Vec<u64> = sub
            .arcs
            .iter()
            .map(|a| {
                if a.dedge != NO_DEDGE && on_best.contains(&a.dedge.edge()) {
                    u64::try_from((u128::from(a.risk_fx) * tenths + 5) / 10).unwrap_or(u64::MAX)
                } else {
                    a.risk_fx
                }
            })
            .collect();
        let objective = Objective::Weighted {
            alpha,
            beta,
            risk: Some(&risk),
        };
        let Some(path) = shortest_path(ctx, sub, objective, cancel)? else {
            continue;
        };
        if path.time_ds > sub.budget_ds
            || path.dedges == best.dedges
            || found.iter().any(|p| p.dedges == path.dedges)
        {
            continue;
        }
        found.push(path);
        if found.len() == 2 {
            break;
        }
    }
    Ok(found)
}
