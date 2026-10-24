//! 72m回廊と75m分割（behavior.md 6章、11章 SpatialAndSelectionTests、追加試験2・3、#8 ステップ10）。

mod common;

use accident_route_backend::contract::geo::METERS_PER_DEGREE;
use accident_route_backend::contract::{CancelToken, LonLat, Mode};
use accident_route_backend::domain::corridor::{
    CORRIDOR_RADIUS_SQ_M2, CorridorHit, Subdivision, corridor_hits, corridor_hits_exhaustive,
};

use common::{Acc, lat_north_of_35, snapshot};

const KX35: f64 = METERS_PER_DEGREE * 0.819_152_044_288_991_8; // cos(35°)

fn line(points: &[(f64, f64)]) -> Vec<LonLat> {
    points
        .iter()
        .map(|&(lon, lat)| LonLat::new(lon, lat))
        .collect()
}

fn hits(coordinates: &[LonLat], accidents: &[Acc], mode: Mode) -> Vec<CorridorHit> {
    let subdivision = Subdivision::new(coordinates);
    let snap = snapshot(accidents);
    let indexed =
        corridor_hits(&subdivision, &snap, mode.mask(), &CancelToken::unbounded()).unwrap();
    // 索引の走査順に依存しない（全小線分と比べる参照実装と一致する）
    assert_eq!(
        indexed,
        corridor_hits_exhaustive(&subdivision, &snap, mode.mask())
    );
    indexed
}

fn ids(hits: &[CorridorHit]) -> Vec<u32> {
    hits.iter().map(|h| h.accident).collect()
}

/// 東西1000mの直線（緯度35度）。
fn east_1000m() -> Vec<LonLat> {
    line(&[(139.0, 35.0), (139.0 + 1000.0 / KX35, 35.0)])
}

#[test]
fn test_corridor_segment_interior_endpoints_and_deduplication() {
    let route = east_1000m();
    let mid = 139.0 + 500.0 / KX35;
    let end = route[1].lon;
    let accidents = [
        Acc::at(mid, lat_north_of_35(71.0)), // 0 内部・72m以内
        Acc::at(mid, lat_north_of_35(73.0)), // 1 内部・72m超
        Acc::at(end + 50.0 / KX35, 35.0),    // 2 終点の先50m
        Acc::at(end + 73.0 / KX35, 35.0),    // 3 終点の先73m
        Acc::at(139.0 - 30.0 / KX35, lat_north_of_35(-30.0)), // 4 始点の外側の斜め
        Acc::at(mid, 35.0).modes(Mode::Bike.mask()), // 5 手段不一致
        Acc::at(mid, 35.0).modes(Mode::Walk.mask() | Mode::Car.mask()), // 6 手段一致
    ];
    let result = hits(&route, &accidents, Mode::Walk);
    assert_eq!(ids(&result), [0, 2, 4, 6]);
    let d = |id: u32| {
        result
            .iter()
            .find(|h| h.accident == id)
            .unwrap()
            .distance_m()
    };
    assert!((d(0) - 71.0).abs() < 1e-6);
    assert!((d(2) - 50.0).abs() < 1e-6);
    assert!((d(4) - 30.0 * 2f64.sqrt()).abs() < 1e-6);
    // 1000mは14本（ceil(1000/75)）。中点はsid 6、終点の先は最後のsid 13
    let sid = |id: u32| result.iter().find(|h| h.accident == id).unwrap().sid;
    assert_eq!(Subdivision::new(&route).segments.len(), 14);
    assert_eq!(sid(0), 6);
    assert_eq!(sid(2), 13);
    assert_eq!(sid(4), 0);
}

fn next_up(x: f64) -> f64 {
    f64::from_bits(x.to_bits() + 1)
}

#[test]
fn radius_boundary_is_inclusive() {
    let route = east_1000m();
    let subdivision = Subdivision::new(&route);
    let mid = 139.0 + 500.0 / KX35;
    // 計算上 d² <= 5184 となる最大の緯度を1 ulpずつ探す
    let d2 = |lat: f64| {
        let p = subdivision.projection.xy(LonLat::new(mid, lat));
        subdivision.segments[6].distance_sq(p)
    };
    let mut lat = lat_north_of_35(71.999_999);
    while d2(next_up(lat)) <= CORRIDOR_RADIUS_SQ_M2 {
        lat = next_up(lat);
    }
    assert!(d2(lat) <= CORRIDOR_RADIUS_SQ_M2 && d2(next_up(lat)) > CORRIDOR_RADIUS_SQ_M2);
    let result = hits(
        &route,
        &[Acc::at(mid, lat), Acc::at(mid, next_up(lat))],
        Mode::Walk,
    );
    assert_eq!(ids(&result), [0], "72mちょうどは含み、直上は除く");
}

#[test]
fn test_diagonal_bbox_does_not_include_distant_point() {
    // 斜めの線分のbbox内でも、線から離れた点は除外する
    let route = line(&[(139.0, 35.0), (139.01, 35.008)]);
    let accidents = [
        Acc::at(139.009, 35.001), // bboxの隅（線から数百m）
        Acc::at(139.005, 35.004), // 線上
    ];
    assert_eq!(ids(&hits(&route, &accidents, Mode::Car)), [1]);
}

#[test]
fn foldback_counts_each_accident_once_with_smallest_sid() {
    // 往復する経路：同じ距離の小線分が2本あるときは小さいsid
    let a = (139.0, 35.0);
    let b = (139.0 + 150.0 / KX35, 35.0);
    let route = line(&[a, b, a]);
    let accidents = [Acc::at(139.0 + 100.0 / KX35, lat_north_of_35(10.0))];
    let result = hits(&route, &accidents, Mode::Walk);
    assert_eq!(result.len(), 1);
    // 150mは2本。行きのsid 1と帰りのsid 2が同距離
    assert_eq!(Subdivision::new(&route).segments.len(), 4);
    assert_eq!(result[0].sid, 1);
}

#[test]
fn subsegment_split_boundaries_and_zero_length_edges() {
    let along = |meters: f64| line(&[(139.0, 35.0), (139.0, 35.0 + meters / METERS_PER_DEGREE)]);
    for (meters, count) in [(74.9, 1), (75.1, 2), (149.9, 2), (150.1, 3), (0.0, 1)] {
        assert_eq!(
            Subdivision::new(&along(meters)).segments.len(),
            count,
            "{meters}m"
        );
    }
    // 重複頂点（長さ0の辺）も1本残し、sidを数える
    let route = line(&[(139.0, 35.0), (139.0, 35.0), (139.0 + 100.0 / KX35, 35.0)]);
    let subdivision = Subdivision::new(&route);
    assert_eq!(subdivision.segments.len(), 3);
    assert_eq!(subdivision.segments[0].n, 1);
    assert_eq!(subdivision.segments[0].start, subdivision.segments[0].end);
    // 始点の上の事故は0長の小線分（sid 0）と次の小線分（sid 1）で同距離なのでsid 0
    let result = hits(&route, &[Acc::at(139.0, lat_north_of_35(5.0))], Mode::Walk);
    assert_eq!(result[0].sid, 0);
    // 補間点：端は元の頂点そのもの
    assert_eq!(subdivision.segments[2].end, route[2]);
}

#[test]
fn duplicate_source_rows_are_counted_separately() {
    // 元CSVの同じ内容の別行は除かない
    let route = east_1000m();
    let mid = 139.0 + 500.0 / KX35;
    let accidents = [Acc::at(mid, 35.0), Acc::at(mid, 35.0)];
    assert_eq!(ids(&hits(&route, &accidents, Mode::Walk)), [0, 1]);
}

/// 決定的な擬似乱数（線形合同法）。
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[test]
fn indexed_search_matches_exhaustive_search_on_random_inputs() {
    let mut rng = Lcg(42);
    for _ in 0..40 {
        let n = 2 + (rng.next() * 8.0) as usize;
        let mut lon = 139.0;
        let mut lat = 35.0;
        let mut route = vec![LonLat::new(lon, lat)];
        for _ in 1..n {
            lon += (rng.next() - 0.5) * 0.006;
            lat += (rng.next() - 0.5) * 0.006;
            route.push(LonLat::new(lon, lat));
        }
        let accidents: Vec<Acc> = (0..300)
            .map(|_| {
                Acc::at(138.995 + rng.next() * 0.02, 34.995 + rng.next() * 0.02)
                    .modes(1 + (rng.next() * 7.0) as u8)
            })
            .collect();
        for mode in Mode::ALL {
            hits(&route, &accidents, mode);
        }
    }
}
