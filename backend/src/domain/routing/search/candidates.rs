//! 候補の整理（形式書18章、behavior.md 5章「候補の整理」2〜5）：上限の確認・重複除去・上限6本・
//! 所要時間の昇順。
//!
//! 重複の判定は「共有する物理区間の長さ」。物理区間は辺番号で判定し、双子の辺（`Topology::are_twins`。
//! 1つの辺に双子は複数ありうる）は同じ区間とみなす。双子の区間は、辺の折れ線に投影して、辺のどの区間に
//! 当たるかで数える（長い1本の反対側が短い複数の辺に分かれていても、重なった長さだけが共有になる）。
//! 出発・到着の辺の途中までの部分は、通った部分だけを数える。

use crate::contract::engine::MAX_CANDIDATES;
use crate::contract::geo::METERS_PER_DEGREE;
use crate::domain::routing::graph::{Coord, EdgeId, Graph};
use crate::domain::routing::snap::point_on_edge;
use crate::domain::routing::topology::Topology;

use super::Path;
use super::endpoints::{Placement, path_distance_dm, placement};

/// 重複とみなす共有の割合の境界。候補の長さに対して、すでに残した候補と共有する長さが
/// この割合を **超える** ものを捨てる（80%ちょうどは残す）。
pub const OVERLAP_THRESHOLD: f64 = 0.8;

/// 返す候補の距離の上限（デシメートル）。80,000m（behavior.md 5章。契約の `ROUTE_MAX_DISTANCE_M`）。
pub const MAX_ROUTE_DISTANCE_DM: u64 = 800_000;

/// 経路が通る辺の区間：辺番号と、辺の始点からの距離（デシメートル）の範囲。出発・到着の吸着点が辺の
/// 途中（`endpoints::Placement::Inside`）のときだけ、最初・最後の有向辺は通った部分になる
/// （`endpoints::travelled_lengths_dm` と同じ規則）。
fn spans(graph: &Graph, path: &Path) -> Vec<(EdgeId, u32, u32)> {
    let start_inside = placement(graph, &path.start) == Some(Placement::Inside);
    let end_inside = placement(graph, &path.end) == Some(Placement::Inside);
    let last = path.dedges.len().saturating_sub(1);
    path.dedges
        .iter()
        .enumerate()
        .map(|(i, &d)| {
            let edge = d.edge();
            let length = graph.edge(edge).length_dm;
            let (mut lo, mut hi) = (0, length);
            if i == 0 && start_inside && edge == path.start.edge {
                let offset = path.start.offset_dm.min(length);
                if d.is_reverse() {
                    hi = offset;
                } else {
                    lo = offset;
                }
            }
            if i == last && end_inside && edge == path.end.edge {
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

/// 経路の長さ（デシメートル）：通った辺の長さの和。出発・到着の辺は通った部分だけ
/// （`endpoints::path_distance_dm`）。経路の形が正しくなければ0。
pub fn path_length_dm(graph: &Graph, path: &Path) -> u64 {
    path_distance_dm(graph, &path.start, &path.end, &path.dedges).unwrap_or(0)
}

/// 座標 `p` を辺 `edge` の折れ線に投影した位置（辺の始点からの距離、デシメートル、実数）。
/// 折れ線の端の外側にある点は、端に丸める。距離は折れ線の長さに対する割合を `length_dm` に直す
/// （`snap::point_on_edge` の逆）。
fn offset_on_edge_dm(graph: &Graph, edge: EdgeId, p: Coord) -> f64 {
    let line = graph.edge_polyline(edge);
    let length_dm = f64::from(graph.edge(edge).length_dm);
    let my = METERS_PER_DEGREE / 1e7;
    let cos_lat = (f64::from(p.lat_e7) / 1e7).to_radians().cos().max(0.05);
    let mx = my * cos_lat;
    let xy = |c: Coord| {
        (
            (f64::from(c.lon_e7) - f64::from(p.lon_e7)) * mx,
            (f64::from(c.lat_e7) - f64::from(p.lat_e7)) * my,
        )
    };
    // (原点までの距離, そこまでの折れ線の長さ)。距離が最小の点を選ぶ
    let mut best = (f64::INFINITY, 0.0_f64);
    let mut walked = 0.0_f64;
    for w in line.windows(2) {
        let ((ax, ay), (bx, by)) = (xy(w[0]), xy(w[1]));
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        let t = if len2 > 0.0 {
            (-(ax * dx + ay * dy) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let dist = (ax + t * dx).hypot(ay + t * dy);
        let segment = len2.sqrt();
        if dist < best.0 {
            best = (dist, walked + t * segment);
        }
        walked += segment;
    }
    if walked > 0.0 {
        best.1 / walked * length_dm
    } else {
        0.0
    }
}

/// 双子の辺 `twin` の区間（`twin` の始点からの距離 `lo`〜`hi`、デシメートル）を、辺 `edge` の始点からの
/// 距離（デシメートル、実数）の区間に写す。区間の両端の点を `edge` の折れ線に投影するので、向きが逆でも、
/// 長さが違っても、双子が `edge` の一部にしか並走していなくても、`edge` のどの区間に当たるかが分かる。
/// `edge` の端の外側に当たる部分は端に丸めるので、`edge` と並走しない部分は長さ0になる。
fn map_twin_interval(graph: &Graph, edge: EdgeId, twin: EdgeId, lo: u32, hi: u32) -> (f64, f64) {
    let a = offset_on_edge_dm(graph, edge, point_on_edge(graph, twin, lo));
    let b = offset_on_edge_dm(graph, edge, point_on_edge(graph, twin, hi));
    (a.min(b), a.max(b))
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
        let mut covered: Vec<(f64, f64)> = Vec::new();
        for &(other, other_lo, other_hi) in &spans_b {
            if other_lo == other_hi {
                continue;
            }
            if other == edge {
                covered.push((f64::from(other_lo), f64::from(other_hi)));
            } else if topology.are_twins(edge, other) {
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
    let within_limits = |p: &Path| {
        p.time_ds <= budget_ds
            && path_distance_dm(graph, &p.start, &p.end, &p.dedges)
                .is_some_and(|dm| dm <= MAX_ROUTE_DISTANCE_DM)
    };
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
