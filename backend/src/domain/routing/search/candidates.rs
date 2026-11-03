//! 候補の整理（形式書18章、behavior.md 5章「候補の整理」2〜5）：上限の確認・重複除去・上限6本・
//! 所要時間の昇順。
//!
//! 重複の判定は「共有する物理区間の長さ」。物理区間は辺番号で判定し、双子の辺（`Topology::twin_of`）
//! は同じ区間とみなし、出発・到着の辺の途中までの部分は重なった長さだけを数える。

use crate::contract::engine::MAX_CANDIDATES;
use crate::domain::routing::graph::{EdgeId, Graph};
use crate::domain::routing::topology::Topology;

use super::Path;

/// 重複とみなす共有の割合の境界。候補の長さに対して、すでに残した候補と共有する長さが
/// この割合を **超える** ものを捨てる（80%ちょうどは残す）。
pub const OVERLAP_THRESHOLD: f64 = 0.8;

/// 返す候補の距離の上限（デシメートル）。80,000m（behavior.md 5章。契約の `ROUTE_MAX_DISTANCE_M`）。
pub const MAX_ROUTE_DISTANCE_DM: u64 = 800_000;

/// 経路が通る辺の区間（`Path` のドキュメントの規則）：辺番号と、辺の始点からの距離（デシメートル）の範囲。
fn spans(graph: &Graph, path: &Path) -> Vec<(EdgeId, u32, u32)> {
    let last = path.dedges.len().saturating_sub(1);
    path.dedges
        .iter()
        .enumerate()
        .map(|(i, &d)| {
            let edge = d.edge();
            let length = graph.edge(edge).length_dm;
            let (mut lo, mut hi) = (0, length);
            if i == 0 && edge == path.start.edge {
                let offset = path.start.offset_dm.min(length);
                if d.is_reverse() {
                    hi = offset;
                } else {
                    lo = offset;
                }
            }
            if i == last && edge == path.end.edge {
                let offset = path.end.offset_dm.min(length);
                if d.is_reverse() {
                    lo = lo.max(offset);
                } else {
                    hi = hi.min(offset);
                }
            }
            (edge, lo, hi.max(lo))
        })
        .collect()
}

/// 経路の長さ（デシメートル）：通った辺の長さの和。出発・到着の辺は通った部分だけ。
pub fn path_length_dm(graph: &Graph, path: &Path) -> u64 {
    spans(graph, path)
        .iter()
        .map(|&(_, lo, hi)| u64::from(hi - lo))
        .sum()
}

/// 双子の辺 `twin` の区間を、辺 `edge` の始点からの距離（デシメートル、実数）に写す。
/// 双子の向きは、端点の座標が近い組で決める（同じ向きなら比例、逆向きなら反転）。
fn map_twin_interval(graph: &Graph, edge: EdgeId, twin: EdgeId, lo: u32, hi: u32) -> (f64, f64) {
    let (e, t) = (graph.edge(edge), graph.edge(twin));
    let near = |a: u32, b: u32| {
        let (p, q) = (graph.node_coord(a), graph.node_coord(b));
        let dlat = f64::from(p.lat_e7) - f64::from(q.lat_e7);
        let dlon = f64::from(p.lon_e7) - f64::from(q.lon_e7);
        dlat * dlat + dlon * dlon
    };
    let same = near(e.tail, t.tail) + near(e.head, t.head);
    let reversed = near(e.tail, t.head) + near(e.head, t.tail);
    let scale = f64::from(e.length_dm) / f64::from(t.length_dm);
    let (a, b) = (f64::from(lo) * scale, f64::from(hi) * scale);
    if same <= reversed {
        (a, b)
    } else {
        (f64::from(e.length_dm) - b, f64::from(e.length_dm) - a)
    }
}

/// 経路 `a` の長さのうち、経路 `b` と物理区間を共有する長さの割合（0〜1）。`a` の長さが0なら0。
///
/// `a` の各区間について、`b` の同じ辺（と双子の辺）の区間の和集合との重なりの長さを足し、`a` の長さで割る。
pub fn overlap_ratio(graph: &Graph, topology: &Topology, a: &Path, b: &Path) -> f64 {
    let spans_a = spans(graph, a);
    let spans_b = spans(graph, b);
    let total: u64 = spans_a.iter().map(|&(_, lo, hi)| u64::from(hi - lo)).sum();
    if total == 0 {
        return 0.0;
    }
    let mut shared = 0.0_f64;
    for &(edge, lo, hi) in &spans_a {
        let twin = topology.twin_of(edge);
        let mut covered: Vec<(f64, f64)> = Vec::new();
        for &(other, other_lo, other_hi) in &spans_b {
            if other_lo == other_hi {
                continue;
            }
            if other == edge {
                covered.push((f64::from(other_lo), f64::from(other_hi)));
            } else if twin == Some(other) {
                covered.push(map_twin_interval(graph, edge, other, other_lo, other_hi));
            }
        }
        covered.sort_by(|x, y| x.0.total_cmp(&y.0));
        // 区間の和集合と [lo, hi] の重なり。
        let (lo, hi) = (f64::from(lo), f64::from(hi));
        let mut reach = lo;
        for (c_lo, c_hi) in covered {
            let from = c_lo.max(reach);
            let to = c_hi.min(hi);
            if to > from {
                shared += to - from;
                reach = to;
            }
        }
    }
    shared / total as f64
}

/// 候補を整理する（behavior.md 5章「候補の整理」2〜5）。
///
/// `p0` は最短時間経路、`best` は事故考慮経路、`others` は凸包上の解とペナルティ法の代替経路
/// （この順に並べる）。`budget_ds` は時間の上限 B。
///
/// 1. 時間が `budget_ds` を超えるもの、距離が80,000mを超えるものを捨てる（`p0` は対象外）。
/// 2. 前から順に（`p0`、`best`、`others`）、すでに残した候補のいずれかと共有する物理区間の長さが、
///    その候補の長さの [`OVERLAP_THRESHOLD`] を超えるものを捨てる。ただし `p0` と `best` は常に残す。
///    `best` が `p0` と有向辺の列・吸着点まで同じなら、`best` は `p0` と同じ候補として1本にまとめる。
/// 3. 先頭から最大 `MAX_CANDIDATES`（6）本を残す（増やさない）。
/// 4. 所要時間の昇順に安定ソートする（同じ時間なら 2. の順）。
pub fn select_candidates(
    graph: &Graph,
    topology: &Topology,
    p0: Path,
    best: Path,
    others: Vec<Path>,
    budget_ds: u64,
) -> Vec<Path> {
    let same_as_p0 = best.dedges == p0.dedges
        && best.start.edge == p0.start.edge
        && best.start.offset_dm == p0.start.offset_dm
        && best.end.edge == p0.end.edge
        && best.end.offset_dm == p0.end.offset_dm;
    let within_limits =
        |p: &Path| p.time_ds <= budget_ds && path_length_dm(graph, p) <= MAX_ROUTE_DISTANCE_DM;
    let mut kept: Vec<Path> = vec![p0];
    if !same_as_p0 && within_limits(&best) {
        kept.push(best);
    }
    for candidate in others {
        if kept.len() >= MAX_CANDIDATES {
            break;
        }
        if !within_limits(&candidate) {
            continue;
        }
        let duplicate = kept
            .iter()
            .any(|k| overlap_ratio(graph, topology, &candidate, k) > OVERLAP_THRESHOLD);
        if !duplicate {
            kept.push(candidate);
        }
    }
    kept.sort_by_key(|p| p.time_ds);
    kept
}
