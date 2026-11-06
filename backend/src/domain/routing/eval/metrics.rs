//! 方式に依存しない物差し Y1・Y2 と、M2bの割り当てによる Y3（計画書9.3節、作業書M5.4）。
//!
//! - Y1：経路の折れ線（出発・到着の部分辺を含む）から15m以内の将来事故。
//! - Y2：各事故を35m以内の最も近い車道の辺に割り当て、経路が通る辺に載る事故。
//! - Y3：`assignment.bin` の辺・交差点の重みの和。
//!
//! 数える事故は、評価年（`--years`）に含まれ、その手段が関与したものだけ。条件では絞らない。

use std::collections::{BTreeMap, HashSet};

use crate::contract::{LatLon, Mode};
use crate::domain::routing::accident::{EdgeAccident, EdgeAccidentIndex};
use crate::domain::routing::eval::geometry::{
    TIE_EPS_M, distance_to_polyline_m, edges_in_window, path_polyline, project_on_edge,
    sub_polyline, travelled_ranges,
};
use crate::domain::routing::graph::{Coord, DirEdge, EdgeId, Graph, HighwayClass};
use crate::domain::routing::risk::road_group;
use crate::domain::routing::search::endpoints::{Placement, placement};
use crate::domain::routing::snap::{OdIndex, SnapPoint};
use crate::domain::routing::topology::{ClusterId, Topology};
use crate::domain::snapshot::AccidentSnapshot;

/// Y1で数える距離の上限（m、計画書9.3節）。
pub const Y1_RADIUS_M: f64 = 15.0;
/// Y2で事故を辺に割り当てる距離の上限（m、計画書4.3節）。
pub const Y2_RADIUS_M: f64 = 35.0;

/// 交差点クラスタの割り当てに係数を掛ける方法（`--exposure-clusters`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExposureClusterMode {
    /// クラスタの `top_class` の group の係数を掛ける。
    #[default]
    TopGroup,
    /// 掛けない（係数1）。
    None,
}

/// 自動車の曝露の補正に使う係数（計画書5章）。group 0〜15の表と、交差点クラスタの掛け方。
///
/// 表にない group（`road_group` が `None`）と、表で1のgroup（11〜15）は係数1とする。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExposureFactors {
    /// group 0〜15の係数。
    pub groups: [f64; 16],
    /// 交差点クラスタの割り当てへの掛け方。
    pub clusters: ExposureClusterMode,
}

impl Default for ExposureFactors {
    fn default() -> Self {
        Self::uniform(ExposureClusterMode::TopGroup)
    }
}

impl ExposureFactors {
    /// すべてのgroupが1の表（補正なし）。
    pub const fn uniform(clusters: ExposureClusterMode) -> Self {
        Self {
            groups: [1.0; 16],
            clusters,
        }
    }

    /// groupの係数。範囲外・`None` は1。
    pub fn group(&self, group: Option<u8>) -> f64 {
        group
            .and_then(|g| self.groups.get(usize::from(g)))
            .copied()
            .unwrap_or(1.0)
    }

    /// 辺の係数（`road_group(edge の HighwayClass::code)`）。
    pub fn edge(&self, graph: &Graph, edge: EdgeId) -> f64 {
        self.group(road_group(graph.edge(edge).class))
    }

    /// 交差点クラスタの係数。`TopGroup` なら `top_class` のgroup、`None` なら1。
    pub fn cluster(&self, topology: &Topology, cluster: ClusterId) -> f64 {
        match self.clusters {
            ExposureClusterMode::TopGroup => topology
                .clusters
                .get(cluster as usize)
                .map(|record| self.group(road_group(record.top_class)))
                .unwrap_or(1.0),
            ExposureClusterMode::None => 1.0,
        }
    }
}

/// 評価年の事故か。
pub fn year_in(years: &[u16], year: u16) -> bool {
    years.contains(&year)
}

/// その手段が関与した事故か（`modes & mode.mask() != 0`）。
pub fn mode_in(modes: u8, mode: Mode) -> bool {
    modes & mode.mask() != 0
}

/// 事故が評価年・手段の条件を満たすか。
pub fn qualifies(snapshot: &AccidentSnapshot, accident: usize, years: &[u16], mode: Mode) -> bool {
    let columns = snapshot.columns();
    year_in(years, columns.source_year[accident]) && mode_in(columns.modes[accident], mode)
}

/// Y2の割り当て先（辺と、事故をその辺に投影した位置）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Y2Target {
    pub edge: EdgeId,
    pub pos_dm: u32,
}

/// 評価する経路（出発・到着の吸着点と有向辺の列）。`Path` から借用して作る。
#[derive(Debug, Clone, Copy)]
pub struct EvalPath<'a> {
    pub start: &'a SnapPoint,
    pub end: &'a SnapPoint,
    pub dedges: &'a [DirEdge],
}

/// 評価年の事故を、距離だけで最寄りの車道の辺に割り当てた結果。
///
/// 車道の辺は `HighwayClass::code` が0〜15で、歩道の親を持たない辺。手段によらない。
/// 35m以内に辺がなければ `None`。距離が同じなら辺番号の小さい方を選ぶ。
/// 評価年の事故についてだけ1回作り、全経路で使い回す。
#[derive(Debug, Clone, Default)]
pub struct Y2Assignments {
    /// 事故ID → 割り当て先（評価年でない事故も `None`）。
    pub targets: Vec<Option<Y2Target>>,
    /// 評価年の事故の数。
    pub total: usize,
    /// 辺に割り当てた事故の数。
    pub assigned: usize,
}

impl Y2Assignments {
    /// 評価年の事故を、`OdIndex` の格子で近くの車道の辺に割り当てる。
    pub fn build(
        graph: &Graph,
        topology: &Topology,
        index: &OdIndex,
        snapshot: &AccidentSnapshot,
        years: &[u16],
    ) -> Y2Assignments {
        let columns = snapshot.columns();
        let mut targets = vec![None; snapshot.len()];
        let mut total = 0_usize;
        let mut assigned = 0_usize;
        let mut candidates = Vec::new();
        for (accident, (&lat, &lon)) in columns.lat.iter().zip(&columns.lon).enumerate() {
            if !year_in(years, columns.source_year[accident]) {
                continue;
            }
            total += 1;
            let point = Coord::from_latlon(LatLon::new(lat, lon));
            candidates.clear();
            edges_in_window(index, point, Y2_RADIUS_M, &mut candidates);
            let mut best: Option<(f64, EdgeId, u32)> = None;
            for &edge in &candidates {
                if graph.edge(edge).class > HighwayClass::Track.code()
                    || topology.sidewalk_parent(edge).is_some()
                {
                    continue;
                }
                let Some((dist, pos_dm)) = project_on_edge(graph, edge, point) else {
                    continue;
                };
                if dist > Y2_RADIUS_M {
                    continue;
                }
                // 候補は辺番号の昇順なので、同じ距離なら先に見た小さい辺を残す。
                if best.is_none_or(|(d, _, _)| dist < d - TIE_EPS_M) {
                    best = Some((dist, edge, pos_dm));
                }
            }
            if let Some((_, edge, pos_dm)) = best {
                targets[accident] = Some(Y2Target { edge, pos_dm });
                assigned += 1;
            }
        }
        Y2Assignments {
            targets,
            total,
            assigned,
        }
    }
}

/// Y1：経路の折れ線から15m以内の評価年の事故（その手段が関与）の件数。
///
/// 事故の格子（0.001度）で近くの事故だけを調べ、経路の折れ線全体までの距離で判定する。
/// 経路の形が正しくなければ `None`。
pub fn y1_count(
    graph: &Graph,
    snapshot: &AccidentSnapshot,
    years: &[u16],
    mode: Mode,
    path: EvalPath<'_>,
) -> Option<usize> {
    let line = path_polyline(graph, path.start, path.end, path.dedges)?;
    if line.is_empty() {
        return Some(0);
    }
    let columns = snapshot.columns();
    let mut checked: HashSet<u32> = HashSet::new();
    let mut count = 0_usize;
    for w in line.windows(2) {
        for cell in
            crate::domain::routing::eval::geometry::cells_around_segment(w[0], w[1], Y1_RADIUS_M)
        {
            for &id in snapshot.cell(cell) {
                if !checked.insert(id) {
                    continue;
                }
                let accident = id as usize;
                if !qualifies(snapshot, accident, years, mode) {
                    continue;
                }
                let point =
                    Coord::from_latlon(LatLon::new(columns.lat[accident], columns.lon[accident]));
                if distance_to_polyline_m(point, &line) <= Y1_RADIUS_M {
                    count += 1;
                }
            }
        }
    }
    Some(count)
}

/// 経路の各有向辺が実際に通る部分の折れ線（出発・到着の部分辺を含む）。
fn edge_pieces(
    graph: &Graph,
    start: &SnapPoint,
    end: &SnapPoint,
    dedges: &[DirEdge],
) -> Option<Vec<(EdgeId, Vec<Coord>)>> {
    let mut out = Vec::with_capacity(dedges.len());
    for range in travelled_ranges(graph, start, end, dedges)? {
        let line = graph.edge_polyline(range.dedge.edge());
        let length = f64::from(graph.edge(range.dedge.edge()).length_dm).max(1.0);
        let mut piece = sub_polyline(
            &line,
            f64::from(range.from_dm) / length,
            f64::from(range.to_dm) / length,
        );
        if range.dedge.is_reverse() {
            piece.reverse();
        }
        out.push((range.dedge.edge(), piece));
    }
    Some(out)
}

/// 経路の辺のうち、事故に最も近いもの。距離が同じなら辺番号の小さい方を選ぶ。
fn nearest_path_edge(point: Coord, pieces: &[(EdgeId, Vec<Coord>)]) -> Option<EdgeId> {
    let mut best: Option<(f64, EdgeId)> = None;
    for (edge, piece) in pieces {
        let distance = distance_to_polyline_m(point, piece);
        let better = match best {
            None => true,
            Some((best_distance, best_edge)) => {
                distance < best_distance - TIE_EPS_M
                    || ((distance - best_distance).abs() <= TIE_EPS_M && *edge < best_edge)
            }
        };
        if better {
            best = Some((distance, *edge));
        }
    }
    best.map(|(_, edge)| edge)
}

/// Y1x：Y1で数える事故1件ごとに、その事故に最も近い経路の辺の group の係数を足す。
///
/// 出発・到着の部分辺を含む実際に通る部分だけを比べ、距離が同じなら辺番号の小さい方を使う。
/// 自動車以外は係数1（Y1と同じ値）。
pub fn y1_weighted(
    graph: &Graph,
    snapshot: &AccidentSnapshot,
    years: &[u16],
    mode: Mode,
    path: EvalPath<'_>,
    factors: &ExposureFactors,
) -> Option<f64> {
    let line = path_polyline(graph, path.start, path.end, path.dedges)?;
    if line.is_empty() {
        return Some(0.0);
    }
    let pieces = edge_pieces(graph, path.start, path.end, path.dedges)?;
    let columns = snapshot.columns();
    let mut checked: HashSet<u32> = HashSet::new();
    let mut total = 0.0_f64;
    for w in line.windows(2) {
        for cell in
            crate::domain::routing::eval::geometry::cells_around_segment(w[0], w[1], Y1_RADIUS_M)
        {
            for &id in snapshot.cell(cell) {
                if !checked.insert(id) {
                    continue;
                }
                let accident = id as usize;
                if !qualifies(snapshot, accident, years, mode) {
                    continue;
                }
                let point =
                    Coord::from_latlon(LatLon::new(columns.lat[accident], columns.lon[accident]));
                if distance_to_polyline_m(point, &line) <= Y1_RADIUS_M {
                    let factor = if mode == Mode::Car {
                        nearest_path_edge(point, &pieces)
                            .map(|edge| factors.edge(graph, edge))
                            .unwrap_or(1.0)
                    } else {
                        1.0
                    };
                    total += factor;
                }
            }
        }
    }
    Some(total)
}

/// 経路が通る辺と、その辺の通る範囲（辺の始点からのdm）。全長なら `(0, length)`。
fn traversed_intervals(
    graph: &Graph,
    topology: &Topology,
    start: &SnapPoint,
    end: &SnapPoint,
    dedges: &[DirEdge],
) -> Option<BTreeMap<EdgeId, Vec<(u32, u32)>>> {
    let ranges = travelled_ranges(graph, start, end, dedges)?;
    let mut out: BTreeMap<EdgeId, Vec<(u32, u32)>> = BTreeMap::new();
    for range in ranges {
        let edge = range.dedge.edge();
        out.entry(edge)
            .or_default()
            .push((range.from_dm, range.to_dm));
        // 歩道は親の車道の辺も通るものとして扱う（親の辺全体。並ぶ部分の厳密な判定はしない）。
        if let Some(parent) = topology.sidewalk_parent(edge) {
            let length = graph.edge(parent).length_dm;
            out.entry(parent).or_default().push((0, length));
        }
    }
    Some(out)
}

/// 区間のどれかが `pos_dm` を含むか。
fn interval_contains(intervals: &[(u32, u32)], pos_dm: u32) -> bool {
    intervals
        .iter()
        .any(|&(from, to)| from <= pos_dm && pos_dm <= to)
}

/// Y2：経路が通る辺に割り当てられた、評価年の事故（その手段が関与）の件数。
///
/// 出発・到着の部分辺では、事故を辺に投影した位置が通る範囲に入る場合だけ数える。
/// 歩道の辺を通るときは、親の車道の辺全体を通るものとして数える。
pub fn y2_count(
    graph: &Graph,
    topology: &Topology,
    snapshot: &AccidentSnapshot,
    years: &[u16],
    mode: Mode,
    assignments: &Y2Assignments,
    path: EvalPath<'_>,
) -> Option<usize> {
    let traversed = traversed_intervals(graph, topology, path.start, path.end, path.dedges)?;
    let mut count = 0_usize;
    for (accident, target) in assignments.targets.iter().enumerate() {
        let Some(target) = target else {
            continue;
        };
        if !qualifies(snapshot, accident, years, mode) {
            continue;
        }
        let Some(intervals) = traversed.get(&target.edge) else {
            continue;
        };
        if interval_contains(intervals, target.pos_dm) {
            count += 1;
        }
    }
    Some(count)
}

/// Y2x：Y2で数える事故1件ごとに、割り当て先の辺の group の係数を足す。
///
/// 自動車以外は係数1（Y2と同じ値）。
#[allow(clippy::too_many_arguments)]
pub fn y2_weighted(
    graph: &Graph,
    topology: &Topology,
    snapshot: &AccidentSnapshot,
    years: &[u16],
    mode: Mode,
    assignments: &Y2Assignments,
    path: EvalPath<'_>,
    factors: &ExposureFactors,
) -> Option<f64> {
    let traversed = traversed_intervals(graph, topology, path.start, path.end, path.dedges)?;
    let mut total = 0.0_f64;
    for (accident, target) in assignments.targets.iter().enumerate() {
        let Some(target) = target else {
            continue;
        };
        if !qualifies(snapshot, accident, years, mode) {
            continue;
        }
        let Some(intervals) = traversed.get(&target.edge) else {
            continue;
        };
        if interval_contains(intervals, target.pos_dm) {
            total += if mode == Mode::Car {
                factors.edge(graph, target.edge)
            } else {
                1.0
            };
        }
    }
    Some(total)
}

/// 歩道の通る範囲を、親の車道の辺の同じ割合の範囲に写す。
///
/// `path_risk` の歩道の扱い（親のリスク密度 × 通った長さ）に合わせ、歩道の長さに対する
/// 位置の割合をそのまま親の辺の長さに対する割合とする。歩道と親の向きが逆でも同じ扱いにする。
fn parent_interval(
    from_dm: u32,
    to_dm: u32,
    sidewalk_length_dm: u32,
    parent_length_dm: u32,
) -> (u32, u32) {
    if sidewalk_length_dm == 0 {
        return (0, parent_length_dm);
    }
    let scale = |value: u32| {
        let num = u64::from(value) * u64::from(parent_length_dm);
        let den = u64::from(sidewalk_length_dm);
        ((num + den / 2) / den).min(u64::from(parent_length_dm)) as u32
    };
    (scale(from_dm), scale(to_dm))
}

fn edge_weight(
    index: &EdgeAccidentIndex,
    snapshot: &AccidentSnapshot,
    years: &[u16],
    mode: Mode,
    edge: EdgeId,
    from_dm: u32,
    to_dm: u32,
) -> f64 {
    let mut total = 0.0_f64;
    for EdgeAccident {
        accident,
        pos_dm,
        weight,
        ..
    } in index.edge_entries(edge)
    {
        if *pos_dm < from_dm || *pos_dm > to_dm {
            continue;
        }
        if qualifies(snapshot, *accident as usize, years, mode) {
            total += f64::from(*weight);
        }
    }
    total
}

/// `edge_weight` の各区間の重みに、辺の係数 `factor` を掛けたもの。
#[allow(clippy::too_many_arguments)]
fn edge_weight_with_factor(
    index: &EdgeAccidentIndex,
    snapshot: &AccidentSnapshot,
    years: &[u16],
    mode: Mode,
    edge: EdgeId,
    from_dm: u32,
    to_dm: u32,
    factor: f64,
) -> f64 {
    let mut total = 0.0_f64;
    for EdgeAccident {
        accident,
        pos_dm,
        weight,
        ..
    } in index.edge_entries(edge)
    {
        if *pos_dm < from_dm || *pos_dm > to_dm {
            continue;
        }
        if qualifies(snapshot, *accident as usize, years, mode) {
            total += f64::from(*weight) * factor;
        }
    }
    total
}

/// Y3：`EdgeAccidentIndex` の辺の割り当て（`MIRROR` を含む）と、外から入る交差点クラスタの
/// 割り当ての重みの和。`path_risk` と同じ規則で、到着の部分辺ではクラスタを数えない。
///
/// 歩道の辺は親の車道の辺の割り当てを、歩道の通る範囲を同じ割合で写した範囲だけ数える。
pub fn y3_weight(
    graph: &Graph,
    topology: &Topology,
    index: &EdgeAccidentIndex,
    snapshot: &AccidentSnapshot,
    years: &[u16],
    mode: Mode,
    path: EvalPath<'_>,
) -> Option<f64> {
    let ranges = travelled_ranges(graph, path.start, path.end, path.dedges)?;
    let start_inside = placement(graph, path.start) == Some(Placement::Inside);
    let end_inside = placement(graph, path.end) == Some(Placement::Inside);
    let last = path.dedges.len().checked_sub(1)?;
    let direct = path.dedges.len() == 1 && start_inside && end_inside;
    let mut total = 0.0_f64;
    for (i, range) in ranges.iter().enumerate() {
        let edge = range.dedge.edge();
        if let Some(parent) = topology.sidewalk_parent(edge) {
            let (from_dm, to_dm) = parent_interval(
                range.from_dm,
                range.to_dm,
                graph.edge(edge).length_dm,
                graph.edge(parent).length_dm,
            );
            total += edge_weight(index, snapshot, years, mode, parent, from_dm, to_dm);
        } else {
            total += edge_weight(
                index,
                snapshot,
                years,
                mode,
                edge,
                range.from_dm,
                range.to_dm,
            );
        }
        // 到着の部分辺は到着ノードに届かないので、クラスタを加えない。
        let arrival_partial = !direct && i == last && end_inside;
        if !direct
            && !arrival_partial
            && let Some(cluster) = topology.entering_cluster(range.dedge, graph)
        {
            for entry in index.cluster_entries(cluster) {
                if qualifies(snapshot, entry.accident as usize, years, mode) {
                    total += f64::from(entry.weight);
                }
            }
        }
    }
    Some(total)
}

/// Y3x：Y3で足す割り当ての重みに、辺のgroupの係数（歩道は親の車道の辺）を掛けたもの。
/// 交差点クラスタは `ExposureClusterMode::TopGroup` なら `top_class` のgroup、
/// `None` なら1を掛ける。自動車以外は係数1（Y3と同じ値）。
#[allow(clippy::too_many_arguments)]
pub fn y3_weighted(
    graph: &Graph,
    topology: &Topology,
    index: &EdgeAccidentIndex,
    snapshot: &AccidentSnapshot,
    years: &[u16],
    mode: Mode,
    path: EvalPath<'_>,
    factors: &ExposureFactors,
) -> Option<f64> {
    let ranges = travelled_ranges(graph, path.start, path.end, path.dedges)?;
    let start_inside = placement(graph, path.start) == Some(Placement::Inside);
    let end_inside = placement(graph, path.end) == Some(Placement::Inside);
    let last = path.dedges.len().checked_sub(1)?;
    let direct = path.dedges.len() == 1 && start_inside && end_inside;
    let edge_factor = |edge| {
        if mode == Mode::Car {
            factors.edge(graph, edge)
        } else {
            1.0
        }
    };
    let mut total = 0.0_f64;
    for (i, range) in ranges.iter().enumerate() {
        let edge = range.dedge.edge();
        if let Some(parent) = topology.sidewalk_parent(edge) {
            let (from_dm, to_dm) = parent_interval(
                range.from_dm,
                range.to_dm,
                graph.edge(edge).length_dm,
                graph.edge(parent).length_dm,
            );
            total += edge_weight_with_factor(
                index,
                snapshot,
                years,
                mode,
                parent,
                from_dm,
                to_dm,
                edge_factor(parent),
            );
        } else {
            total += edge_weight_with_factor(
                index,
                snapshot,
                years,
                mode,
                edge,
                range.from_dm,
                range.to_dm,
                edge_factor(edge),
            );
        }
        // 到着の部分辺は到着ノードに届かないので、クラスタを加えない。
        let arrival_partial = !direct && i == last && end_inside;
        if !direct
            && !arrival_partial
            && let Some(cluster) = topology.entering_cluster(range.dedge, graph)
        {
            let factor = if mode == Mode::Car {
                factors.cluster(topology, cluster)
            } else {
                1.0
            };
            for entry in index.cluster_entries(cluster) {
                if qualifies(snapshot, entry.accident as usize, years, mode) {
                    total += f64::from(entry.weight) * factor;
                }
            }
        }
    }
    Some(total)
}

/// 経路の道のり（m）。
pub fn length_m(graph: &Graph, path: EvalPath<'_>) -> Option<f64> {
    crate::domain::routing::eval::geometry::path_distance_m(
        graph,
        path.start,
        path.end,
        path.dedges,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_interval_maps_the_same_fraction() {
        assert_eq!(parent_interval(0, 100, 100, 250), (0, 250));
        assert_eq!(parent_interval(25, 75, 100, 200), (50, 150));
        // 親が短くても、割合を保って丸める。
        assert_eq!(parent_interval(1, 3, 10, 5), (1, 2));
    }
}
