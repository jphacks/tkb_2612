//! LARAC（ラグランジュ緩和による制約付き最短路の近似、形式書18章、behavior.md 5章 手順6）。
//! 時間の予算内でリスクが最小の経路の下界と、候補の経路（凸包の点）を返す。
//!
//! 時間 `t`・リスク `r` の経路について、「`alpha`×時間 + `beta`×リスク」（動かす量は `alpha`・`beta` の
//! 整数の組。`behavior.md` の「時間＋λ×リスク」の λ は `beta / alpha`）を最小にする経路を、
//! 状態付きのダイクストラ法で求める。すべて整数で計算するので、丸めの誤差はない。
//!
//! 1. 時間最小の経路 `pd`（(時間, リスク) の辞書順最小）と、リスク最小の経路 `pc`（(リスク, 時間) の
//!    辞書順最小）を求める。`pd` が予算を超えるなら解なし。`pc` が予算内なら `pc` が最適。
//! 2. 予算内の最良の経路 `pd` と、予算を超える経路 `pc` を結ぶ直線の法線方向
//!    （`alpha = pd.リスク − pc.リスク`、`beta = pc.時間 − pd.時間`）で重み付き最小の経路 `r` を求める。
//!    `r` が直線より厳密に下にあれば、`r` の時間が予算内なら `pd`、超えるなら `pc` を `r` に替えて繰り返す。
//!    直線上なら収束（これ以上凸包の点はない）。
//! 3. 各回の重み付きの最小値 `L*` から、最適リスクの下界 `ceil((L* − alpha×予算) / beta)` が得られる
//!    （予算内のどの経路 `x` も `alpha×x.時間 + beta×x.リスク ≥ L*` かつ `x.時間 ≤ 予算`）。
//!    リスク最小の経路 `pc` のリスクも下界。

use crate::contract::{CancelToken, Interrupt};

use super::ellipse::Subgraph;
use super::subgraph_search::{Objective, shortest_path};
use super::{Path, SearchContext};

/// LARACの結果。
#[derive(Debug, Clone, PartialEq)]
pub struct LaracResult {
    /// 時間最小（同じならリスク最小）の経路。出発から到着へ到達できないとき、またはリスク最小の経路が
    /// 予算内で最適と分かって計算しなかったときは `None`。
    pub fastest: Option<Path>,
    /// リスク最小（同じなら時間最小）の経路。到達できなければ `None`。
    pub min_risk: Option<Path>,
    /// 時間の予算内で見つかったリスク最小の経路。最短時間経路が予算を超える場合は `None`。
    pub best: Option<Path>,
    /// 予算内の最小リスクの下界（固定小数点）。`best` が `None` のときは意味がない。
    pub lower_bound_fx: u64,
    /// `best` のリスクが下界と等しい（リスクの最適性が確認できた）。時間の最小までは保証しない。
    pub exact: bool,
    /// 緩和の途中で得た (時間, リスク) の凸包の点の経路。時間の昇順。リスク最小の経路と（計算したときは）
    /// 最短時間経路を含み、予算を超えるものも含む。
    pub hull: Vec<Path>,
    /// 最後に使った重み `(alpha, beta)`（互いに素）。ペナルティ法の代替経路が使う。LARACの反復に
    /// 入らなかったときは `None`。
    pub weights: Option<(u64, u64)>,
    /// 重み付きの探索を行った回数。
    pub iterations: u32,
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// 部分グラフで、`sub.budget_ds` 以内の時間で最小のリスクの経路を近似する。
/// 反復は `iterations` 回まで。
pub fn larac(
    ctx: &SearchContext<'_>,
    sub: &Subgraph,
    iterations: u32,
    cancel: &CancelToken,
) -> Result<LaracResult, Interrupt> {
    let mut result = LaracResult {
        fastest: None,
        min_risk: None,
        best: None,
        lower_bound_fx: 0,
        exact: false,
        hull: Vec::new(),
        weights: None,
        iterations: 0,
    };
    let Some(min_risk) = shortest_path(ctx, sub, Objective::RiskThenTime, cancel)? else {
        return Ok(result);
    };
    let budget = sub.budget_ds;
    result.min_risk = Some(min_risk.clone());
    result.lower_bound_fx = min_risk.risk_fx;
    if min_risk.time_ds <= budget {
        // リスク最小の経路が予算内なら、それが最適（(リスク, 時間) の辞書順でも最小）。
        result.best = Some(min_risk.clone());
        result.exact = true;
        result.hull = vec![min_risk];
        return Ok(result);
    }
    let Some(fastest) = shortest_path(ctx, sub, Objective::TimeThenRisk, cancel)? else {
        return Ok(result);
    };
    result.fastest = Some(fastest.clone());
    if fastest.time_ds > budget {
        result.hull = sorted_hull(vec![fastest, min_risk]);
        return Ok(result);
    }

    let mut pd = fastest;
    let mut pc = min_risk;
    let mut hull = vec![pd.clone(), pc.clone()];
    for _ in 0..iterations {
        cancel.check()?;
        // 不変条件：pd は予算内、pc は予算超え、pc のリスクは pd のリスクより小さい。
        let (Some(alpha), Some(beta)) = (
            pd.risk_fx.checked_sub(pc.risk_fx),
            pc.time_ds.checked_sub(pd.time_ds),
        ) else {
            break;
        };
        if alpha == 0 || beta == 0 {
            break;
        }
        let g = gcd(alpha, beta);
        let (alpha, beta) = (alpha / g, beta / g);
        result.weights = Some((alpha, beta));
        let Some(r) = shortest_path(
            ctx,
            sub,
            Objective::Weighted {
                alpha,
                beta,
                risk: None,
            },
            cancel,
        )?
        else {
            break;
        };
        result.iterations += 1;
        let weigh = |p: &Path| {
            u128::from(alpha) * u128::from(p.time_ds) + u128::from(beta) * u128::from(p.risk_fx)
        };
        let l_star = weigh(&r);
        let l_line = weigh(&pc);
        // 双対の下界：予算内のどの経路 x も alpha×x.時間 + beta×x.リスク ≥ l_star、x.時間 ≤ 予算。
        let slack = u128::from(alpha) * u128::from(budget);
        if l_star > slack {
            let bound = (l_star - slack).div_ceil(u128::from(beta));
            result.lower_bound_fx = result
                .lower_bound_fx
                .max(u64::try_from(bound).unwrap_or(u64::MAX));
        }
        if l_star >= l_line {
            break;
        }
        hull.push(r.clone());
        if r.time_ds <= budget {
            pd = r;
        } else {
            pc = r;
        }
    }
    result.exact = result.lower_bound_fx >= pd.risk_fx;
    result.best = Some(pd);
    result.hull = sorted_hull(hull);
    Ok(result)
}

/// 時間の昇順（同じ時間ならリスクの昇順、同じ経路は1つ）に並べる。
fn sorted_hull(mut paths: Vec<Path>) -> Vec<Path> {
    paths.sort_by_key(|p| (p.time_ds, p.risk_fx));
    paths.dedup_by(|a, b| a.dedges == b.dedges);
    paths
}
