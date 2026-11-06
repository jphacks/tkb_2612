//! 評価で共有する幾何の計算（局所平面近似・経路の折れ線・格子の照会）。

use std::collections::BTreeMap;

use crate::contract::Mode;
use crate::contract::geo::{METERS_PER_DEGREE, straight_distance_m};
use crate::domain::routing::graph::{Coord, DirEdge, EdgeId, Graph};
use crate::domain::routing::search::endpoints::{Placement, placement, travelled_lengths_dm};
use crate::domain::routing::snap::OdIndex;

/// 距離が等しいとみなす差（m）。`snap` と同じ扱い。
pub const TIE_EPS_M: f64 = 1e-9;

/// 点を原点にした局所平面（m）。`contract::geo::straight_distance_m` と同じ近似。
#[derive(Debug, Clone, Copy)]
pub struct LocalPlane {
    origin: Coord,
    /// 1e-7度あたりのm（経度方向）。
    mx: f64,
    /// 1e-7度あたりのm（緯度方向）。
    my: f64,
}

impl LocalPlane {
    pub fn new(origin: Coord) -> LocalPlane {
        let my = METERS_PER_DEGREE / 1e7;
        let cos_lat = (f64::from(origin.lat_e7) / 1e7)
            .to_radians()
            .cos()
            .max(0.05);
        LocalPlane {
            origin,
            mx: my * cos_lat,
            my,
        }
    }

    /// 原点からの（東, 北）のm。
    pub fn xy(&self, c: Coord) -> (f64, f64) {
        (
            (f64::from(c.lon_e7) - f64::from(self.origin.lon_e7)) * self.mx,
            (f64::from(c.lat_e7) - f64::from(self.origin.lat_e7)) * self.my,
        )
    }
}

/// 2点間の距離（m）。`contract::geo` と同じ平均緯度の局所近似。
pub fn segment_length_m(a: Coord, b: Coord) -> f64 {
    straight_distance_m(a.to_latlon(), b.to_latlon())
}

/// 折れ線の各線分の長さ（m）。
pub fn segment_lengths_m(line: &[Coord]) -> Vec<f64> {
    line.windows(2)
        .map(|w| segment_length_m(w[0], w[1]))
        .collect()
}

/// 折れ線の長さ（m）。
pub fn polyline_length_m(line: &[Coord]) -> f64 {
    segment_lengths_m(line).iter().sum()
}

/// 点から線分までの距離（m）。点を原点にした局所平面で測る。
pub fn point_segment_distance_m(point: Coord, a: Coord, b: Coord) -> f64 {
    let plane = LocalPlane::new(point);
    let (ax, ay) = plane.xy(a);
    let (bx, by) = plane.xy(b);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (-(ax * dx + ay * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (px, py) = if t <= 0.0 {
        (ax, ay)
    } else if t >= 1.0 {
        (bx, by)
    } else {
        (ax + t * dx, ay + t * dy)
    };
    px.hypot(py)
}

/// 点から折れ線までの距離（m）。折れ線が空なら無限大、1点ならその点との距離。
pub fn distance_to_polyline_m(point: Coord, line: &[Coord]) -> f64 {
    match line {
        [] => f64::INFINITY,
        [only] => straight_distance_m(point.to_latlon(), only.to_latlon()),
        _ => line
            .windows(2)
            .map(|w| point_segment_distance_m(point, w[0], w[1]))
            .fold(f64::INFINITY, f64::min),
    }
}

/// 折れ線上の、点に最も近い位置（距離m、線分番号、線分上の割合）。
/// 距離が同じなら先の線分を返す。
pub fn project_on_polyline(point: Coord, line: &[Coord]) -> Option<(f64, usize, f64)> {
    if line.is_empty() {
        return None;
    }
    if line.len() == 1 {
        return Some((
            straight_distance_m(point.to_latlon(), line[0].to_latlon()),
            0,
            0.0,
        ));
    }
    let plane = LocalPlane::new(point);
    let mut best = (f64::INFINITY, 0_usize, 0.0_f64);
    for (i, w) in line.windows(2).enumerate() {
        let (ax, ay) = plane.xy(w[0]);
        let (bx, by) = plane.xy(w[1]);
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        let t = if len2 > 0.0 {
            (-(ax * dx + ay * dy) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (px, py) = if t <= 0.0 {
            (ax, ay)
        } else if t >= 1.0 {
            (bx, by)
        } else {
            (ax + t * dx, ay + t * dy)
        };
        let dist = px.hypot(py);
        if dist < best.0 - TIE_EPS_M {
            best = (dist, i, t);
        }
    }
    Some(best)
}

/// 折れ線上の、点に最も近い点の方向（進む向きの単位ベクトル）。同じ距離なら先の線分。
pub fn nearest_direction(point: Coord, line: &[Coord]) -> Option<(f64, (f64, f64))> {
    let (dist, seg, _) = project_on_polyline(point, line)?;
    let w = line.get(seg..seg + 2)?;
    let plane = LocalPlane::new(point);
    let (ax, ay) = plane.xy(w[0]);
    let (bx, by) = plane.xy(w[1]);
    let (dx, dy) = (bx - ax, by - ay);
    let norm = dx.hypot(dy);
    if norm <= 0.0 {
        return Some((dist, (0.0, 0.0)));
    }
    Some((dist, (dx / norm, dy / norm)))
}

/// 線分の始点・終点を、割合 `f`（0〜1）で補間する。
fn interpolate(a: Coord, b: Coord, f: f64) -> Coord {
    let lerp = |x: i32, y: i32| (f64::from(x) + f * (f64::from(y) - f64::from(x))).round() as i32;
    Coord::new(lerp(a.lat_e7, b.lat_e7), lerp(a.lon_e7, b.lon_e7))
}

/// 折れ線の、始点側から割合 `f0`〜`f1` の部分。`snap::point_on_edge` と同じ規則で、
/// `offset_dm / length_dm` を折れ線の幾何学的な長さの割合として扱う。
pub fn sub_polyline(line: &[Coord], f0: f64, f1: f64) -> Vec<Coord> {
    if line.is_empty() {
        return Vec::new();
    }
    let lengths = segment_lengths_m(line);
    let total: f64 = lengths.iter().sum();
    if total <= 0.0 || line.len() < 2 {
        return vec![line[0]];
    }
    let point_at = |d: f64| {
        let mut remaining = d.clamp(0.0, total);
        for (i, &len) in lengths.iter().enumerate() {
            if remaining <= len && len > 0.0 {
                return interpolate(line[i], line[i + 1], (remaining / len).clamp(0.0, 1.0));
            }
            remaining -= len;
        }
        line[line.len() - 1]
    };
    let (p0, p1) = (f0.clamp(0.0, 1.0) * total, f1.clamp(0.0, 1.0) * total);
    let mut out = vec![point_at(p0)];
    let mut acc = 0.0;
    for (i, &len) in lengths.iter().enumerate() {
        acc += len;
        if acc > p0 && acc < p1 {
            out.push(line[i + 1]);
        }
    }
    out.push(point_at(p1));
    out
}

/// 経路の各有向辺が通る範囲（辺の始点からの距離dm）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TravelRange {
    pub dedge: DirEdge,
    pub from_dm: u32,
    pub to_dm: u32,
}

/// 経路の出発・到着の部分辺を含む、各有向辺の通る範囲。形が正しくなければ `None`。
///
/// `path_risk`（`risk::arc_cost`）と同じ規則で求める。出発が辺の途中なら最初の辺の
/// 吸着点から終端まで（逆向きなら始点まで）、到着が辺の途中なら最後の辺の始点から
/// 吸着点まで（逆向きなら吸着点から終端まで）。同じ辺の途中から途中へ1本で行く場合は
/// 2つの吸着点のあいだ。それ以外は全長。
pub fn travelled_ranges(
    graph: &Graph,
    start: &crate::domain::routing::snap::SnapPoint,
    end: &crate::domain::routing::snap::SnapPoint,
    dedges: &[DirEdge],
) -> Option<Vec<TravelRange>> {
    let lengths = travelled_lengths_dm(graph, start, end, dedges)?;
    let start_inside = placement(graph, start) == Some(Placement::Inside);
    let end_inside = placement(graph, end) == Some(Placement::Inside);
    let last = dedges.len().checked_sub(1)?;
    let direct = dedges.len() == 1 && start_inside && end_inside;
    let mut out = Vec::with_capacity(dedges.len());
    for (i, &d) in dedges.iter().enumerate() {
        let length = graph.edge(d.edge()).length_dm;
        let (from_dm, to_dm) = if direct {
            (
                start.offset_dm.min(end.offset_dm),
                start.offset_dm.max(end.offset_dm),
            )
        } else if i == 0 && start_inside {
            if d.is_reverse() {
                (0, start.offset_dm)
            } else {
                (start.offset_dm, length)
            }
        } else if i == last && end_inside {
            if d.is_reverse() {
                (end.offset_dm, length)
            } else {
                (0, end.offset_dm)
            }
        } else {
            (0, length)
        };
        debug_assert_eq!(to_dm.saturating_sub(from_dm), lengths[i]);
        out.push(TravelRange {
            dedge: d,
            from_dm,
            to_dm,
        });
    }
    Some(out)
}

/// 経路の折れ線（出発の吸着点から到着の吸着点まで、実際に通る形）。
pub fn path_polyline(
    graph: &Graph,
    start: &crate::domain::routing::snap::SnapPoint,
    end: &crate::domain::routing::snap::SnapPoint,
    dedges: &[DirEdge],
) -> Option<Vec<Coord>> {
    let ranges = travelled_ranges(graph, start, end, dedges)?;
    let mut out: Vec<Coord> = Vec::new();
    for range in ranges {
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
        for c in piece {
            if out.last() != Some(&c) {
                out.push(c);
            }
        }
    }
    Some(out)
}

/// 経路の道路種別（`HighwayClass::code`）ごとの長さ（デシメートル）。0の種別は出さない。
pub fn class_lengths_dm(
    graph: &Graph,
    start: &crate::domain::routing::snap::SnapPoint,
    end: &crate::domain::routing::snap::SnapPoint,
    dedges: &[DirEdge],
) -> Option<BTreeMap<u8, u64>> {
    let lengths = travelled_lengths_dm(graph, start, end, dedges)?;
    let mut out = BTreeMap::new();
    for (d, travelled) in dedges.iter().zip(lengths) {
        *out.entry(graph.edge(d.edge()).class).or_insert(0) += u64::from(travelled);
    }
    Some(out)
}

/// 点のまわりの格子セル（0.001度）の範囲。`reach_m` を緯度・経度の差に直す。
fn cell_range(point: Coord, reach_m: f64) -> ((i64, i64), (i64, i64)) {
    let my = METERS_PER_DEGREE / 1e7;
    let cos_lat = (f64::from(point.lat_e7) / 1e7).to_radians().cos().max(0.05);
    let dlat = reach_m / my;
    let dlon = reach_m / (my * cos_lat);
    let lat_lo = (f64::from(point.lat_e7) - dlat) / 1e7;
    let lat_hi = (f64::from(point.lat_e7) + dlat) / 1e7;
    let lon_lo = (f64::from(point.lon_e7) - dlon) / 1e7;
    let lon_hi = (f64::from(point.lon_e7) + dlon) / 1e7;
    (
        (
            (lat_lo * 1000.0).floor() as i64,
            (lon_lo * 1000.0).floor() as i64,
        ),
        (
            (lat_hi * 1000.0).floor() as i64,
            (lon_hi * 1000.0).floor() as i64,
        ),
    )
}

/// 線分のまわりの格子セル（0.001度）の範囲。`reach_m` を足した外接矩形で決める。
fn segment_cell_range(a: Coord, b: Coord, reach_m: f64) -> ((i64, i64), (i64, i64)) {
    let my = METERS_PER_DEGREE / 1e7;
    let mid_lat = (f64::from(a.lat_e7) + f64::from(b.lat_e7)) / 2.0 / 1e7;
    let cos_lat = mid_lat.to_radians().cos().max(0.05);
    let dlat = reach_m / my;
    let dlon = reach_m / (my * cos_lat);
    let lat_lo = (f64::from(a.lat_e7.min(b.lat_e7)) - dlat) / 1e7;
    let lat_hi = (f64::from(a.lat_e7.max(b.lat_e7)) + dlat) / 1e7;
    let lon_lo = (f64::from(a.lon_e7.min(b.lon_e7)) - dlon) / 1e7;
    let lon_hi = (f64::from(a.lon_e7.max(b.lon_e7)) + dlon) / 1e7;
    (
        (
            (lat_lo * 1000.0).floor() as i64,
            (lon_lo * 1000.0).floor() as i64,
        ),
        (
            (lat_hi * 1000.0).floor() as i64,
            (lon_hi * 1000.0).floor() as i64,
        ),
    )
}

/// 点のまわり `reach_m` 以内の事故を調べるための格子セル。
pub fn cells_around_point(point: Coord, reach_m: f64) -> Vec<(i64, i64)> {
    let ((row_lo, col_lo), (row_hi, col_hi)) = cell_range(point, reach_m);
    let mut cells = Vec::new();
    for row in row_lo..=row_hi {
        for col in col_lo..=col_hi {
            cells.push((row, col));
        }
    }
    cells
}

/// 線分のまわり `reach_m` 以内の事故を調べるための格子セル。
pub fn cells_around_segment(a: Coord, b: Coord, reach_m: f64) -> Vec<(i64, i64)> {
    let ((row_lo, col_lo), (row_hi, col_hi)) = segment_cell_range(a, b, reach_m);
    let mut cells = Vec::new();
    for row in row_lo..=row_hi {
        for col in col_lo..=col_hi {
            cells.push((row, col));
        }
    }
    cells
}

/// 点から `reach_m` 以内にある可能性のある辺（`OdIndex` のセルで絞る。昇順・重複なし）。
pub fn edges_in_window(index: &OdIndex, center: Coord, reach_m: f64, out: &mut Vec<EdgeId>) {
    let p = &index.params;
    let my = METERS_PER_DEGREE / 1e7;
    let cos_lat = (f64::from(center.lat_e7) / 1e7)
        .to_radians()
        .cos()
        .max(0.05);
    let mx = my * cos_lat;
    let dlat = (reach_m / my).ceil() as i64;
    let dlon = (reach_m / mx).ceil() as i64;
    let cell =
        |value: i64, base: i32, size: i32| (value - i64::from(base)).div_euclid(i64::from(size));
    let row_lo = cell(i64::from(center.lat_e7) - dlat, p.lat0_e7, p.cell_lat_e7).max(0);
    let row_hi =
        cell(i64::from(center.lat_e7) + dlat, p.lat0_e7, p.cell_lat_e7).min(i64::from(p.rows) - 1);
    let col_lo = cell(i64::from(center.lon_e7) - dlon, p.lon0_e7, p.cell_lon_e7).max(0);
    let col_hi =
        cell(i64::from(center.lon_e7) + dlon, p.lon0_e7, p.cell_lon_e7).min(i64::from(p.cols) - 1);
    if row_lo > row_hi || col_lo > col_hi {
        return;
    }
    for row in row_lo..=row_hi {
        let first = (row * i64::from(p.cols) + col_lo) as u32;
        let last = (row * i64::from(p.cols) + col_hi) as u32;
        let lo = index.cell_keys.partition_point(|&k| k < first);
        let hi = index.cell_keys.partition_point(|&k| k <= last);
        for i in lo..hi {
            let (a, b) = (index.cell_ofs[i] as usize, index.cell_ofs[i + 1] as usize);
            out.extend_from_slice(&index.cell_edges[a..b]);
        }
    }
    out.sort_unstable();
    out.dedup();
}

/// 線分のまわり `reach_m` 以内にある可能性のある辺（`OdIndex` のセルで絞る。昇順・重複なし）。
pub fn edges_near_polyline(index: &OdIndex, line: &[Coord], reach_m: f64, out: &mut Vec<EdgeId>) {
    let p = &index.params;
    let my = METERS_PER_DEGREE / 1e7;
    for w in line.windows(2) {
        let mid_lat = (f64::from(w[0].lat_e7) + f64::from(w[1].lat_e7)) / 2.0 / 1e7;
        let cos_lat = mid_lat.to_radians().cos().max(0.05);
        let dlat = (reach_m / my).ceil() as i64;
        let dlon = (reach_m / (my * cos_lat)).ceil() as i64;
        let row_lo = ((i64::from(w[0].lat_e7.min(w[1].lat_e7)) - dlat - i64::from(p.lat0_e7))
            .div_euclid(i64::from(p.cell_lat_e7)))
        .max(0);
        let row_hi = ((i64::from(w[0].lat_e7.max(w[1].lat_e7)) + dlat - i64::from(p.lat0_e7))
            .div_euclid(i64::from(p.cell_lat_e7)))
        .min(i64::from(p.rows) - 1);
        let col_lo = ((i64::from(w[0].lon_e7.min(w[1].lon_e7)) - dlon - i64::from(p.lon0_e7))
            .div_euclid(i64::from(p.cell_lon_e7)))
        .max(0);
        let col_hi = ((i64::from(w[0].lon_e7.max(w[1].lon_e7)) + dlon - i64::from(p.lon0_e7))
            .div_euclid(i64::from(p.cell_lon_e7)))
        .min(i64::from(p.cols) - 1);
        if row_lo > row_hi || col_lo > col_hi {
            continue;
        }
        for row in row_lo..=row_hi {
            let first = (row * i64::from(p.cols) + col_lo) as u32;
            let last = (row * i64::from(p.cols) + col_hi) as u32;
            let lo = index.cell_keys.partition_point(|&k| k < first);
            let hi = index.cell_keys.partition_point(|&k| k <= last);
            for i in lo..hi {
                let (a, b) = (index.cell_ofs[i] as usize, index.cell_ofs[i + 1] as usize);
                out.extend_from_slice(&index.cell_edges[a..b]);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
}

/// 辺の上の位置（辺の始点からのdm）に投影した、点に最も近い位置。距離mと位置dm。
pub fn project_on_edge(graph: &Graph, edge: EdgeId, point: Coord) -> Option<(f64, u32)> {
    let line = graph.edge_polyline(edge);
    let (dist, seg, t) = project_on_polyline(point, &line)?;
    let lengths = segment_lengths_m(&line);
    let total: f64 = lengths.iter().sum();
    let length_dm = graph.edge(edge).length_dm;
    let pos_dm = if total > 0.0 {
        let before = lengths[..seg.min(lengths.len())].iter().sum::<f64>()
            + t * lengths.get(seg).copied().unwrap_or(0.0);
        ((before / total * f64::from(length_dm)).round() as u32).min(length_dm)
    } else {
        0
    };
    Some((dist, pos_dm))
}

/// 経路の時間の正本を返す（形が正しくない・通れない辺があれば `None`）。
pub fn path_time_ds(
    graph: &Graph,
    mode: Mode,
    start: &crate::domain::routing::snap::SnapPoint,
    end: &crate::domain::routing::snap::SnapPoint,
    dedges: &[DirEdge],
) -> Option<u64> {
    crate::domain::routing::search::endpoints::path_time_ds(graph, mode, start, end, dedges)
}

/// 経路の道のり（m）。形が正しくなければ `None`。
pub fn path_distance_m(
    graph: &Graph,
    start: &crate::domain::routing::snap::SnapPoint,
    end: &crate::domain::routing::snap::SnapPoint,
    dedges: &[DirEdge],
) -> Option<f64> {
    crate::domain::routing::search::endpoints::path_distance_dm(graph, start, end, dedges)
        .map(|dm| dm as f64 / 10.0)
}
