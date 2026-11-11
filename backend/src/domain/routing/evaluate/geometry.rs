//! 応答の形状（behavior.md 5章「経路の値と形状」）。
//!
//! 出発の吸着点、通った有向辺の形状点とノード、到着の吸着点を並べ、直前と同じ座標を除く。
//! 頂点が15,000個を超えるときだけ、表示用にDouglas–Peucker法で間引く。

use crate::contract::{CancelToken, EngineError, Interrupt, LonLat};
use crate::domain::corridor::{
    CANCEL_CHECK_INTERVAL, Projection, SUBSEGMENT_LENGTH_M, Subdivision,
};
use crate::domain::routing::graph::{Coord, Graph};
use crate::domain::routing::search::Path;
use crate::domain::routing::search::endpoints::{Placement, placement};

/// 応答で許す頂点数の上限（behavior.md 5章）。
pub const MAX_DISPLAY_VERTICES: usize = 15_000;
/// 間引きの最初の許容誤差（m）。
pub const INITIAL_SIMPLIFY_TOLERANCE_M: f64 = 0.5;

/// 経路の形状。`full` は間引く前の座標で、辺・クラスタの事故を置く位置に使う。
#[derive(Debug, Clone)]
pub struct RouteShape {
    /// 間引く前の座標（連続する重複を除いたもの）。
    pub full: Vec<Coord>,
    /// `full` を度に直したもの。
    pub full_latlon: Vec<LonLat>,
    /// 応答に使う座標（間引いた後）。
    pub display: Vec<LonLat>,
    /// 各有向辺が `full` のどの頂点範囲を描いたか（開始・終了を含む）。
    pub ranges: Vec<(usize, usize)>,
    /// `full` の局所投影。辺・クラスタの位置と間引きの距離に使う。
    pub projection: Projection,
    /// 表示用の形状を間引いたか。
    pub simplified: bool,
    /// 間引いたときの、`display` の各頂点に対応する `full` の頂点番号。
    /// 間引いていなければ `None`。
    pub display_source: Option<Vec<usize>>,
}

impl RouteShape {
    /// 有向辺iの終点（内部ノード）の `full` での頂点番号。i = 0..n−2 で使う。
    pub fn internal_node_vertex(&self, dedge_index: usize) -> Option<usize> {
        self.ranges.get(dedge_index).map(|&(_, end)| end)
    }
}

/// 2頂点間の点（`t` は0〜1）。`snap::point_on_edge` と同じく1e-7度に四捨五入する。
fn interpolate(a: Coord, b: Coord, t: f64) -> Coord {
    if t <= 0.0 {
        return a;
    }
    if t >= 1.0 {
        return b;
    }
    let lerp = |x: i32, y: i32| (f64::from(x) + t * (f64::from(y) - f64::from(x))).round() as i32;
    Coord::new(lerp(a.lat_e7, b.lat_e7), lerp(a.lon_e7, b.lon_e7))
}

/// 辺の折れ線の、始点側から割合 `f0`〜`f1` の部分（`f0 ≤ f1`）。
/// 割合は `snap::point_on_edge` と同じく、折れ線の幾何学的な長さに対する割合とする。
pub fn sub_polyline(line: &[Coord], f0: f64, f1: f64) -> Vec<Coord> {
    let lengths: Vec<f64> = line
        .windows(2)
        .map(|w| crate::contract::geo::straight_distance_m(w[0].to_latlon(), w[1].to_latlon()))
        .collect();
    let total: f64 = lengths.iter().sum();
    if total <= 0.0 || line.len() < 2 {
        return vec![line[0]];
    }
    // 折れ線の始点からの距離 `d` の点
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

/// 直前と同じ座標を除きながら、部分折れ線を `out` に足す。足した範囲（開始・終了の頂点番号）を返す。
fn append_line(out: &mut Vec<Coord>, line: &[Coord]) -> (usize, usize) {
    let Some(&first) = line.first() else {
        let at = out.len().saturating_sub(1);
        return (at, at);
    };
    let start = if out.last() == Some(&first) {
        out.len() - 1
    } else {
        out.len()
    };
    for &coord in line {
        if out.last() != Some(&coord) {
            out.push(coord);
        }
    }
    let end = out.len() - 1;
    (start, end)
}

/// 経路の座標を作る（behavior.md 5章）。
pub fn build_shape(
    graph: &Graph,
    path: &Path,
    cancel: &CancelToken,
) -> Result<RouteShape, EngineError> {
    if path.dedges.is_empty() {
        return Err(EngineError::ContractViolation(
            "経路の有向辺が空です".to_owned(),
        ));
    }
    let start_inside = placement(graph, &path.start) == Some(Placement::Inside);
    let end_inside = placement(graph, &path.end) == Some(Placement::Inside);
    let last = path.dedges.len() - 1;
    let mut full: Vec<Coord> = Vec::new();
    let mut ranges = Vec::with_capacity(path.dedges.len());
    for (i, &d) in path.dedges.iter().enumerate() {
        if i % CANCEL_CHECK_INTERVAL == 0 {
            cancel.check()?;
        }
        let edge = d.edge();
        let length = f64::from(graph.edge(edge).length_dm).max(1.0);
        let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
        if i == 0 && start_inside {
            let f = f64::from(path.start.offset_dm) / length;
            if d.is_reverse() {
                hi = f;
            } else {
                lo = f;
            }
        }
        if i == last && end_inside {
            let f = f64::from(path.end.offset_dm) / length;
            if d.is_reverse() {
                lo = f;
            } else {
                hi = f;
            }
        }
        let mut piece = sub_polyline(&graph.edge_polyline(edge), lo.min(hi), hi.max(lo));
        if d.is_reverse() {
            piece.reverse();
        }
        ranges.push(append_line(&mut full, &piece));
    }
    if full.len() < 2 {
        return Err(EngineError::ContractViolation(
            "経路の形状が2頂点未満です".to_owned(),
        ));
    }
    let full_latlon: Vec<LonLat> = full.iter().map(|c| LonLat::from(c.to_latlon())).collect();
    let projection = Projection::for_path(&full_latlon);
    let (display, display_source) = display_coordinates_with_source(&full, &projection, cancel)?;
    if display.len() < 2 {
        return Err(EngineError::ContractViolation(
            "間引いた経路の形状が2頂点未満です".to_owned(),
        ));
    }
    Ok(RouteShape {
        full,
        full_latlon,
        display,
        ranges,
        projection,
        simplified: display_source.is_some(),
        display_source,
    })
}

/// 表示用の座標。15,000頂点以下ならそのまま（間引かない）、超えるときだけ
/// Douglas–Peucker法で15,000頂点以下にする。
pub fn display_coordinates(
    full: &[Coord],
    projection: &Projection,
    cancel: &CancelToken,
) -> Result<(Vec<LonLat>, bool), Interrupt> {
    let (display, source) = display_coordinates_with_source(full, projection, cancel)?;
    Ok((display, source.is_some()))
}

/// [`display_coordinates`] と同じ結果に、間引いたときの元の頂点番号を添えて返す。
fn display_coordinates_with_source(
    full: &[Coord],
    projection: &Projection,
    cancel: &CancelToken,
) -> Result<(Vec<LonLat>, Option<Vec<usize>>), Interrupt> {
    let full_latlon: Vec<LonLat> = full.iter().map(|c| LonLat::from(c.to_latlon())).collect();
    if full.len() <= MAX_DISPLAY_VERTICES {
        return Ok((full_latlon, None));
    }
    let (display, source) = simplify_to_limit_with_source(full, projection, cancel)?;
    Ok((display, Some(source)))
}

/// 両端を残すDouglas–Peucker法。許容誤差0.5mから2倍ずつ大きくし、15,000頂点以下になった
/// 最初の結果と、残した頂点の番号を返す。距離は `projection` の局所平面で測る。
fn simplify_to_limit_with_source(
    coordinates: &[Coord],
    projection: &Projection,
    cancel: &CancelToken,
) -> Result<(Vec<LonLat>, Vec<usize>), Interrupt> {
    let mut tolerance = INITIAL_SIMPLIFY_TOLERANCE_M;
    loop {
        let kept = douglas_peucker_indices(coordinates, projection, tolerance, cancel)?;
        if kept.len() <= MAX_DISPLAY_VERTICES || kept.len() <= 2 {
            let display = kept
                .iter()
                .map(|&i| LonLat::from(coordinates[i].to_latlon()))
                .collect();
            return Ok((display, kept));
        }
        tolerance *= 2.0;
    }
}

#[cfg(test)]
fn simplify_to_limit(
    coordinates: &[Coord],
    projection: &Projection,
    cancel: &CancelToken,
) -> Result<Vec<LonLat>, Interrupt> {
    simplify_to_limit_with_source(coordinates, projection, cancel).map(|(display, _)| display)
}

/// 点 `p` から線分 `a`〜`b` までの距離（m）。
fn point_segment_distance(projection: &Projection, p: Coord, a: Coord, b: Coord) -> f64 {
    let (px, py) = projection.xy(LonLat::from(p.to_latlon()));
    let (ax, ay) = projection.xy(LonLat::from(a.to_latlon()));
    let (bx, by) = projection.xy(LonLat::from(b.to_latlon()));
    let (dx, dy) = (bx - ax, by - ay);
    let dd = dx * dx + dy * dy;
    let t = if dd > 0.0 {
        (((px - ax) * dx + (py - ay) * dy) / dd).clamp(0.0, 1.0)
    } else {
        0.0
    };
    ((px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)).sqrt()
}

/// 両端を残すDouglas–Peucker法。`keep` の立った頂点の番号を元の順に返す。
fn douglas_peucker_indices(
    coordinates: &[Coord],
    projection: &Projection,
    tolerance: f64,
    cancel: &CancelToken,
) -> Result<Vec<usize>, Interrupt> {
    let n = coordinates.len();
    if n <= 2 {
        return Ok((0..n).collect());
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0_usize, n - 1)];
    let mut work = 0_usize;
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        work += b - a;
        if work / CANCEL_CHECK_INTERVAL != (work - (b - a)) / CANCEL_CHECK_INTERVAL {
            cancel.check()?;
        }
        let mut best = 0.0_f64;
        let mut best_index = a;
        for i in a + 1..b {
            let distance =
                point_segment_distance(projection, coordinates[i], coordinates[a], coordinates[b]);
            if distance > best {
                best = distance;
                best_index = i;
            }
        }
        if best > tolerance {
            keep[best_index] = true;
            stack.push((a, best_index));
            stack.push((best_index, b));
        }
    }
    Ok((0..n).filter(|&i| keep[i]).collect())
}

#[cfg(test)]
fn douglas_peucker(
    coordinates: &[Coord],
    projection: &Projection,
    tolerance: f64,
    cancel: &CancelToken,
) -> Result<Vec<Coord>, Interrupt> {
    let kept = douglas_peucker_indices(coordinates, projection, tolerance, cancel)?;
    Ok(kept.into_iter().map(|i| coordinates[i]).collect())
}

/// 75m分割の、元の辺1本あたりの小線分数。
pub fn subsegment_count(length_m: f64) -> u32 {
    ((length_m / SUBSEGMENT_LENGTH_M).ceil() as u32).max(1)
}

/// 元の各線分の最初のsid。長さは線分数+1で、末尾は小線分の総数。
pub fn segment_bases(subdivision: &Subdivision) -> Vec<u32> {
    let mut bases = Vec::with_capacity(subdivision.edge_lengths.len() + 1);
    let mut sid = 0_u32;
    for &length in &subdivision.edge_lengths {
        bases.push(sid);
        sid += subsegment_count(length);
    }
    bases.push(sid);
    bases
}

/// 間引いていない形状で、元の線分 `j` の中の割合 `t` が属するsidを返す。
/// `bases` は [`segment_bases`] の値。
pub fn sid_at_segment(bases: &[u32], segment_index: usize, t: f64) -> Option<u32> {
    if segment_index + 1 >= bases.len() {
        return None;
    }
    let base = bases[segment_index];
    let n = bases[segment_index + 1] - base;
    let k = ((t.clamp(0.0, 1.0) * f64::from(n)).floor() as u32).min(n - 1);
    Some(base + k)
}

/// 点Pから最寄りのsid（同じ距離なら小さいsid）。全小線分と比べる。
pub fn nearest_sid(
    subdivision: &Subdivision,
    p: LonLat,
    cancel: &CancelToken,
) -> Result<u32, Interrupt> {
    let xy = subdivision.projection.xy(p);
    let mut best: Option<(f64, u32)> = None;
    for (sid, segment) in subdivision.segments.iter().enumerate() {
        if sid % CANCEL_CHECK_INTERVAL == 0 {
            cancel.check()?;
        }
        let d2 = segment.distance_sq(xy);
        if best.is_none_or(|(b, _)| d2 < b) {
            best = Some((d2, sid as u32));
        }
    }
    Ok(best.map_or(0, |(_, sid)| sid))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coord(lat: f64, lon: f64) -> Coord {
        Coord::from_latlon(crate::contract::LatLon::new(lat, lon))
    }

    #[test]
    fn sub_polyline_cuts_by_length_fraction() {
        let a = coord(35.0, 139.0);
        let b = coord(35.0, 139.001);
        let c = coord(35.0, 139.004);
        let line = [a, b, c];
        assert_eq!(sub_polyline(&line, 0.0, 1.0), vec![a, b, c]);
        let part = sub_polyline(&line, 0.125, 0.375);
        assert_eq!(part.len(), 3);
        assert_eq!(part[1], b);
        assert!(part[0].lon_e7 > a.lon_e7 && part[0].lon_e7 < b.lon_e7);
        assert!(part[2].lon_e7 > b.lon_e7 && part[2].lon_e7 < c.lon_e7);
    }

    #[test]
    fn douglas_peucker_keeps_endpoints_and_drops_collinear_points() {
        let points: Vec<Coord> = (0..1000)
            .map(|i| coord(35.0, 139.0 + f64::from(i) * 1e-6))
            .collect();
        let latlon: Vec<LonLat> = points.iter().map(|c| LonLat::from(c.to_latlon())).collect();
        let projection = Projection::for_path(&latlon);
        let simplified = douglas_peucker(&points, &projection, 0.5, &CancelToken::unbounded())
            .expect("期限なし");
        assert_eq!(simplified.len(), 2);
        assert_eq!(simplified[0], points[0]);
        assert_eq!(simplified[1], points[points.len() - 1]);
    }

    #[test]
    fn exactly_the_limit_is_not_simplified() {
        let points: Vec<Coord> = (0..MAX_DISPLAY_VERTICES)
            .map(|i| coord(35.0, 139.0 + i as f64 * 1e-7))
            .collect();
        let latlon: Vec<LonLat> = points.iter().map(|c| LonLat::from(c.to_latlon())).collect();
        let projection = Projection::for_path(&latlon);
        let (display, simplified) =
            display_coordinates(&points, &projection, &CancelToken::unbounded()).expect("期限なし");
        assert!(!simplified);
        assert_eq!(display.len(), MAX_DISPLAY_VERTICES);
    }

    #[test]
    fn simplify_limit_keeps_straight_line() {
        let points: Vec<Coord> = (0..16_001)
            .map(|i| coord(35.0, 139.0 + i as f64 * 1e-7))
            .collect();
        let latlon: Vec<LonLat> = points.iter().map(|c| LonLat::from(c.to_latlon())).collect();
        let projection = Projection::for_path(&latlon);
        let simplified =
            simplify_to_limit(&points, &projection, &CancelToken::unbounded()).expect("期限なし");
        assert!(simplified.len() <= MAX_DISPLAY_VERTICES);
        assert!(simplified.len() >= 2);
        assert_eq!(simplified[0], latlon[0]);
        assert_eq!(simplified[simplified.len() - 1], latlon[latlon.len() - 1]);
    }
}
