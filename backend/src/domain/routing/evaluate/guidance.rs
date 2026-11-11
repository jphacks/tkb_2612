//! 進行方向の案内地点（計画書4章）。
//!
//! 経路の途中のノードで、経路の次以外の出口があるところを分岐とみなし、連にまとめてから
//! 前後の方位の変化で右左折を決める。道路のカーブだけでは案内しない。

use crate::contract::{CancelToken, GuidanceStep, Interrupt, LonLat, Maneuver, Mode};
use crate::domain::corridor::CANCEL_CHECK_INTERVAL;
use crate::domain::routing::evaluate::geometry::RouteShape;
use crate::domain::routing::graph::time::turn_angle_deg;
use crate::domain::routing::graph::{DirEdge, Graph, NodeId, edge_flags};
use crate::domain::routing::risk::road_group;
use crate::domain::routing::search::Path;
use crate::domain::routing::topology::Topology;

/// 連の前後の方位を測る長さ（m）。
pub const BEARING_BASELINE_M: f64 = 20.0;
/// 同じ連にまとめる、分岐ノード間の距離（m）。
pub const RUN_GAP_M: f64 = 15.0;
/// 連の長さの上限（m、環状交差点を除く）。
pub const RUN_MAX_M: f64 = 60.0;
/// 分岐の相手とみなす出口の角度（度）。
pub const FORK_MAX_DEG: f64 = 45.0;
/// 連の角度がこれより小さければ、ほぼ直進とみなす（度）。
pub const STRAIGHT_MAX_DEG: f64 = 10.0;
/// ほぼ直進のときに分岐の相手とみなす出口の角度（度）。
pub const STRAIGHT_FORK_MAX_DEG: f64 = 20.0;
/// 右折・左折にする下限（度）。
pub const TURN_MIN_DEG: f64 = 45.0;
/// 斜めにする下限（度）。
pub const SLIGHT_MIN_DEG: f64 = 30.0;
/// 道なりとして省略できる上限（度）。
pub const OBVIOUS_MAX_DEG: f64 = 60.0;
/// Uターンにする下限（度）。
pub const UTURN_MIN_DEG: f64 = 150.0;
/// 到着の直前の案内を省く距離（m）。
pub const END_SUPPRESS_M: f64 = 10.0;
/// 連の先頭がこの距離より始点に近ければ案内しない（m）。手前の方位を測れないため。
pub const START_SUPPRESS_M: f64 = 10.0;

/// 距離の比較の許容（m）。
const DISTANCE_EPS_M: f64 = 1e-9;

/// 経路の途中のノード1つ分の判定材料。
struct NodeInfo {
    /// `path.dedges` の何番目の有向辺の終点か。
    index: usize,
    /// グラフのノード。
    node: NodeId,
    /// `shape.full` の頂点番号。
    vertex: usize,
    /// 経路始点からこのノードまでの `shape.full` の累積距離（m）。
    distance_m: f64,
    /// 経路の次でも引き返しでもない、通れる出口。
    others: Vec<DirEdge>,
    /// 逆向きにしか通れない道路。進めないが、分岐として目に見える。
    one_ways: Vec<DirEdge>,
}

impl NodeInfo {
    fn has_branch(&self) -> bool {
        !self.others.is_empty() || !self.one_ways.is_empty()
    }
}

/// 同じ案内にまとめるノードの連。
struct Run {
    /// `NodeInfo` の添字（経路に沿った順）。
    nodes: Vec<usize>,
    /// 環状交差点の辺を含むか。
    has_roundabout: bool,
}

/// 案内地点の列を作る（計画書4章）。最後は必ず `arrive`（`at_m = length_m`）。
///
/// `vertex_distances_m` は `shape.display` の各頂点の経路始点からの距離（m）。
/// 案内地点は `shape.full` の頂点で決め、間引いていれば表示用の距離軸へ補間する。
pub fn build_guidance(
    graph: &Graph,
    topology: &Topology,
    path: &Path,
    shape: &RouteShape,
    mode: Mode,
    vertex_distances_m: &[f64],
    cancel: &CancelToken,
) -> Result<Vec<GuidanceStep>, Interrupt> {
    cancel.check()?;
    let length_m = vertex_distances_m.last().copied().unwrap_or(0.0);
    let full_distance_m = full_distances(shape);
    let full_xy: Vec<(f64, f64)> = shape
        .full
        .iter()
        .map(|&c| shape.projection.xy(LonLat::from(c.to_latlon())))
        .collect();
    let mut steps = Vec::new();
    if path.dedges.len() >= 2 && !full_distance_m.is_empty() {
        let infos = node_infos(graph, shape, path, mode, &full_distance_m, cancel)?;
        let runs = group_runs(graph, topology, &path.dedges, &infos);
        for (i, run) in runs.iter().enumerate() {
            if i % CANCEL_CHECK_INTERVAL == 0 {
                cancel.check()?;
            }
            if infos[run.nodes[0]].distance_m < START_SUPPRESS_M {
                continue;
            }
            let range = bearing_range(&runs, &infos, i, &full_distance_m);
            let Some(maneuver) = classify(
                graph,
                &path.dedges,
                &infos,
                run,
                &full_distance_m,
                &full_xy,
                range,
            ) else {
                continue;
            };
            let node = if run.has_roundabout {
                *run.nodes.last().expect("連は空でない")
            } else {
                sharpest_node(graph, &path.dedges, &infos, run)
            };
            let at_m = at_m_for_vertex(
                shape,
                vertex_distances_m,
                &full_distance_m,
                infos[node].vertex,
            );
            if at_m < length_m - END_SUPPRESS_M {
                steps.push(GuidanceStep { maneuver, at_m });
            }
        }
    }
    cancel.check()?;
    steps.push(GuidanceStep {
        maneuver: Maneuver::Arrive,
        at_m: length_m,
    });
    Ok(steps)
}

/// `shape.full` の各頂点までの累積距離（m）。
fn full_distances(shape: &RouteShape) -> Vec<f64> {
    let mut distances = Vec::with_capacity(shape.full.len());
    let mut total = 0.0;
    if shape.full.is_empty() {
        return distances;
    }
    distances.push(total);
    for pair in shape.full.windows(2) {
        total += shape.projection.distance(
            LonLat::from(pair[0].to_latlon()),
            LonLat::from(pair[1].to_latlon()),
        );
        distances.push(total);
    }
    distances
}

/// 経路の途中のノードごとに、経路の次でも引き返しでもない出口を求める（手順1）。
fn node_infos(
    graph: &Graph,
    shape: &RouteShape,
    path: &Path,
    mode: Mode,
    full_distance_m: &[f64],
    cancel: &CancelToken,
) -> Result<Vec<NodeInfo>, Interrupt> {
    let mut infos = Vec::with_capacity(path.dedges.len().saturating_sub(1));
    for (i, &d) in path.dedges.iter().enumerate().take(path.dedges.len() - 1) {
        if i % CANCEL_CHECK_INTERVAL == 0 {
            cancel.check()?;
        }
        let Some(vertex) = shape.internal_node_vertex(i) else {
            continue;
        };
        let Some(&distance_m) = full_distance_m.get(vertex) else {
            continue;
        };
        let node = graph.dedge_to(d);
        let next = path.dedges[i + 1];
        let back = d.reversed();
        let (others, one_ways): (Vec<DirEdge>, Vec<DirEdge>) = graph
            .out_dedges(node)
            .iter()
            .copied()
            .filter(|&out| out != next && out != back)
            .filter(|&out| graph.passable(out, mode) || graph.passable(out.reversed(), mode))
            .filter(|&out| !graph.node_blocked(graph.dedge_to(out), mode))
            .partition(|&out| graph.passable(out, mode));
        infos.push(NodeInfo {
            index: i,
            node,
            vertex,
            distance_m,
            others,
            one_ways,
        });
    }
    Ok(infos)
}

/// 中間ノードを連にまとめる（手順2）。
fn group_runs(
    graph: &Graph,
    topology: &Topology,
    dedges: &[DirEdge],
    infos: &[NodeInfo],
) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut current: Option<Run> = None;
    let mut last_branch_distance_m: Option<f64> = None;
    for (pos, info) in infos.iter().enumerate() {
        let d = dedges[info.index];
        let next = dedges[info.index + 1];
        let is_roundabout = graph.edge(d.edge()).flags & edge_flags::ROUNDABOUT != 0;
        let is_entrance =
            !is_roundabout && graph.edge(next.edge()).flags & edge_flags::ROUNDABOUT != 0;
        // 引き返すノードは、他に出口がなくても案内するので連を始める。
        let starts_run = info.has_branch() || next == d.reversed();

        if is_entrance {
            if let Some(run) = current.take() {
                runs.push(run);
            }
            current = Some(Run {
                nodes: vec![pos],
                has_roundabout: true,
            });
        } else if let Some(run) = current.as_ref() {
            // 連から離れたノードどうしが同じクラスタでも、この連には加えない。
            let same_cluster = run.nodes.last() == Some(&(pos - 1))
                && matches!(
                    (
                        topology.cluster_of(infos[pos - 1].node),
                        topology.cluster_of(info.node)
                    ),
                    (Some(a), Some(b)) if a == b
                );
            let within_gap = info.has_branch()
                && last_branch_distance_m.is_some_and(|distance| {
                    info.distance_m - distance <= RUN_GAP_M + DISTANCE_EPS_M
                });
            // 環を出た先のノードを加えると、案内地点が出口からずれて後続の曲がりと合成される。
            let left_roundabout = run.has_roundabout && !is_roundabout;
            let attach = is_roundabout || (!left_roundabout && (same_cluster || within_gap));
            let start_distance_m = infos[run.nodes[0]].distance_m;
            let over_max = attach
                && !is_roundabout
                && info.distance_m - start_distance_m > RUN_MAX_M + DISTANCE_EPS_M;
            if over_max {
                runs.push(current.take().expect("連がある"));
                current = Some(Run {
                    nodes: vec![pos],
                    has_roundabout: is_roundabout,
                });
            } else if attach {
                let run = current.as_mut().expect("連がある");
                run.nodes.push(pos);
                run.has_roundabout |= is_roundabout;
            } else if starts_run {
                runs.push(current.take().expect("連がある"));
                current = Some(Run {
                    nodes: vec![pos],
                    has_roundabout: is_roundabout,
                });
            }
        } else if starts_run {
            current = Some(Run {
                nodes: vec![pos],
                has_roundabout: is_roundabout,
            });
        }

        if info.has_branch() {
            last_branch_distance_m = Some(info.distance_m);
        }
    }
    if let Some(run) = current {
        runs.push(run);
    }
    runs
}

/// 連の前後の方位を測ってよい、経路に沿った距離の範囲（m）。
#[derive(Clone, Copy)]
struct BearingRange {
    from_m: f64,
    to_m: f64,
}

/// 環状交差点の連とその隣の連は、相手の連の境界を越えて方位を測らない。環の曲線や、
/// 環のすぐ前後の曲がりが合成されて、向きを誤るため。
fn bearing_range(
    runs: &[Run],
    infos: &[NodeInfo],
    index: usize,
    full_distance_m: &[f64],
) -> BearingRange {
    let run = &runs[index];
    let separated = |other: &&Run| other.has_roundabout || run.has_roundabout;
    let from_m = index
        .checked_sub(1)
        .map(|previous| &runs[previous])
        .filter(separated)
        .map_or(0.0, |previous| {
            infos[*previous.nodes.last().expect("連は空でない")].distance_m
        });
    let to_m = runs
        .get(index + 1)
        .filter(separated)
        .map_or(full_distance_m.last().copied().unwrap_or(0.0), |next| {
            infos[next.nodes[0]].distance_m
        });
    BearingRange { from_m, to_m }
}

/// 連の角度（度、時計回りが正）を、先頭ノードの手前と末尾ノードの先の方位から測る（手順3）。
fn run_angle(
    xy: &[(f64, f64)],
    full_distance_m: &[f64],
    start_distance_m: f64,
    end_distance_m: f64,
    range: BearingRange,
) -> f64 {
    if xy.len() < 2 || full_distance_m.len() < 2 {
        return 0.0;
    }
    let start_from = (start_distance_m - BEARING_BASELINE_M).max(range.from_m);
    let start = (
        point_at(xy, full_distance_m, start_from),
        point_at(xy, full_distance_m, start_distance_m),
    );
    let end = (
        point_at(xy, full_distance_m, end_distance_m),
        point_at(
            xy,
            full_distance_m,
            (end_distance_m + BEARING_BASELINE_M).min(range.to_m),
        ),
    );
    let v1 = (start.1.0 - start.0.0, start.1.1 - start.0.1);
    let v2 = (end.1.0 - end.0.0, end.1.1 - end.0.1);
    if (v1.0 == 0.0 && v1.1 == 0.0) || (v2.0 == 0.0 && v2.1 == 0.0) {
        return 0.0;
    }
    // `turn_angle_deg` と同じく、東をx・北をyとした時計回りの角度。
    let cross = v1.1 * v2.0 - v1.0 * v2.1;
    let dot = v1.0 * v2.0 + v1.1 * v2.1;
    cross.atan2(dot).to_degrees()
}

/// 折れ線の、始点から `distance_m` の点（局所平面の座標）。
fn point_at(xy: &[(f64, f64)], full_distance_m: &[f64], distance_m: f64) -> (f64, f64) {
    if xy.is_empty() {
        return (0.0, 0.0);
    }
    if xy.len() == 1 {
        return xy[0];
    }
    let total = *full_distance_m.last().expect("空でない");
    let distance = distance_m.clamp(0.0, total);
    let at = full_distance_m.partition_point(|&d| d <= distance);
    let j = at.saturating_sub(1).min(xy.len() - 2);
    let (a, b) = (full_distance_m[j], full_distance_m[j + 1]);
    let t = if b > a {
        ((distance - a) / (b - a)).clamp(0.0, 1.0)
    } else {
        1.0
    };
    (
        xy[j].0 + (xy[j + 1].0 - xy[j].0) * t,
        xy[j].1 + (xy[j + 1].1 - xy[j].1) * t,
    )
}

/// 連を分類する（手順4）。案内なしなら `None`。
fn classify(
    graph: &Graph,
    dedges: &[DirEdge],
    infos: &[NodeInfo],
    run: &Run,
    full_distance_m: &[f64],
    full_xy: &[(f64, f64)],
    range: BearingRange,
) -> Option<Maneuver> {
    if run
        .nodes
        .iter()
        .any(|&i| dedges[infos[i].index + 1] == dedges[infos[i].index].reversed())
    {
        return Some(Maneuver::Uturn);
    }
    if !run.nodes.iter().any(|&i| infos[i].has_branch()) {
        return None;
    }
    let start_distance_m = infos[run.nodes[0]].distance_m;
    let end_distance_m = infos[*run.nodes.last().expect("連は空でない")].distance_m;
    let angle = run_angle(
        full_xy,
        full_distance_m,
        start_distance_m,
        end_distance_m,
        range,
    );
    let magnitude = angle.abs();
    if magnitude >= UTURN_MIN_DEG {
        return Some(Maneuver::Uturn);
    }
    if (SLIGHT_MIN_DEG..OBVIOUS_MAX_DEG).contains(&magnitude)
        && is_obvious(graph, dedges, infos, run)
    {
        return None;
    }
    if magnitude >= TURN_MIN_DEG {
        return Some(if angle > 0.0 {
            Maneuver::Right
        } else {
            Maneuver::Left
        });
    }
    if magnitude >= SLIGHT_MIN_DEG {
        // 斜めでも、近い角度の出口と分かれるなら、その出口との位置関係で向きを決める
        // （両方の出口が同じ側に開くY字で、逆の案内を返さないため）。
        if let Some(fork) = fork_maneuver(graph, dedges, infos, run, angle) {
            return Some(fork);
        }
        return Some(if angle > 0.0 {
            Maneuver::SlightRight
        } else {
            Maneuver::SlightLeft
        });
    }
    fork_maneuver(graph, dedges, infos, run, angle)
}

/// 道路の区分。`None` は最下位。
fn class_rank(class: u8) -> u8 {
    road_group(class).unwrap_or(u8::MAX)
}

/// 道なりか（連の入る辺と出る辺が同じ区分で、ほかの出口がすべて下位）。
fn is_obvious(graph: &Graph, dedges: &[DirEdge], infos: &[NodeInfo], run: &Run) -> bool {
    let first = &infos[run.nodes[0]];
    let last = &infos[*run.nodes.last().expect("連は空でない")];
    let in_class = graph.edge(dedges[first.index].edge()).class;
    let out_class = graph.edge(dedges[last.index + 1].edge()).class;
    if road_group(in_class) != road_group(out_class) {
        return false;
    }
    let out_rank = class_rank(out_class);
    run.nodes
        .iter()
        .flat_map(|&i| infos[i].others.iter().chain(&infos[i].one_ways))
        .all(|&other| class_rank(graph.edge(other.edge()).class) > out_rank)
}

/// `_link` の道路区分か（`road_group` の奇数側）。
const fn is_link(class: u8) -> bool {
    matches!(class, 1 | 3 | 5 | 7 | 9)
}

/// 斜めの分岐か（手順4の「分岐」）。向きは、分岐の相手の出口と経路の位置関係で決める。
/// 出口がすべて経路より右なら `slight_left`、すべて左なら `slight_right` とする。
/// 両側にあれば（3叉の中央の枝など）、ほぼ直進でなければ連の角度の符号で決め、
/// ほぼ直進なら最初に見つかった出口との位置関係で決める。
fn fork_maneuver(
    graph: &Graph,
    dedges: &[DirEdge],
    infos: &[NodeInfo],
    run: &Run,
    angle: f64,
) -> Option<Maneuver> {
    let straight = angle.abs() < STRAIGHT_MAX_DEG;
    let limit = if straight {
        STRAIGHT_FORK_MAX_DEG
    } else {
        FORK_MAX_DEG
    };
    let last = &infos[*run.nodes.last().expect("連は空でない")];
    let out_class = graph.edge(dedges[last.index + 1].edge()).class;
    let out_rank = class_rank(out_class);
    // 最初に見つかった出口の側と、両側に出口があるか。
    let mut first_side: Option<Maneuver> = None;
    let mut both_sides = false;
    for &i in &run.nodes {
        let info = &infos[i];
        let local =
            turn_angle_deg(graph, dedges[info.index], dedges[info.index + 1]).unwrap_or(0.0);
        for &other in &info.others {
            let Some(other_angle) = turn_angle_deg(graph, dedges[info.index], other) else {
                continue;
            };
            let other_class = graph.edge(other.edge()).class;
            if other_angle.abs() >= limit
                || class_rank(other_class) > out_rank
                // 本線を走っている間、ランプの出口は分岐として案内しない。
                || (is_link(other_class) && !is_link(out_class))
            {
                continue;
            }
            // 出口が経路より右なら経路は左側の枝、左なら右側の枝。
            let side = if other_angle > local {
                Maneuver::SlightLeft
            } else if other_angle < local {
                Maneuver::SlightRight
            } else {
                continue;
            };
            match first_side {
                None => first_side = Some(side),
                Some(first) if first != side => both_sides = true,
                Some(_) => {}
            }
        }
    }
    let side = first_side?;
    if both_sides && !straight {
        return Some(if angle > 0.0 {
            Maneuver::SlightRight
        } else {
            Maneuver::SlightLeft
        });
    }
    Some(side)
}

/// 案内地点にするノード（手順5）。連の中で局所角の絶対値が最大のノード（同値なら先）。
fn sharpest_node(graph: &Graph, dedges: &[DirEdge], infos: &[NodeInfo], run: &Run) -> usize {
    let mut best = run.nodes[0];
    let mut best_angle = local_angle(graph, dedges, &infos[best]);
    for &i in &run.nodes[1..] {
        let angle = local_angle(graph, dedges, &infos[i]);
        if angle > best_angle {
            best = i;
            best_angle = angle;
        }
    }
    best
}

fn local_angle(graph: &Graph, dedges: &[DirEdge], info: &NodeInfo) -> f64 {
    turn_angle_deg(graph, dedges[info.index], dedges[info.index + 1]).map_or(0.0, f64::abs)
}

/// `shape.full` の頂点番号を、`vertex_distances_m` の距離軸へ変換する。
fn at_m_for_vertex(
    shape: &RouteShape,
    vertex_distances_m: &[f64],
    full_distance_m: &[f64],
    vertex: usize,
) -> f64 {
    let Some(&last) = vertex_distances_m.last() else {
        return 0.0;
    };
    let Some(source) = &shape.display_source else {
        return vertex_distances_m.get(vertex).copied().unwrap_or(last);
    };
    let vertex = vertex.min(full_distance_m.len().saturating_sub(1));
    // `source` は `full` の頂点番号の昇順。
    let at = source.partition_point(|&i| i < vertex);
    if let Some(&kept) = source.get(at)
        && kept == vertex
    {
        return vertex_distances_m.get(at).copied().unwrap_or(last);
    }
    if at == 0 {
        return vertex_distances_m.first().copied().unwrap_or(0.0);
    }
    if at >= source.len() {
        return last;
    }
    let (a, b) = (source[at - 1], source[at]);
    let (distance_a, distance_b) = (full_distance_m[a], full_distance_m[b]);
    let ratio = if distance_b > distance_a {
        ((full_distance_m[vertex] - distance_a) / (distance_b - distance_a)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (start_m, end_m) = (
        vertex_distances_m.get(at - 1).copied().unwrap_or(0.0),
        vertex_distances_m.get(at).copied().unwrap_or(last),
    );
    start_m + (end_m - start_m) * ratio
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::routing::graph::Coord;
    use crate::domain::routing::graph::synthetic::{EdgeSpec, GraphBuilder};

    /// 15,001頂点の折れ線で、間引いたときの距離の補間を確かめる。
    #[test]
    fn display_source_interpolates_distances() {
        let n = 15_001;
        let mut builder = GraphBuilder::new();
        let start = builder.add_node_deg(35.0, 139.0);
        let end = builder.add_node_deg(35.0, 139.0 + (n - 1) as f64 * 1e-6);
        let shape_points: Vec<Coord> = (1..n - 1)
            .map(|i| {
                Coord::from_latlon(crate::contract::LatLon::new(35.0, 139.0 + i as f64 * 1e-6))
            })
            .collect();
        builder.add_edge(EdgeSpec::new(start, end).shape(&shape_points));
        let graph = builder.build();
        let d = DirEdge::new(0, false);
        let path = Path {
            dedges: vec![d],
            start: crate::domain::routing::snap::SnapPoint::on_edge(&graph, 0, 0),
            end: crate::domain::routing::snap::SnapPoint::on_edge(
                &graph,
                0,
                graph.edge(0).length_dm,
            ),
            time_ds: 0,
            risk_fx: 0,
        };
        let cancel = CancelToken::unbounded();
        let route_shape =
            crate::domain::routing::evaluate::geometry::build_shape(&graph, &path, &cancel)
                .expect("形状を作れる");
        assert!(route_shape.simplified);
        let source = route_shape.display_source.as_ref().expect("間引いた");
        let subdivision =
            crate::domain::corridor::Subdivision::build(&route_shape.display, &cancel)
                .expect("小線分を作れる");
        let vertex_distances_m = crate::domain::navigation::vertex_distances(&subdivision);
        let full_distance_m = full_distances(&route_shape);
        let mut kept = vec![false; route_shape.full.len()];
        for &vertex in source {
            kept[vertex] = true;
        }
        let mut previous = 0.0;
        for vertex in 0..route_shape.full.len() {
            let at_m = at_m_for_vertex(&route_shape, &vertex_distances_m, &full_distance_m, vertex);
            assert!(at_m >= previous, "{vertex}: {at_m} < {previous}");
            previous = at_m;
            if kept[vertex] {
                let k = source.partition_point(|&i| i < vertex);
                assert_eq!(at_m, vertex_distances_m[k], "{vertex}");
            } else {
                let full_m = full_distance_m[vertex];
                assert!((at_m - full_m).abs() <= 1.0, "{vertex}: {at_m} != {full_m}");
            }
        }
    }
}
