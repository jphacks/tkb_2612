//! 現行方式（OSRMの候補）の経路の折れ線を、自前のグラフの有向辺の列に写す（M5.4）。
//!
//! 折れ線の始点・終点を `snap` で吸着し、折れ線から `corridor_m` 以内に辺の折れ線の点の
//! 80%以上が入る有向辺を「回廊の辺」とする。回廊の辺は辺の時間、それ以外は辺の時間の20倍を
//! コストにして、ノード単位のDijkstraで始点から終点への最短経路を求める。右左折禁止と
//! 右左折コストは考えない。出発・到着の部分辺は `Path` の規則に合わせる。

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};

use crate::contract::Mode;
use crate::domain::routing::NONE;
use crate::domain::routing::eval::geometry::{
    class_lengths_dm, distance_to_polyline_m, edges_near_polyline, path_distance_m, path_polyline,
    path_time_ds, segment_length_m,
};
use crate::domain::routing::graph::time::partial_edge_time_ds;
use crate::domain::routing::graph::{Coord, DirEdge, Graph, NodeId};
use crate::domain::routing::search::Path;
use crate::domain::routing::search::endpoints::{Endpoints, Placement};
use crate::domain::routing::snap::{OdIndex, snap};
use crate::domain::routing::topology::Topology;

/// 写した経路と、元の折れ線とのずれ。
#[derive(Debug, Clone)]
pub struct MatchedRoute {
    /// 自前のグラフの経路（時間は `path_time_ds` の正本）。
    pub path: Path,
    /// 写した経路のうち、元の折れ線から `corridor_m` より離れた部分の長さ（m）。
    pub outside_m: f64,
    /// 写した経路の点と元の折れ線の最大の距離（m）。
    pub max_dev_m: f64,
}

/// 現行方式の折れ線を、自前のグラフの経路に写す。
///
/// `coordinates` は経路の折れ線（始点から終点の順）。写せなければ理由を返す。
pub fn match_route(
    graph: &Graph,
    topology: &Topology,
    index: &OdIndex,
    mode: Mode,
    coordinates: &[Coord],
    corridor_m: f64,
) -> Result<MatchedRoute, String> {
    if coordinates.len() < 2 {
        return Err("座標が2点未満です".to_owned());
    }
    if !corridor_m.is_finite() || corridor_m <= 0.0 {
        return Err(format!("回廊の幅が正の数ではありません: {corridor_m}"));
    }
    let start = snap(graph, topology, index, coordinates[0].to_latlon(), mode)
        .ok_or_else(|| "始点を吸着できません".to_owned())?;
    let end = snap(
        graph,
        topology,
        index,
        coordinates[coordinates.len() - 1].to_latlon(),
        mode,
    )
    .ok_or_else(|| "終点を吸着できません".to_owned())?;
    let endpoints = Endpoints::new(graph, mode, &start, &end)
        .ok_or_else(|| "始点と終点が同じ位置です".to_owned())?;

    let corridor = corridor_dedges(graph, index, mode, coordinates, corridor_m);
    let dedges = shortest_with_corridor(graph, mode, &endpoints, &corridor)
        .ok_or_else(|| "経路が見つかりません".to_owned())?;
    let time_ds = path_time_ds(graph, mode, &start, &end, &dedges)
        .ok_or_else(|| "経路の時間を計算できません".to_owned())?;
    let line = path_polyline(graph, &start, &end, &dedges)
        .ok_or_else(|| "経路の形が正しくありません".to_owned())?;
    let (outside_m, max_dev_m) = deviation(&line, coordinates, corridor_m);
    Ok(MatchedRoute {
        path: Path {
            dedges,
            start,
            end,
            time_ds,
            risk_fx: 0,
        },
        outside_m,
        max_dev_m,
    })
}

/// 経路の道路種別ごとの長さ（m）。0の種別は出さない。`path` の形が正しくなければ `None`。
pub fn class_lengths_m(graph: &Graph, path: &Path) -> Option<std::collections::BTreeMap<u8, f64>> {
    let dm = class_lengths_dm(graph, &path.start, &path.end, &path.dedges)?;
    Some(
        dm.into_iter()
            .map(|(class, dm)| (class, dm as f64 / 10.0))
            .collect(),
    )
}

/// 経路の道のり（m）。形が正しくなければ `None`。
pub fn distance_m(graph: &Graph, path: &Path) -> Option<f64> {
    path_distance_m(graph, &path.start, &path.end, &path.dedges)
}

/// 折れ線の各線分を10mおきに補った点と、その点での進行方向（同じ線分の両端）。
fn sample_with_direction(line: &[Coord]) -> Vec<(Coord, Coord, Coord)> {
    let mut out: Vec<(Coord, Coord, Coord)> = Vec::new();
    for w in line.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = segment_length_m(a, b);
        if len <= 0.0 {
            continue;
        }
        let pieces = (len / 10.0).ceil().max(1.0) as usize;
        for k in 0..=pieces {
            let t = k as f64 / pieces as f64;
            let lerp =
                |x: i32, y: i32| (f64::from(x) + t * (f64::from(y) - f64::from(x))).round() as i32;
            let point = Coord::new(lerp(a.lat_e7, b.lat_e7), lerp(a.lon_e7, b.lon_e7));
            if out.last().is_some_and(|(p, _, _)| *p == point) {
                continue;
            }
            out.push((point, a, b));
        }
    }
    out
}

/// 有向辺 `d` が回廊の辺か。辺の折れ線の点（10mおきに補う）の80%以上が折れ線から
/// `corridor_m` 以内にあり、その点での辺の向きと折れ線の向きの差が120°以下であること。
fn in_corridor(graph: &Graph, d: DirEdge, route: &[Coord], corridor_m: f64) -> bool {
    let line = graph.dedge_polyline(d);
    let samples = sample_with_direction(&line);
    if samples.is_empty() {
        return false;
    }
    let mut inside = 0_usize;
    for (point, a, b) in &samples {
        let Some((dist, route_dir)) =
            crate::domain::routing::eval::geometry::nearest_direction(*point, route)
        else {
            continue;
        };
        if dist > corridor_m {
            continue;
        }
        let plane = crate::domain::routing::eval::geometry::LocalPlane::new(*point);
        let (ax, ay) = plane.xy(*a);
        let (bx, by) = plane.xy(*b);
        let (dx, dy) = (bx - ax, by - ay);
        let norm = dx.hypot(dy);
        if norm <= 0.0 {
            continue;
        }
        let cos = (dx / norm) * route_dir.0 + (dy / norm) * route_dir.1;
        // 120°以下（cos ≥ -1/2）。逆向きの一方通行を回廊にしない。
        if cos >= -0.5 - 1e-12 {
            inside += 1;
        }
    }
    inside * 5 >= samples.len() * 4
}

/// 回廊の有向辺（折れ線の近くの辺だけを `OdIndex` で絞って調べる）。
fn corridor_dedges(
    graph: &Graph,
    index: &OdIndex,
    mode: Mode,
    route: &[Coord],
    corridor_m: f64,
) -> HashSet<DirEdge> {
    let mut candidates = Vec::new();
    edges_near_polyline(index, route, corridor_m, &mut candidates);
    let mut corridor = HashSet::new();
    for edge in candidates {
        for reverse in [false, true] {
            let d = DirEdge::new(edge, reverse);
            if graph.passable(d, mode) && in_corridor(graph, d, route, corridor_m) {
                corridor.insert(d);
            }
        }
    }
    corridor
}

/// 回廊の辺は辺の時間、それ以外は辺の時間の20倍をコストにする。
fn arc_cost(
    graph: &Graph,
    mode: Mode,
    corridor: &HashSet<DirEdge>,
    d: DirEdge,
    travelled_dm: u32,
) -> Option<u64> {
    let base = u64::from(partial_edge_time_ds(graph, d, mode, travelled_dm)?);
    let factor = if corridor.contains(&d) { 1 } else { 20 };
    Some(base * factor)
}

/// 経路の終わり方。
enum EndChoice {
    /// 同じ辺の途中から途中へ1本で行く。
    Direct(DirEdge),
    /// ノードに着く。
    Node(NodeId),
    /// 到着の部分辺を通る（その手前のノード、最後の有向辺）。
    Arrival(NodeId, DirEdge),
}

fn state_allowed(graph: &Graph, mode: Mode, node: NodeId, end_node: Option<NodeId>) -> bool {
    !graph.node_blocked(node, mode) || Some(node) == end_node
}

/// ノード単位のDijkstra。出発・到着の部分辺は `Endpoints` の選択肢を使う。
fn shortest_with_corridor(
    graph: &Graph,
    mode: Mode,
    endpoints: &Endpoints,
    corridor: &HashSet<DirEdge>,
) -> Option<Vec<DirEdge>> {
    let end_node = endpoints.end_node();
    let end_inside = endpoints.end_placement == Placement::Inside;
    let n = graph.node_count();
    let mut dist = vec![u64::MAX; n];
    let mut prev: Vec<Option<(NodeId, DirEdge)>> = vec![None; n];
    let mut heap: BinaryHeap<Reverse<(u64, NodeId)>> = BinaryHeap::new();

    let mut best_end: Option<(u64, EndChoice)> = None;
    if let Some(direct) = endpoints.direct
        && let Some(cost) = arc_cost(graph, mode, corridor, direct.dedge, direct.travelled_dm)
    {
        best_end = Some((cost, EndChoice::Direct(direct.dedge)));
    }
    // 出発がノード上なら、到着の部分辺だけを通る経路（前の辺がない）も選べる。
    if let Some(start_node) = endpoints.start_node() {
        for arrival in &endpoints.arrivals {
            if arrival.from != start_node {
                continue;
            }
            let Some(cost) = arc_cost(graph, mode, corridor, arrival.dedge, arrival.travelled_dm)
            else {
                continue;
            };
            if best_end.as_ref().is_none_or(|(best, _)| cost < *best) {
                best_end = Some((cost, EndChoice::Arrival(start_node, arrival.dedge)));
            }
        }
    }

    for departure in &endpoints.departures {
        let Some(cost) = arc_cost(
            graph,
            mode,
            corridor,
            departure.dedge,
            departure.travelled_dm,
        ) else {
            continue;
        };
        let to = departure.to;
        if !state_allowed(graph, mode, to, end_node) {
            continue;
        }
        if cost < dist[to as usize] {
            dist[to as usize] = cost;
            prev[to as usize] = Some((NONE, departure.dedge));
            heap.push(Reverse((cost, to)));
        }
    }

    while let Some(Reverse((cost, node))) = heap.pop() {
        if cost != dist[node as usize] {
            continue;
        }
        if best_end.as_ref().is_some_and(|(best, _)| cost >= *best) {
            break;
        }
        if !end_inside && Some(node) == end_node {
            best_end = Some((cost, EndChoice::Node(node)));
            break;
        }
        if end_inside {
            for arrival in &endpoints.arrivals {
                if arrival.from != node {
                    continue;
                }
                let Some(partial) =
                    arc_cost(graph, mode, corridor, arrival.dedge, arrival.travelled_dm)
                else {
                    continue;
                };
                let total = cost + partial;
                if best_end.as_ref().is_none_or(|(best, _)| total < *best) {
                    best_end = Some((total, EndChoice::Arrival(node, arrival.dedge)));
                }
            }
        }
        for &d in graph.out_dedges(node) {
            if !graph.passable(d, mode) {
                continue;
            }
            let to = graph.dedge_to(d);
            if !state_allowed(graph, mode, to, end_node) {
                continue;
            }
            let Some(step) = arc_cost(graph, mode, corridor, d, graph.edge(d.edge()).length_dm)
            else {
                continue;
            };
            let next = cost + step;
            if next < dist[to as usize] {
                dist[to as usize] = next;
                prev[to as usize] = Some((node, d));
                heap.push(Reverse((next, to)));
            }
        }
    }

    let (_, choice) = best_end?;
    let mut dedges = Vec::new();
    match choice {
        EndChoice::Direct(d) => dedges.push(d),
        EndChoice::Node(node) => append_prev(node, &prev, &mut dedges),
        EndChoice::Arrival(from, d) => {
            dedges.push(d);
            append_prev(from, &prev, &mut dedges);
        }
    }
    dedges.reverse();
    if dedges.is_empty() {
        None
    } else {
        Some(dedges)
    }
}

/// ノードに至るまでの有向辺を、終点側から `out` に積む。
fn append_prev(mut node: NodeId, prev: &[Option<(NodeId, DirEdge)>], out: &mut Vec<DirEdge>) {
    while let Some((before, d)) = prev[node as usize] {
        out.push(d);
        if before == NONE {
            break;
        }
        node = before;
    }
}

/// 写した経路のうち元の折れ線から `corridor_m` より離れた部分の長さと、最大の距離。
fn deviation(line: &[Coord], route: &[Coord], corridor_m: f64) -> (f64, f64) {
    let mut outside = 0.0_f64;
    let mut max_dev = 0.0_f64;
    for w in line.windows(2) {
        let len = segment_length_m(w[0], w[1]);
        let pieces = (len / 10.0).ceil().max(1.0) as usize;
        for k in 0..pieces {
            let t = (k as f64 + 0.5) / pieces as f64;
            let lerp =
                |x: i32, y: i32| (f64::from(x) + t * (f64::from(y) - f64::from(x))).round() as i32;
            let point = Coord::new(
                lerp(w[0].lat_e7, w[1].lat_e7),
                lerp(w[0].lon_e7, w[1].lon_e7),
            );
            let dist = distance_to_polyline_m(point, route);
            max_dev = max_dev.max(dist);
            if dist > corridor_m {
                outside += len / pieces as f64;
            }
        }
    }
    for &point in line {
        max_dev = max_dev.max(distance_to_polyline_m(point, route));
    }
    (outside, max_dev)
}

/// 経路の時間の正本（テスト用の再計算）。
pub fn canonical_time_ds(graph: &Graph, mode: Mode, path: &Path) -> Option<u64> {
    path_time_ds(graph, mode, &path.start, &path.end, &path.dedges)
}

/// 経路の点の数（テスト用）。
pub fn path_point_count(graph: &Graph, path: &Path) -> Option<usize> {
    path_polyline(graph, &path.start, &path.end, &path.dedges).map(|line| line.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_include_both_ends_and_follow_direction() {
        let a = Coord::new(350_000_000, 1_390_000_000);
        let b = Coord::new(350_000_000, 1_390_000_500);
        let samples = sample_with_direction(&[a, b]);
        assert!(samples.len() >= 2);
        assert_eq!(samples[0].0, a);
        assert_eq!(samples[samples.len() - 1].0, b);
        // 東向き（経度が増える）
        assert!(samples[0].2.lon_e7 > samples[0].1.lon_e7);
    }
}
