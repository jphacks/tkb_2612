//! 評価地域の行政界（M5.1）と内外判定（M5.2）。
//!
//! PBFから読んだwayのノードID列を閉じたリングに組み立て、外周と穴から多角形を作る。
//! GeoJSON（`FeatureCollection`）の書き出しと読み込み、点の内外判定、面積の計算を持つ。
//! PBFの読み取りは `data::osm` が行い、このモジュールは座標だけを扱う。

use std::collections::BTreeMap;
use std::fmt;

use serde_json::{Value, json};

use crate::contract::LatLon;
use crate::contract::geo::METERS_PER_DEGREE;
use crate::domain::routing::graph::Coord;

/// 行政界のway。`nodes` はOSMのノードIDの列（先頭と末尾が端点）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundaryWay {
    pub id: i64,
    pub nodes: Vec<i64>,
}

/// リングの組み立ての失敗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingError(pub String);

impl fmt::Display for RingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for RingError {}

/// 向きがばらばらのwayを、端点が同じノードどうしでつないで閉じたリングにする。
///
/// 返すリングはノードIDの列で、先頭と末尾は同じノード。すでに閉じているwayはそのまま1つのリングに
/// する。途中で閉じた部分リングは切り出し、残りは続けてつなぐ（8の字に接するリングも扱える）。
/// 端点が合わないwayが残った場合は、そのwayのIDを含むエラーを返す。
pub fn assemble_rings(ways: &[BoundaryWay]) -> Result<Vec<Vec<i64>>, RingError> {
    let mut rings = Vec::new();
    let mut used = vec![false; ways.len()];
    for (i, way) in ways.iter().enumerate() {
        if way.nodes.len() < 2 {
            return Err(RingError(format!(
                "way {} の構成ノードが2個未満です",
                way.id
            )));
        }
        if way.nodes[0] == way.nodes[way.nodes.len() - 1] {
            if way.nodes.len() < 4 {
                return Err(RingError(format!(
                    "way {} は閉じていますが、構成ノードが4個未満です",
                    way.id
                )));
            }
            rings.push(way.nodes.clone());
            used[i] = true;
        }
    }

    // 端点のノードID → そのノードを端点に持つwayの添字（wayの順）。
    let mut incident: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
    for (i, way) in ways.iter().enumerate() {
        if used[i] {
            continue;
        }
        incident.entry(way.nodes[0]).or_default().push(i);
        incident
            .entry(*way.nodes.last().expect("2個以上"))
            .or_default()
            .push(i);
    }

    for start in 0..ways.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let first = &ways[start];
        let mut chain = first.nodes.clone();
        let start_node = chain[0];
        // 鎖の節点（wayの端点）の位置。開始ノードは先頭。
        let mut joints: BTreeMap<i64, usize> = BTreeMap::new();
        joints.insert(start_node, 0);
        let end = chain.len() - 1;
        let mut current = chain[end];
        joints.insert(current, end);

        loop {
            if current == start_node {
                rings.push(chain.clone());
                break;
            }
            let next = incident
                .get(&current)
                .and_then(|ids| ids.iter().copied().find(|&j| !used[j]));
            let Some(next) = next else {
                return Err(RingError(format!(
                    "way {} から始めたリングが閉じません（node {current} でつながる未使用のwayがありません）",
                    first.id
                )));
            };
            used[next] = true;
            let way = &ways[next];
            let (added, far) = if way.nodes[0] == current {
                (way.nodes[1..].to_vec(), *way.nodes.last().expect("2個以上"))
            } else if *way.nodes.last().expect("2個以上") == current {
                // 向きを逆にして、現在の端点を先頭にする。
                (
                    way.nodes.iter().rev().skip(1).copied().collect(),
                    way.nodes[0],
                )
            } else {
                return Err(RingError(format!(
                    "way {} は node {current} に接していません",
                    way.id
                )));
            };
            chain.extend(added);
            let new_end = chain.len() - 1;
            current = far;
            if let Some(&joint) = joints.get(&far) {
                // 鎖が途中の節点に戻ったので、その間を1つのリングとして切り出す。
                rings.push(chain[joint..].to_vec());
                chain.truncate(joint + 1);
                joints.retain(|_, &mut index| index <= joint);
                if joint == 0 {
                    chain.clear();
                    break;
                }
            } else {
                joints.insert(far, new_end);
            }
        }
    }
    Ok(rings)
}

/// 座標の外接矩形（1e-7度）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionBBox {
    pub lat_min: i32,
    pub lon_min: i32,
    pub lat_max: i32,
    pub lon_max: i32,
}

impl RegionBBox {
    /// 座標の列から作る。空なら `None`。
    pub fn from_coords(coords: impl IntoIterator<Item = Coord>) -> Option<RegionBBox> {
        let mut iter = coords.into_iter();
        let first = iter.next()?;
        let mut bbox = RegionBBox {
            lat_min: first.lat_e7,
            lon_min: first.lon_e7,
            lat_max: first.lat_e7,
            lon_max: first.lon_e7,
        };
        for c in iter {
            bbox.lat_min = bbox.lat_min.min(c.lat_e7);
            bbox.lon_min = bbox.lon_min.min(c.lon_e7);
            bbox.lat_max = bbox.lat_max.max(c.lat_e7);
            bbox.lon_max = bbox.lon_max.max(c.lon_e7);
        }
        Some(bbox)
    }

    /// リング（先頭と末尾が同じ）から作る。
    pub fn from_ring(ring: &[Coord]) -> Option<RegionBBox> {
        RegionBBox::from_coords(ring.iter().copied())
    }

    /// 点が矩形の内側か（境界を含む）。
    pub fn contains(&self, p: Coord) -> bool {
        (self.lat_min..=self.lat_max).contains(&p.lat_e7)
            && (self.lon_min..=self.lon_max).contains(&p.lon_e7)
    }

    /// 中心の緯度・経度（度）。
    pub fn center(&self) -> (f64, f64) {
        (
            (f64::from(self.lat_min) + f64::from(self.lat_max)) / 2.0 / 1e7,
            (f64::from(self.lon_min) + f64::from(self.lon_max)) / 2.0 / 1e7,
        )
    }

    /// 2つの矩形を包む矩形。
    pub fn union(self, other: RegionBBox) -> RegionBBox {
        RegionBBox {
            lat_min: self.lat_min.min(other.lat_min),
            lon_min: self.lon_min.min(other.lon_min),
            lat_max: self.lat_max.max(other.lat_max),
            lon_max: self.lon_max.max(other.lon_max),
        }
    }
}

/// 外周1つと穴からなる多角形。座標は閉じたリング（先頭と末尾が同じ）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Polygon {
    outer: Vec<Coord>,
    holes: Vec<Vec<Coord>>,
    bbox: RegionBBox,
}

impl Polygon {
    /// 外周と穴から作る。リングを閉じ、外周は反時計回り・穴は時計回りにそろえる。
    pub fn new(outer: Vec<Coord>, holes: Vec<Vec<Coord>>) -> Result<Polygon, String> {
        let outer = normalize_ring(outer, true)?;
        let bbox = RegionBBox::from_ring(&outer).ok_or("外周のリングが空です")?;
        let mut normalized = Vec::with_capacity(holes.len());
        for hole in holes {
            normalized.push(normalize_ring(hole, false)?);
        }
        Ok(Polygon {
            outer,
            holes: normalized,
            bbox,
        })
    }

    /// 点が多角形の内側か。穴の内側は外とみなす。
    pub fn contains(&self, p: Coord) -> bool {
        self.bbox.contains(p)
            && point_in_ring(&self.outer, p)
            && !self.holes.iter().any(|hole| point_in_ring(hole, p))
    }

    pub fn outer(&self) -> &[Coord] {
        &self.outer
    }

    pub fn holes(&self) -> &[Vec<Coord>] {
        &self.holes
    }

    pub fn bbox(&self) -> RegionBBox {
        self.bbox
    }

    /// 外周の面積（m²）。
    pub fn outer_area_m2(&self) -> f64 {
        ring_area_m2(&self.outer)
    }

    /// 外周から穴を引いた面積（m²）。
    pub fn net_area_m2(&self) -> f64 {
        let holes: f64 = self.holes.iter().map(|hole| ring_area_m2(hole)).sum();
        (self.outer_area_m2() - holes).max(0.0)
    }
}

/// 外周のリングと穴のリングを多角形にまとめる。穴は、その頂点を含む外周のうち面積が最小のものに付ける。
pub fn polygons_from_rings(
    outer_rings: Vec<Vec<Coord>>,
    inner_rings: Vec<Vec<Coord>>,
) -> Result<Vec<Polygon>, String> {
    if outer_rings.is_empty() {
        return Err("外周のリングがありません".to_owned());
    }
    let mut polygons = Vec::with_capacity(outer_rings.len());
    for outer in outer_rings {
        polygons.push(Polygon::new(outer, Vec::new())?);
    }
    for hole in inner_rings {
        let hole = normalize_ring(hole, false)?;
        let target = polygons
            .iter()
            .enumerate()
            .filter(|(_, polygon)| {
                hole.iter()
                    .any(|&point| point_in_ring(&polygon.outer, point))
            })
            .min_by(|(_, a), (_, b)| a.outer_area_m2().total_cmp(&b.outer_area_m2()))
            .map(|(i, _)| i);
        let Some(index) = target else {
            return Err("穴のリングがどの外周の内側にもありません".to_owned());
        };
        polygons[index].holes.push(hole);
    }
    Ok(polygons)
}

/// 1つの評価地域の形。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionGeometry {
    polygons: Vec<Polygon>,
    bbox: RegionBBox,
}

impl RegionGeometry {
    pub fn new(polygons: Vec<Polygon>) -> Result<RegionGeometry, String> {
        let bbox = polygons
            .iter()
            .map(Polygon::bbox)
            .reduce(RegionBBox::union)
            .ok_or("多角形がありません")?;
        Ok(RegionGeometry { polygons, bbox })
    }

    /// 点が地域の内側か。外接矩形で先に絞ってから偶奇規則で判定する。
    pub fn contains(&self, p: Coord) -> bool {
        if !self.bbox.contains(p) {
            return false;
        }
        self.polygons.iter().any(|polygon| polygon.contains(p))
    }

    pub fn polygons(&self) -> &[Polygon] {
        &self.polygons
    }

    pub fn bbox(&self) -> RegionBBox {
        self.bbox
    }

    /// 面積（km²）。外周から穴を引いた値の合計。
    pub fn area_km2(&self) -> f64 {
        self.polygons.iter().map(Polygon::net_area_m2).sum::<f64>() / 1e6
    }

    /// GeoJSONの `FeatureCollection` から、`properties.region` が `region_id` の Feature を読む。
    /// `MultiPolygon` と `Polygon` を受け付ける。
    pub fn from_geojson(text: &str, region_id: &str) -> Result<RegionGeometry, String> {
        let root: Value =
            serde_json::from_str(text).map_err(|e| format!("GeoJSONを読めません: {e}"))?;
        let features = root
            .get("features")
            .and_then(Value::as_array)
            .ok_or("GeoJSONにfeaturesがありません")?;
        let feature = features
            .iter()
            .find(|feature| {
                feature
                    .pointer("/properties/region")
                    .and_then(Value::as_str)
                    == Some(region_id)
            })
            .ok_or_else(|| format!("地域 {region_id} がGeoJSONにありません"))?;
        let geometry = feature
            .get("geometry")
            .ok_or_else(|| format!("地域 {region_id} のgeometryがありません"))?;
        let kind = geometry
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("地域 {region_id} のgeometry.typeがありません"))?;
        let coordinates = geometry
            .get("coordinates")
            .ok_or_else(|| format!("地域 {region_id} のgeometry.coordinatesがありません"))?;
        let polygons = match kind {
            "MultiPolygon" => coordinates
                .as_array()
                .ok_or("MultiPolygonのcoordinatesが配列ではありません")?
                .iter()
                .map(parse_polygon)
                .collect::<Result<Vec<_>, _>>()?,
            "Polygon" => vec![parse_polygon(coordinates)?],
            other => return Err(format!("対応していないgeometry.typeです: {other}")),
        };
        RegionGeometry::new(polygons)
    }
}

/// GeoJSONに書く1地域。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionFeature {
    pub region: String,
    pub relations: Vec<(i64, String)>,
    pub geometry: RegionGeometry,
}

/// 地域の列をGeoJSONの `FeatureCollection`（`serde_json::Value`）にする。
pub fn feature_collection(features: &[RegionFeature]) -> Value {
    let features: Vec<Value> = features
        .iter()
        .map(|feature| {
            let polygons: Vec<Value> = feature
                .geometry
                .polygons()
                .iter()
                .map(|polygon| {
                    let mut rings = Vec::with_capacity(polygon.holes().len() + 1);
                    rings.push(ring_json(polygon.outer()));
                    rings.extend(polygon.holes().iter().map(|hole| ring_json(hole)));
                    Value::Array(rings)
                })
                .collect();
            let outer = feature.geometry.polygons().len();
            let inner: usize = feature
                .geometry
                .polygons()
                .iter()
                .map(|polygon| polygon.holes().len())
                .sum();
            let relations: Vec<Value> = feature
                .relations
                .iter()
                .map(|(id, name)| json!({ "id": id, "name": name }))
                .collect();
            json!({
                "type": "Feature",
                "properties": {
                    "region": feature.region,
                    "relations": relations,
                    "rings": { "outer": outer, "inner": inner },
                    "area_km2": (feature.geometry.area_km2() * 100.0).round() / 100.0,
                },
                "geometry": {
                    "type": "MultiPolygon",
                    "coordinates": polygons,
                },
            })
        })
        .collect();
    json!({ "type": "FeatureCollection", "features": features })
}

fn ring_json(ring: &[Coord]) -> Value {
    Value::Array(
        ring.iter()
            .map(|c| json!([f64::from(c.lon_e7) / 1e7, f64::from(c.lat_e7) / 1e7]))
            .collect(),
    )
}

fn parse_polygon(value: &Value) -> Result<Polygon, String> {
    let rings = value
        .as_array()
        .ok_or("多角形のcoordinatesが配列ではありません")?;
    let Some((outer, holes)) = rings.split_first() else {
        return Err("多角形にリングがありません".to_owned());
    };
    let outer = parse_ring(outer)?;
    let holes = holes
        .iter()
        .map(parse_ring)
        .collect::<Result<Vec<_>, _>>()?;
    Polygon::new(outer, holes)
}

fn parse_ring(value: &Value) -> Result<Vec<Coord>, String> {
    let points = value
        .as_array()
        .ok_or("リングのcoordinatesが配列ではありません")?;
    let mut ring = Vec::with_capacity(points.len());
    for point in points {
        let pair = point.as_array().ok_or("座標が配列ではありません")?;
        let (Some(lon), Some(lat)) = (
            pair.first().and_then(Value::as_f64),
            pair.get(1).and_then(Value::as_f64),
        ) else {
            return Err("座標が [lon, lat] の数ではありません".to_owned());
        };
        if !lon.is_finite() || !lat.is_finite() {
            return Err("座標が有限の数ではありません".to_owned());
        }
        ring.push(Coord::from_latlon(LatLon::new(lat, lon)));
    }
    if ring.len() < 3 {
        return Err("リングの頂点が3個未満です".to_owned());
    }
    Ok(ring)
}

/// 連続する同じ点を除き、閉じたリングにする。向きは外周なら反時計回り、穴なら時計回り。
fn normalize_ring(mut ring: Vec<Coord>, outer: bool) -> Result<Vec<Coord>, String> {
    ring.dedup();
    if ring.first() == ring.last() {
        ring.pop();
    }
    if ring.len() < 3 {
        return Err("リングの頂点が3個未満です".to_owned());
    }
    let area = signed_area_e7(&ring);
    if (outer && area < 0.0) || (!outer && area > 0.0) {
        ring.reverse();
    }
    ring.push(ring[0]);
    Ok(ring)
}

/// 閉じていないリングの符号付き面積（1e-7度の2乗）。反時計回りが正。
fn signed_area_e7(ring: &[Coord]) -> f64 {
    if ring.len() < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..ring.len() {
        let a = ring[i];
        let b = ring[(i + 1) % ring.len()];
        sum +=
            f64::from(a.lon_e7) * f64::from(b.lat_e7) - f64::from(b.lon_e7) * f64::from(a.lat_e7);
    }
    sum / 2.0
}

/// 閉じたリングの面積（m²）。最初の点の接平面で靴ひも公式を使う。
fn ring_area_m2(ring: &[Coord]) -> f64 {
    if ring.len() < 4 {
        return 0.0;
    }
    let origin = ring[0].to_latlon();
    let cos_lat = origin.lat.to_radians().cos().max(0.05);
    let x = |lon: f64| (lon - origin.lon) * METERS_PER_DEGREE * cos_lat;
    let y = |lat: f64| (lat - origin.lat) * METERS_PER_DEGREE;
    let mut sum = 0.0;
    for pair in ring.windows(2) {
        let a = pair[0].to_latlon();
        let b = pair[1].to_latlon();
        sum += x(a.lon) * y(b.lat) - x(b.lon) * y(a.lat);
    }
    (sum / 2.0).abs()
}

/// 偶奇規則（水平な半直線と辺の交差を数える）。境界上の点の扱いは決めない。
fn point_in_ring(ring: &[Coord], p: Coord) -> bool {
    let mut inside = false;
    for pair in ring.windows(2) {
        let a = pair[0];
        let b = pair[1];
        if (a.lat_e7 > p.lat_e7) != (b.lat_e7 > p.lat_e7) {
            let dy = (i64::from(b.lat_e7) - i64::from(a.lat_e7)) as f64;
            let t = (i64::from(p.lat_e7) - i64::from(a.lat_e7)) as f64 / dy;
            let cross_lon =
                f64::from(a.lon_e7) + t * (i64::from(b.lon_e7) - i64::from(a.lon_e7)) as f64;
            if f64::from(p.lon_e7) < cross_lon {
                inside = !inside;
            }
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coord(lat: f64, lon: f64) -> Coord {
        Coord::from_latlon(LatLon::new(lat, lon))
    }

    fn way(id: i64, nodes: &[i64]) -> BoundaryWay {
        BoundaryWay {
            id,
            nodes: nodes.to_vec(),
        }
    }

    #[test]
    fn three_ways_with_mixed_directions_form_one_ring() {
        let ways = [way(10, &[1, 2]), way(20, &[3, 1]), way(30, &[2, 3])];
        let rings = assemble_rings(&ways).unwrap();
        assert_eq!(rings.len(), 1);
        assert_eq!(rings[0], vec![1, 2, 3, 1]);
    }

    #[test]
    fn open_ways_fail_with_the_way_id() {
        let ways = [way(10, &[1, 2]), way(20, &[3, 4])];
        let error = assemble_rings(&ways).unwrap_err();
        assert!(error.to_string().contains("10"), "{error}");
    }

    #[test]
    fn eight_shaped_rings_are_split_at_the_shared_node() {
        // 1-2-3-1 と 1-4-5-1 が node 1 を共有する。
        let ways = [
            way(10, &[1, 2]),
            way(20, &[2, 3]),
            way(30, &[3, 1]),
            way(40, &[1, 4]),
            way(50, &[4, 5]),
            way(60, &[5, 1]),
        ];
        let mut rings = assemble_rings(&ways).unwrap();
        rings.sort();
        assert_eq!(rings, vec![vec![1, 2, 3, 1], vec![1, 4, 5, 1]]);
    }

    #[test]
    fn a_point_in_a_hole_is_outside() {
        let outer = vec![
            coord(35.0, 139.0),
            coord(35.0, 139.01),
            coord(35.01, 139.01),
            coord(35.01, 139.0),
        ];
        let hole = vec![
            coord(35.004, 139.004),
            coord(35.004, 139.006),
            coord(35.006, 139.006),
            coord(35.006, 139.004),
        ];
        let polygons = polygons_from_rings(vec![outer], vec![hole]).unwrap();
        let region = RegionGeometry::new(polygons).unwrap();
        assert!(region.contains(coord(35.002, 139.002)));
        assert!(!region.contains(coord(35.005, 139.005)));
        assert!(!region.contains(coord(35.02, 139.02)));
        assert!(
            (region.area_km2() - 0.96).abs() < 0.02,
            "{}",
            region.area_km2()
        );
    }

    #[test]
    fn geojson_round_trip_keeps_the_hole() {
        let outer = vec![
            coord(35.0, 139.0),
            coord(35.0, 139.01),
            coord(35.01, 139.01),
            coord(35.01, 139.0),
        ];
        let hole = vec![
            coord(35.004, 139.004),
            coord(35.004, 139.006),
            coord(35.006, 139.006),
            coord(35.006, 139.004),
        ];
        let region =
            RegionGeometry::new(polygons_from_rings(vec![outer], vec![hole]).unwrap()).unwrap();
        let text = serde_json::to_string(&feature_collection(&[RegionFeature {
            region: "test".to_owned(),
            relations: vec![(1, "テスト".to_owned())],
            geometry: region.clone(),
        }]))
        .unwrap();
        let parsed = RegionGeometry::from_geojson(&text, "test").unwrap();
        assert_eq!(parsed.polygons().len(), 1);
        assert!(parsed.contains(coord(35.002, 139.002)));
        assert!(!parsed.contains(coord(35.005, 139.005)));
        assert!((parsed.area_km2() - region.area_km2()).abs() < 1e-6);
    }
}
