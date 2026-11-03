//! 時間予算付きの制約付き最短路（形式書18章、behavior.md 5章 手順7）：LARACの結果を初期解・下界にして、
//! 時間の予算の中で厳密に近づける。
//!
//! 部分グラフ上で、時間が予算以下の経路のうち (リスク, 時間) の辞書順で最小のものを探す、多ラベルの
//! 探索。ラベルは (時間, リスク) の組で、**状態は「局所ノード、直前の有向辺、照合器の状態」**。
//! 優越判定（時間もリスクも以下のラベルがあれば捨てる）は、同じ状態のラベルどうしだけで行う。右左折
//! コストと右左折禁止は直前の有向辺と照合器の状態に依存するので、これらが違うラベルは比べない。
//!
//! 取り出す順は、リスクの下界（ラベルのリスク＋到着までのリスクの最小）、同じなら時間の下界の昇順
//! （A*）。下界は、部分グラフの辺だけを使い、ノードの加算・右左折コスト・禁止・通過不可を無視した
//! 到着までの最小値で、実際の値以下なので、無矛盾な下界になる。したがって、到着の状態のラベルを最初に
//! 取り出した時点で、そのラベルが (リスク, 時間) の辞書順で最小である（`Optimal`）。
//!
//! 捨てるラベル：時間（下界を足して）が予算を超えるもの、暫定解（初期値はLARACの解）の
//! (リスク, 時間) を辞書順で改善できないもの（リスクの下界が暫定解のリスクを超える、または等しくて
//! 時間の下界が暫定解の時間以上）、同じ状態の既存のラベルに優越されるもの。
//!
//! 打ち切り：`SearchParams::rcsp_budget_ms` の経過、ラベル数が `rcsp_label_limit` に達した場合は、
//! その時点の暫定解を返し、`Optimal` とは記録しない（`Bounded`）。中断は `Interrupt`。
//!
//! 周回：状態付きの探索なので、右左折禁止を避ける周回（同じノードを違う直前の辺や照合器の状態で
//! 2回通る歩道）を許す。同じ状態を2回通るラベルは、時間が正で増えるため、最初に着いたラベルに
//! 優越されて捨てられる。時間は予算以下で、同じ状態のラベルは時間の異なる有限個なので、探索は
//! 必ず有限で終わる。

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::{Duration, Instant};

use crate::contract::{CancelToken, Interrupt};

use super::ellipse::Subgraph;
use super::larac::LaracResult;
use super::subgraph_search::{
    CANCEL_CHECK_INTERVAL, INF, Objective, SOURCE_STATE, StateSpace, TERMINAL_STATE,
    costs_to_target, path_dedges, shortest_path,
};
use super::{Path, SearchContext, SearchStatus};

/// 制約付き最短路の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct RcspResult {
    /// 見つかった経路（予算内の最小リスク）。予算内の経路がなければ最短時間経路（`FastestOnly`）。
    pub path: Path,
    /// `Optimal`（最適性を確認）、`Bounded`（時間予算・ラベル数の上限で打ち切り）、
    /// `FastestOnly`（予算内の経路がない）。
    pub status: SearchStatus,
    /// 下界との相対差 `(リスク − 下界) / リスク`（0以上。`Optimal` なら0。リスクが0なら0）。
    pub gap: f64,
}

#[derive(Clone, Copy)]
struct Label {
    time: u64,
    risk: u64,
    state: u32,
    parent: u32,
    arc: u32,
    dead: bool,
}

const NO_LABEL: u32 = u32::MAX;

/// 状態ごとの非劣解（時間の昇順、リスクは狭義の降順）に、末尾に積んだラベル `new_index` を入れる。
/// 既存のラベルに優越されるなら入れずに `false`（呼び出し側がラベルを取り除く）。入れるときは、
/// 新しいラベルに優越される既存のラベルを取り除き（`dead` にし）、`true`。同じ (時間, リスク) は
/// 先に入ったものを残す。
fn insert_label(frontier: &mut Vec<u32>, labels: &mut [Label], new_index: u32) -> bool {
    let (time, risk) = (
        labels[new_index as usize].time,
        labels[new_index as usize].risk,
    );
    let after_equal = frontier.partition_point(|&i| labels[i as usize].time <= time);
    if after_equal > 0 && labels[frontier[after_equal - 1] as usize].risk <= risk {
        return false;
    }
    // 時間が time 以上で、リスクが risk 以上のものは、新しいラベルに優越される。
    let first = frontier.partition_point(|&i| labels[i as usize].time < time);
    let mut last = first;
    while last < frontier.len() && labels[frontier[last] as usize].risk >= risk {
        labels[frontier[last] as usize].dead = true;
        last += 1;
    }
    frontier.splice(first..last, [new_index]);
    true
}

/// 部分グラフで、`ctx.params.rcsp_budget_ms` の間、予算内の最小リスクの経路を探す。
///
/// 出発から到着へ到達できなければ `None`。
pub fn bounded_rcsp(
    ctx: &SearchContext<'_>,
    sub: &Subgraph,
    larac: &LaracResult,
    cancel: &CancelToken,
) -> Result<Option<RcspResult>, Interrupt> {
    cancel.check()?;
    let started = Instant::now();
    let time_budget = Duration::from_millis(ctx.params.rcsp_budget_ms);
    let label_limit = ctx.params.rcsp_label_limit as usize;
    let bound = sub.budget_ds;

    let fallback = |cancel: &CancelToken| -> Result<Option<RcspResult>, Interrupt> {
        let path = match &larac.fastest {
            Some(p) => Some(p.clone()),
            None => shortest_path(ctx, sub, Objective::TimeThenRisk, cancel)?,
        };
        Ok(path.map(|path| RcspResult {
            path,
            status: SearchStatus::FastestOnly,
            gap: 0.0,
        }))
    };

    let to_time = costs_to_target(sub, |a| u64::from(a.time_ds), cancel)?;
    let to_risk = costs_to_target(sub, |a| a.risk_fx, cancel)?;
    let source_node = sub.source as usize;
    if to_time[source_node] == INF {
        return Ok(None);
    }
    if to_time[source_node] > bound {
        return fallback(cancel);
    }

    // 暫定解（予算内のもの）。
    let mut incumbent: Option<Path> = larac.best.clone().filter(|p| p.time_ds <= bound);
    let mut lower_bound = larac.lower_bound_fx;

    let mut space = StateSpace::new(ctx, sub);
    let mut labels: Vec<Label> = Vec::new();
    let mut frontiers: Vec<Vec<u32>> = vec![Vec::new(); space.state_count()];
    let mut heap: BinaryHeap<Reverse<(u64, u64, u32)>> = BinaryHeap::new();
    labels.push(Label {
        time: 0,
        risk: 0,
        state: SOURCE_STATE,
        parent: NO_LABEL,
        arc: NO_LABEL,
        dead: false,
    });
    frontiers[SOURCE_STATE as usize].push(0);
    heap.push(Reverse((to_risk[source_node], to_time[source_node], 0)));

    let mut stopped_early = false;
    let mut found: Option<u32> = None;
    let mut pops: u32 = 0;
    while !heap.is_empty() {
        // 時間予算と中断は、最初と一定回数ごとに調べる。ラベル数の上限は毎回調べる。
        if pops & (CANCEL_CHECK_INTERVAL - 1) == 0 {
            cancel.check()?;
            if started.elapsed() >= time_budget {
                stopped_early = true;
                break;
            }
        }
        if labels.len() >= label_limit {
            stopped_early = true;
            break;
        }
        pops = pops.wrapping_add(1);
        let Some(Reverse((_, _, index))) = heap.pop() else {
            break;
        };
        let label = labels[index as usize];
        if label.dead {
            continue;
        }
        if label.state == TERMINAL_STATE {
            found = Some(index);
            break;
        }
        let range = space.transitions(label.state);
        if frontiers.len() < space.state_count() {
            frontiers.resize(space.state_count(), Vec::new());
        }
        for k in range {
            let tr = space.trans_at(k);
            let arc = &sub.arcs[tr.arc as usize];
            let time = label.time + u64::from(tr.extra_ds) + u64::from(arc.time_ds);
            if time > bound {
                continue;
            }
            let risk = label.risk + arc.risk_fx;
            let node = space.node_of(tr.to) as usize;
            if to_time[node] == INF || to_risk[node] == INF {
                continue;
            }
            let f_time = time.saturating_add(to_time[node]);
            if f_time > bound {
                continue;
            }
            let f_risk = risk.saturating_add(to_risk[node]);
            if let Some(inc) = &incumbent
                && (f_risk > inc.risk_fx || (f_risk == inc.risk_fx && f_time >= inc.time_ds))
            {
                continue;
            }
            let new_index = labels.len() as u32;
            labels.push(Label {
                time,
                risk,
                state: tr.to,
                parent: index,
                arc: tr.arc,
                dead: false,
            });
            if insert_label(&mut frontiers[tr.to as usize], &mut labels, new_index) {
                heap.push(Reverse((f_risk, f_time, new_index)));
            } else {
                labels.pop();
            }
        }
    }

    let status;
    let path;
    if let Some(index) = found {
        let label = labels[index as usize];
        let mut arcs = Vec::new();
        let mut i = index;
        while labels[i as usize].parent != NO_LABEL {
            arcs.push(labels[i as usize].arc);
            i = labels[i as usize].parent;
        }
        arcs.reverse();
        path = Path {
            dedges: path_dedges(sub, arcs.into_iter()),
            start: sub.start,
            end: sub.end,
            time_ds: label.time,
            risk_fx: label.risk,
        };
        status = SearchStatus::Optimal;
        // 暫定解より悪いラベルは捨てているので、見つけたラベルは暫定解より厳密に良い。
    } else if let Some(inc) = incumbent.take() {
        if stopped_early {
            // 残っているラベルのリスクの下界の最小が、最適リスクの下界になる。
            let open_min = heap
                .iter()
                .filter(|Reverse((_, _, i))| !labels[*i as usize].dead)
                .map(|Reverse((f_risk, _, _))| *f_risk)
                .min();
            if let Some(open_min) = open_min {
                lower_bound = lower_bound.max(open_min.min(inc.risk_fx));
            }
            status = SearchStatus::Bounded;
        } else {
            // すべて探索して、暫定解より良いものがなかった。
            status = SearchStatus::Optimal;
            lower_bound = inc.risk_fx;
        }
        path = inc;
    } else {
        return fallback(cancel);
    }
    let gap = if status == SearchStatus::Optimal || path.risk_fx == 0 {
        0.0
    } else {
        (path.risk_fx.saturating_sub(lower_bound)) as f64 / path.risk_fx as f64
    };
    Ok(Some(RcspResult { path, status, gap }))
}
