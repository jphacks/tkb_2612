//! 出発地・目的地を辺に吸着する（形式書11章）。

use serde::{Deserialize, Serialize};

use crate::contract::geo::{METERS_PER_DEGREE, straight_distance_m};
use crate::contract::{LatLon, Mode};
use crate::domain::routing::SNAP_MAX_DISTANCE_M;
use crate::domain::routing::graph::{Coord, EdgeId, Graph, access};
use crate::domain::routing::topology::Topology;

/// `odindex.bin` の `PARAMS`（形式書11章）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OdIndexParams {
    /// 格子の原点（日本矩形の南西端 20°・122°）。
    pub lat0_e7: i32,
    pub lon0_e7: i32,
    /// セルの大きさ（初期値はどちらも25,000＝0.0025度）。
    pub cell_lat_e7: i32,
    pub cell_lon_e7: i32,
    pub rows: u32,
    pub cols: u32,
}

/// `odindex.bin` の中身：吸着のための格子。
#[derive(Debug, Clone, PartialEq)]
pub struct OdIndex {
    pub params: OdIndexParams,
    /// `PARAMS` のJSON全体（`params` に入らないキーも保つ）。
    pub raw_params: serde_json::Value,
    /// `CELLKEY`：空でないセルの番号 `row×cols + col` の昇順。長さK。
    pub cell_keys: Vec<u32>,
    /// `CELLOFS`：各セルの辺の開始位置。長さK+1。
    pub cell_ofs: Vec<u32>,
    /// `CELLEDG`：セルを通る辺の番号（セル内は昇順）。辺の折れ線のいずれかの線分が通るセルすべて。
    pub cell_edges: Vec<EdgeId>,
}

impl OdIndex {
    /// 座標のセル番号 `row×cols + col`。格子の外は `None`。
    /// `row = floor((lat_e7 − lat0_e7) / cell_lat_e7)`、`col` も同様。
    pub fn cell_key(&self, coord: Coord) -> Option<u32> {
        let p = &self.params;
        let dlat = i64::from(coord.lat_e7) - i64::from(p.lat0_e7);
        let dlon = i64::from(coord.lon_e7) - i64::from(p.lon0_e7);
        if dlat < 0 || dlon < 0 {
            return None;
        }
        let row = dlat / i64::from(p.cell_lat_e7);
        let col = dlon / i64::from(p.cell_lon_e7);
        if row >= i64::from(p.rows) || col >= i64::from(p.cols) {
            return None;
        }
        Some((row * i64::from(p.cols) + col) as u32)
    }

    /// セルを通る辺（昇順）。空のセルは空のスライス。
    pub fn edges_in_cell(&self, key: u32) -> &[EdgeId] {
        match self.cell_keys.binary_search(&key) {
            Ok(i) => &self.cell_edges[self.cell_ofs[i] as usize..self.cell_ofs[i + 1] as usize],
            Err(_) => &[],
        }
    }
}

/// 吸着した点（形式書11章）。辺番号と始点からの距離で表す仮想ノードとして探索に入れる。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapPoint {
    pub edge: EdgeId,
    /// 辺の始点からの距離（デシメートル、0〜`length_dm`）。
    pub offset_dm: u32,
    /// 吸着した辺上の点。
    pub position: Coord,
    /// 元の点からの距離（m、`contract::geo` と同じ局所平面近似）。
    pub distance_m: f64,
}

impl SnapPoint {
    /// 辺 `edge` の始点から `offset_dm` の点（`offset_dm` は辺の長さに切る）。元の点からの距離は0。
    /// テストや、座標から吸着せずに位置を指定する呼び出し側のための組み立て。
    pub fn on_edge(graph: &Graph, edge: EdgeId, offset_dm: u32) -> SnapPoint {
        let offset_dm = offset_dm.min(graph.edge(edge).length_dm);
        SnapPoint {
            edge,
            offset_dm,
            position: point_on_edge(graph, edge, offset_dm),
            distance_m: 0.0,
        }
    }
}

/// 辺の折れ線の線分の長さ（m）。辺の長さ `length_dm` の定義（形式書1章）と同じ `straight_distance_m` の和。
fn segment_lengths_m(line: &[Coord]) -> Vec<f64> {
    line.windows(2)
        .map(|w| straight_distance_m(w[0].to_latlon(), w[1].to_latlon()))
        .collect()
}

/// 辺の始点から `offset_dm`（0〜`length_dm`）進んだ点の座標。
///
/// `offset_dm / length_dm` を折れ線の幾何学的な長さの割合として扱う（`length_dm` は丸めた値なので、
/// 折れ線の長さとは1dm未満の差がある）。折れ線のない（長さ0の）辺は始点。
pub fn point_on_edge(graph: &Graph, edge: EdgeId, offset_dm: u32) -> Coord {
    let line = graph.edge_polyline(edge);
    let length_dm = graph.edge(edge).length_dm;
    if offset_dm == 0 {
        return line[0];
    }
    if offset_dm >= length_dm {
        return line[line.len() - 1];
    }
    let lengths = segment_lengths_m(&line);
    let total: f64 = lengths.iter().sum();
    if total <= 0.0 {
        return line[0];
    }
    let mut remaining = f64::from(offset_dm) / f64::from(length_dm) * total;
    for (i, &len) in lengths.iter().enumerate() {
        if remaining <= len && len > 0.0 {
            let t = (remaining / len).clamp(0.0, 1.0);
            return interpolate(line[i], line[i + 1], t);
        }
        remaining -= len;
    }
    line[line.len() - 1]
}

/// `a` から `b` へ、割合 `t`（0〜1）の点。1e-7度に四捨五入する。両端はそのまま返す。
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

/// 吸着する点を原点にした局所平面（m）。経度の差には、その点の緯度の余弦を掛ける
/// （`contract::geo` と同じ局所平面近似。2点の平均緯度ではなく原点の緯度を使うので、
/// 同じ点を共有する辺どうしの距離が厳密に比べられる）。
struct Plane {
    origin: Coord,
    /// 1e-7度あたりのm（経度方向）。
    mx: f64,
    /// 1e-7度あたりのm（緯度方向）。
    my: f64,
}

impl Plane {
    fn new(origin: Coord) -> Plane {
        let my = METERS_PER_DEGREE / 1e7;
        // 緯度が極端な入力でも、窓が無限に広がらないように余弦に下限を置く。
        let cos_lat = (f64::from(origin.lat_e7) / 1e7)
            .to_radians()
            .cos()
            .max(0.05);
        Plane {
            origin,
            mx: my * cos_lat,
            my,
        }
    }

    /// 原点からの（東, 北）のm。
    fn xy(&self, c: Coord) -> (f64, f64) {
        (
            (f64::from(c.lon_e7) - f64::from(self.origin.lon_e7)) * self.mx,
            (f64::from(c.lat_e7) - f64::from(self.origin.lat_e7)) * self.my,
        )
    }
}

/// 折れ線上の、原点に最も近い点。
struct Hit {
    /// 原点からの距離（m）。
    dist: f64,
    /// 最寄りの点がある線分の番号（折れ線の `seg` 番目と `seg+1` 番目の間）。
    seg: usize,
    /// 線分上の割合（0〜1）。
    t: f64,
}

/// 折れ線の各線分について、原点に最も近い点を調べ、最小のものを返す。距離が同じなら先の線分。
/// 線分の両端に最寄りの点があるときは、端の座標そのものから距離を計算するので、
/// 同じ端点を共有する辺は同じ距離になる。
fn closest_on_polyline(plane: &Plane, line: &[Coord]) -> Hit {
    let mut best = Hit {
        dist: f64::INFINITY,
        seg: 0,
        t: 0.0,
    };
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
        if dist < best.dist {
            best = Hit { dist, seg: i, t };
        }
    }
    best
}

/// 距離が等しいとみなす差（m）。浮動小数点の誤差で、同じ距離の辺の順序が入れ替わらないようにする。
const TIE_EPS_M: f64 = 1e-9;

/// 窓の余裕（上限300mの何倍までセルを調べるか）。
const WINDOW_MARGIN: f64 = 1.02;

/// 点から `SNAP_MAX_DISTANCE_M` 以内にある可能性のある辺（格子のセルで絞った、昇順・重複なし）。
fn candidate_edges(index: &OdIndex, plane: &Plane) -> Vec<EdgeId> {
    let p = &index.params;
    let reach_m = SNAP_MAX_DISTANCE_M * WINDOW_MARGIN;
    let dlat = (reach_m / plane.my).ceil() as i64;
    let dlon = (reach_m / plane.mx).ceil() as i64;
    let cell = |origin: i32, delta: i64, base: i32, size: i32| {
        (i64::from(origin) + delta - i64::from(base)).div_euclid(i64::from(size))
    };
    let o = plane.origin;
    let rows = (
        cell(o.lat_e7, -dlat, p.lat0_e7, p.cell_lat_e7),
        cell(o.lat_e7, dlat, p.lat0_e7, p.cell_lat_e7),
    );
    let cols = (
        cell(o.lon_e7, -dlon, p.lon0_e7, p.cell_lon_e7),
        cell(o.lon_e7, dlon, p.lon0_e7, p.cell_lon_e7),
    );
    let last_row = i64::from(p.rows) - 1;
    let last_col = i64::from(p.cols) - 1;
    let (row_lo, row_hi) = (rows.0.max(0), rows.1.min(last_row));
    let (col_lo, col_hi) = (cols.0.max(0), cols.1.min(last_col));
    let mut edges = Vec::new();
    for row in row_lo..=row_hi {
        for col in col_lo..=col_hi {
            let key = row * i64::from(p.cols) + col;
            if let Ok(key) = u32::try_from(key) {
                edges.extend_from_slice(index.edges_in_cell(key));
            }
        }
    }
    edges.sort_unstable();
    edges.dedup();
    edges
}

/// 点を、その手段で通れる辺に吸着する（形式書11章）。
///
/// 対象は、その手段で少なくとも一方向に通れて、`Topology::snappable` が真の辺。距離が最小の辺の
/// 最も近い点に吸着し、同じ距離なら辺番号が小さいものを選ぶ。距離が `SNAP_MAX_DISTANCE_M`
/// を超える、または辺がなければ `None`（呼び出し側が `NoRoute` にする）。
///
/// 点を1e-7度に四捨五入し、その緯度で決めた局所平面で辺の折れ線までの距離を比べる。調べる辺は、
/// 点から300m（に余裕を足した範囲）に入るセルの辺だけで、辺番号の昇順に見る。
/// 返す `offset_dm` は、折れ線の長さに対する最寄りの点の位置の割合に `length_dm` を掛けて
/// 四捨五入した値（0〜`length_dm`）。`distance_m` は `contract::geo::straight_distance_m` で測った、
/// 元の点から吸着した点までの距離。
pub fn snap(
    graph: &Graph,
    topology: &Topology,
    index: &OdIndex,
    point: LatLon,
    mode: Mode,
) -> Option<SnapPoint> {
    if !point.lat.is_finite() || !point.lon.is_finite() {
        return None;
    }
    let plane = Plane::new(Coord::from_latlon(point));
    let edge_count = graph.edge_count();
    let mut best: Option<(EdgeId, Hit)> = None;
    for e in candidate_edges(index, &plane) {
        if e as usize >= edge_count
            || graph.edge(e).access & access::both(mode) == 0
            || !topology.snappable(e, mode)
        {
            continue;
        }
        let hit = closest_on_polyline(&plane, &graph.edge_polyline(e));
        if best
            .as_ref()
            .is_none_or(|(_, b)| hit.dist < b.dist - TIE_EPS_M)
        {
            best = Some((e, hit));
        }
    }
    let (edge, hit) = best?;

    let line = graph.edge_polyline(edge);
    let position = interpolate(line[hit.seg], line[hit.seg + 1], hit.t);
    let distance_m = straight_distance_m(point, position.to_latlon());
    if distance_m > SNAP_MAX_DISTANCE_M {
        return None;
    }
    let lengths = segment_lengths_m(&line);
    let total: f64 = lengths.iter().sum();
    let length_dm = graph.edge(edge).length_dm;
    let offset_dm = if total > 0.0 {
        let before: f64 = lengths[..hit.seg].iter().sum::<f64>() + hit.t * lengths[hit.seg];
        let scaled = (before / total * f64::from(length_dm)).round();
        (scaled as u32).min(length_dm)
    } else {
        0
    };
    Some(SnapPoint {
        edge,
        offset_dm,
        position,
        distance_m,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> OdIndex {
        OdIndex {
            params: OdIndexParams {
                lat0_e7: 200_000_000,
                lon0_e7: 1_220_000_000,
                cell_lat_e7: 25_000,
                cell_lon_e7: 25_000,
                rows: 10,
                cols: 10,
            },
            raw_params: serde_json::Value::Null,
            cell_keys: vec![0, 11],
            cell_ofs: vec![0, 1, 3],
            cell_edges: vec![4, 5, 9],
        }
    }

    #[test]
    fn cell_lookup() {
        let i = index();
        assert_eq!(i.cell_key(Coord::new(200_000_000, 1_220_000_000)), Some(0));
        assert_eq!(i.cell_key(Coord::new(200_025_000, 1_220_025_000)), Some(11));
        assert_eq!(i.cell_key(Coord::new(199_999_999, 1_220_000_000)), None);
        assert_eq!(i.cell_key(Coord::new(200_250_000, 1_220_000_000)), None);
        assert_eq!(i.edges_in_cell(0), &[4]);
        assert_eq!(i.edges_in_cell(11), &[5, 9]);
        assert!(i.edges_in_cell(5).is_empty());
    }
}
