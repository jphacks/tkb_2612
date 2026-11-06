//! 1e-7度の整数座標に対する、局所平面近似の幾何（前処理用）。
//!
//! 距離の定義は `contract::geo::straight_distance_m` と同じ平均緯度の局所近似に合わせる。
//! 辺の折れ線は数十m〜数kmなので、折れ線ごとに基準の緯度を決めれば十分に近い。

use crate::contract::LatLon;
use crate::contract::geo::{METERS_PER_DEGREE, straight_distance_m};
use crate::domain::routing::graph::Coord;

/// 基準点を原点にした局所平面（x：東、y：北、単位m）。
#[derive(Debug, Clone, Copy)]
pub struct LocalFrame {
    origin: Coord,
    cos_lat: f64,
}

impl LocalFrame {
    pub fn new(origin: Coord) -> Self {
        let lat_rad = (f64::from(origin.lat_e7) * 1e-7).to_radians();
        Self {
            origin,
            cos_lat: lat_rad.cos(),
        }
    }

    /// 局所平面のメートルを座標に戻す（`xy` の逆。1e-7度に四捨五入する）。
    pub fn coord(&self, p: (f64, f64)) -> Coord {
        let dlon = p.0 / (1e-7 * METERS_PER_DEGREE * self.cos_lat);
        let dlat = p.1 / (1e-7 * METERS_PER_DEGREE);
        Coord::new(
            (f64::from(self.origin.lat_e7) + dlat).round() as i32,
            (f64::from(self.origin.lon_e7) + dlon).round() as i32,
        )
    }

    /// 座標を局所平面のメートルにする。
    pub fn xy(&self, c: Coord) -> (f64, f64) {
        let dlat = f64::from(c.lat_e7) - f64::from(self.origin.lat_e7);
        let dlon = f64::from(c.lon_e7) - f64::from(self.origin.lon_e7);
        (
            dlon * 1e-7 * METERS_PER_DEGREE * self.cos_lat,
            dlat * 1e-7 * METERS_PER_DEGREE,
        )
    }
}

/// 2点間の距離（m）。`contract::geo::straight_distance_m` と同じ。
pub fn distance_m(a: Coord, b: Coord) -> f64 {
    straight_distance_m(a.to_latlon(), b.to_latlon())
}

/// 折れ線の長さ（m）。
pub fn polyline_length_m(points: &[Coord]) -> f64 {
    points.windows(2).map(|w| distance_m(w[0], w[1])).sum()
}

/// 辺の長さ（デシメートル、形式書1章）：折れ線の各線分の距離の和を10倍して四捨五入した値（最小1）。
pub fn length_dm(points: &[Coord]) -> u32 {
    let dm = (polyline_length_m(points) * 10.0).round();
    if dm < 1.0 {
        1
    } else {
        dm.min(f64::from(u32::MAX)) as u32
    }
}

/// 点と線分の距離。`(距離, 線分上の位置 t∈[0,1])`。
pub fn point_segment_distance(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 <= 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - cx).hypot(p.1 - cy), t)
}

/// 折れ線上の点と、その点での進行方向の単位ベクトル。
pub type Sample = ((f64, f64), (f64, f64));

/// 局所平面の折れ線。
#[derive(Debug, Clone)]
pub struct XyLine {
    pub points: Vec<(f64, f64)>,
    pub length: f64,
}

impl XyLine {
    pub fn new(frame: &LocalFrame, coords: &[Coord]) -> Self {
        let points: Vec<(f64, f64)> = coords.iter().map(|&c| frame.xy(c)).collect();
        let length = points
            .windows(2)
            .map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1))
            .sum();
        Self { points, length }
    }

    /// 点までの最短距離と、そのときの線分の単位方向（線分が退化していれば `None`）。
    pub fn nearest(&self, p: (f64, f64)) -> (f64, Option<(f64, f64)>) {
        let mut best = (f64::INFINITY, None);
        for w in self.points.windows(2) {
            let (d, _) = point_segment_distance(p, w[0], w[1]);
            if d < best.0 {
                let (dx, dy) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
                let len = dx.hypot(dy);
                best = (d, (len > 0.0).then(|| (dx / len, dy / len)));
            }
        }
        if self.points.len() == 1 {
            let d = (p.0 - self.points[0].0).hypot(p.1 - self.points[0].1);
            best = (d, None);
        }
        best
    }

    /// `nearest` と同じ最寄りの点を探し、その点が折れ線の端の外側にはみ出して射影される点か
    /// （始点より手前、または終点より先にあって、端点が最寄りになったか）も返す。
    /// 並走している区間かどうかの判定に使う（折れ線の延長線上にあるだけの点を除く）。
    pub fn nearest_with_overhang(&self, p: (f64, f64)) -> (f64, Option<(f64, f64)>, bool) {
        let last = self.points.len().saturating_sub(2);
        let mut best = (f64::INFINITY, None, false);
        for (i, w) in self.points.windows(2).enumerate() {
            let (d, _) = point_segment_distance(p, w[0], w[1]);
            if d < best.0 {
                let (dx, dy) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
                let len2 = dx * dx + dy * dy;
                let raw = if len2 > 0.0 {
                    ((p.0 - w[0].0) * dx + (p.1 - w[0].1) * dy) / len2
                } else {
                    0.0
                };
                let overhang = (i == 0 && raw < 0.0) || (i == last && raw > 1.0);
                let len = len2.sqrt();
                best = (d, (len > 0.0).then(|| (dx / len, dy / len)), overhang);
            }
        }
        if self.points.len() == 1 {
            let d = (p.0 - self.points[0].0).hypot(p.1 - self.points[0].1);
            best = (d, None, true);
        }
        best
    }

    /// 先頭から `step/2, 3*step/2, ...` の位置（長さ未満）にある点と単位方向。短い線でも1点は返す。
    pub fn samples(&self, step: f64) -> Vec<Sample> {
        let mut out = Vec::new();
        if self.points.len() < 2 {
            return out;
        }
        let mut target = (step / 2.0).min(self.length / 2.0);
        let mut walked = 0.0;
        for w in self.points.windows(2) {
            let (dx, dy) = (w[1].0 - w[0].0, w[1].1 - w[0].1);
            let len = dx.hypot(dy);
            if len <= 0.0 {
                continue;
            }
            let dir = (dx / len, dy / len);
            while target <= walked + len + 1e-9 && target < self.length + 1e-9 {
                let t = ((target - walked) / len).clamp(0.0, 1.0);
                out.push(((w[0].0 + t * dx, w[0].1 + t * dy), dir));
                target += step;
            }
            walked += len;
        }
        if out.is_empty() {
            // 全体が退化している線（長さ0）。始点を1点として返す
            out.push((self.points[0], (0.0, 0.0)));
        }
        out
    }
}

/// `LatLon` への変換（近似の確認用）。
pub fn latlon(c: Coord) -> LatLon {
    c.to_latlon()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_frame_scales_longitude_by_latitude() {
        let origin = Coord::new(350_000_000, 1_390_000_000);
        let frame = LocalFrame::new(origin);
        // 北へ0.001度は約111m、東へ0.001度は約91m（緯度35度）
        let (x, y) = frame.xy(Coord::new(350_010_000, 1_390_010_000));
        assert!((y - 111.19).abs() < 0.1, "{y}");
        assert!((x - 91.08).abs() < 0.2, "{x}");
        assert_eq!(frame.xy(origin), (0.0, 0.0));
        // 逆変換
        let c = Coord::new(350_010_000, 1_390_010_000);
        assert_eq!(frame.coord(frame.xy(c)), c);
    }

    #[test]
    fn length_dm_is_rounded_and_at_least_one() {
        let a = Coord::new(350_000_000, 1_390_000_000);
        assert_eq!(length_dm(&[a, a]), 1);
        let b = Coord::new(350_000_900, 1_390_000_000); // 約10.0m
        let d = length_dm(&[a, b]);
        assert!((99..=101).contains(&d), "{d}");
        // 折れ線は各線分の和
        let c = Coord::new(350_000_900, 1_390_000_900);
        let both = length_dm(&[a, b, c]);
        assert_eq!(
            both,
            ((distance_m(a, b) + distance_m(b, c)) * 10.0).round() as u32
        );
    }

    #[test]
    fn point_segment_distance_clamps() {
        let (d, t) = point_segment_distance((5.0, 3.0), (0.0, 0.0), (10.0, 0.0));
        assert_eq!((d, t), (3.0, 0.5));
        let (d, t) = point_segment_distance((-4.0, 3.0), (0.0, 0.0), (10.0, 0.0));
        assert_eq!((d, t), (5.0, 0.0));
        let (d, _) = point_segment_distance((1.0, 1.0), (0.0, 0.0), (0.0, 0.0));
        assert!((d - 2f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn samples_cover_short_and_long_lines() {
        let line = XyLine {
            points: vec![(0.0, 0.0), (100.0, 0.0)],
            length: 100.0,
        };
        let s = line.samples(20.0);
        assert_eq!(s.len(), 5);
        assert_eq!(s[0].0, (10.0, 0.0));
        assert_eq!(s[4].0, (90.0, 0.0));
        assert_eq!(s[0].1, (1.0, 0.0));
        // step より短い線は中点の1点
        let short = XyLine {
            points: vec![(0.0, 0.0), (0.0, 6.0)],
            length: 6.0,
        };
        let s = short.samples(20.0);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].0, (0.0, 3.0));
        let (d, dir) = line.nearest((50.0, 7.0));
        assert_eq!((d, dir), (7.0, Some((1.0, 0.0))));
    }

    #[test]
    fn nearest_with_overhang_flags_points_beyond_the_ends() {
        // L字の折れ線：(0,0)→(100,0)→(100,100)
        let line = XyLine {
            points: vec![(0.0, 0.0), (100.0, 0.0), (100.0, 100.0)],
            length: 200.0,
        };
        // 内側（最初の線分に射影される）
        assert_eq!(
            line.nearest_with_overhang((50.0, 10.0)),
            (10.0, Some((1.0, 0.0)), false)
        );
        // 折れ角の外側（頂点が最寄りだが、端ではない）
        let (d, _, overhang) = line.nearest_with_overhang((110.0, -10.0));
        assert!((d - 200.0_f64.sqrt()).abs() < 1e-9);
        assert!(!overhang);
        // 始点より手前・終点より先は、端点が最寄りでもはみ出しとする
        assert!(line.nearest_with_overhang((-5.0, 3.0)).2);
        assert!(line.nearest_with_overhang((95.0, 130.0)).2);
        // 端点ちょうどの手前（折れ線の中）ははみ出しではない
        assert!(!line.nearest_with_overhang((100.0, 95.0)).2);
    }
}
