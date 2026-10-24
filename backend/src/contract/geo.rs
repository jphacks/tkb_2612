//! 座標と距離の共通定義。
//!
//! 座標は配列ではなく名前付きフィールドで持ち、順序の取り違えを型で防ぐ。
//! GeoJSONの `[lon, lat]` への変換はAPI層（DTO）と経路アダプタだけが行う。

use std::f64::consts::PI;
use std::ops::RangeInclusive;

/// 地球半径R（m）。behavior.md 4章。
pub const EARTH_RADIUS_M: f64 = 6_371_008.8;
/// 1度あたりのメートル数 K = πR/180。
pub const METERS_PER_DEGREE: f64 = PI * EARTH_RADIUS_M / 180.0;
/// 受理する緯度（両端を含む）。
pub const LAT_RANGE: RangeInclusive<f64> = 20.0..=46.0;
/// 受理する経度（両端を含む）。
pub const LON_RANGE: RangeInclusive<f64> = 122.0..=154.0;

/// 入力地点・住所候補・経由点の座標（WGS84、十進度）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

impl LatLon {
    pub const fn new(lat: f64, lon: f64) -> Self {
        Self { lat, lon }
    }

    /// 有限値で、日本周辺の矩形に入っているか。
    pub fn is_in_japan(&self) -> bool {
        self.lat.is_finite()
            && self.lon.is_finite()
            && LAT_RANGE.contains(&self.lat)
            && LON_RANGE.contains(&self.lon)
    }
}

/// 経路geometryや事故点の頂点。JSONでは `[lon, lat]` の順に並べる。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LonLat {
    pub lon: f64,
    pub lat: f64,
}

impl LonLat {
    pub const fn new(lon: f64, lat: f64) -> Self {
        Self { lon, lat }
    }

    pub fn is_in_japan(&self) -> bool {
        LatLon::from(*self).is_in_japan()
    }
}

impl From<LatLon> for LonLat {
    fn from(p: LatLon) -> Self {
        Self {
            lon: p.lon,
            lat: p.lat,
        }
    }
}

impl From<LonLat> for LatLon {
    fn from(p: LonLat) -> Self {
        Self {
            lat: p.lat,
            lon: p.lon,
        }
    }
}

/// behavior.md 4章の直線距離（m）。測地線距離ではなく、平均緯度での局所近似。
pub fn straight_distance_m(a: LatLon, b: LatLon) -> f64 {
    let mean_lat_rad = (a.lat + b.lat) / 2.0 * PI / 180.0;
    METERS_PER_DEGREE * (a.lat - b.lat).hypot((a.lon - b.lon) * mean_lat_rad.cos())
}
