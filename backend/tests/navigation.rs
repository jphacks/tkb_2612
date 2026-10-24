//! ナビ用の全対象区間と表示用の高リスク区間（behavior.md 7章、11章 項目12、#8 ステップ12）。
//!
//! `docs/api/fixtures/navigation-cases.json` の全ケースを、geometryとsid別の重みから再計算する。
//! 距離の許容差は `distance_tolerance_m`（1e-6m）、整数・順序・対象採否は完全一致。

use serde_json::Value;

use accident_route_backend::contract::geo::METERS_PER_DEGREE;
use accident_route_backend::contract::{LonLat, NavigationData};
use accident_route_backend::domain::corridor::Subdivision;
use accident_route_backend::domain::navigation::{display_sections, navigation_data};
use accident_route_backend::testing::fixtures::docs_fixture_path;

fn cases() -> (Vec<Value>, f64) {
    let doc: Value = serde_json::from_str(
        &std::fs::read_to_string(docs_fixture_path("navigation-cases.json")).unwrap(),
    )
    .unwrap();
    let tolerance = doc["distance_tolerance_m"].as_f64().unwrap();
    (doc["cases"].as_array().unwrap().clone(), tolerance)
}

fn coordinates(geometry: &Value) -> Vec<LonLat> {
    geometry["coordinates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| LonLat::new(p[0].as_f64().unwrap(), p[1].as_f64().unwrap()))
        .collect()
}

fn close(actual: f64, expected: &Value, tolerance: f64, what: &str) {
    let expected = expected.as_f64().unwrap();
    assert!(
        (actual - expected).abs() <= tolerance,
        "{what}: {actual} != {expected}"
    );
}

#[test]
fn every_navigation_case_matches() {
    let (cases, tolerance) = cases();
    assert!(!cases.is_empty());
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let subdivision = Subdivision::new(&coordinates(&case["geometry"]));
        let mut weights = vec![0u64; subdivision.segments.len()];
        for w in case["segment_severity_weights"].as_array().unwrap() {
            weights[w["segment_index"].as_u64().unwrap() as usize] =
                w["severity_weight"].as_u64().unwrap();
        }
        let expected = &case["expected"];
        assert_eq!(
            subdivision.segments.len() as u64,
            expected["subsegment_count"].as_u64().unwrap(),
            "{name}: subsegment_count"
        );
        let (display, total) = display_sections(&subdivision, &weights);
        assert_eq!(
            total,
            expected["high_risk_total"].as_u64().unwrap(),
            "{name}: high_risk_total"
        );
        let display_ids: Vec<u64> = display.iter().map(|s| u64::from(s.segment_index)).collect();
        let expected_ids: Vec<u64> = expected["display_segment_indices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .collect();
        assert_eq!(display_ids, expected_ids, "{name}: display_segment_indices");

        let nav = navigation_data(&subdivision, &weights);
        let e = &expected["navigation"];
        close(
            nav.projection_lat_deg,
            &e["projection_lat_deg"],
            1e-12,
            &format!("{name}: φ"),
        );
        close(
            nav.length_m,
            &e["length_m"],
            tolerance,
            &format!("{name}: length_m"),
        );
        let distances = e["vertex_distances_m"].as_array().unwrap();
        assert_eq!(nav.vertex_distances_m.len(), distances.len(), "{name}");
        for (a, b) in nav.vertex_distances_m.iter().zip(distances) {
            close(*a, b, tolerance, &format!("{name}: vertex_distances_m"));
        }
        let sections = e["sections"].as_array().unwrap();
        assert_eq!(nav.sections.len(), sections.len(), "{name}: 区間数");
        for (s, t) in nav.sections.iter().zip(sections) {
            assert_eq!(
                u64::from(s.start_segment_index),
                t["start_segment_index"].as_u64().unwrap(),
                "{name}"
            );
            assert_eq!(
                u64::from(s.end_segment_index),
                t["end_segment_index"].as_u64().unwrap(),
                "{name}"
            );
            assert_eq!(
                s.severity_weight,
                t["severity_weight"].as_u64().unwrap(),
                "{name}"
            );
            close(
                s.start_m,
                &t["start_m"],
                tolerance,
                &format!("{name}: start_m"),
            );
            close(s.end_m, &t["end_m"], tolerance, &format!("{name}: end_m"));
            // IDは③が組み立てる。区間の両端と一致することだけ確かめる
            let id = format!(
                "route-1:nav-{}-{}",
                s.start_segment_index, s.end_segment_index
            );
            assert_eq!(
                t["id"].as_str().unwrap().split_once(':').unwrap().1,
                id.split_once(':').unwrap().1
            );
        }
        check_correlation(&nav, total, &subdivision);
    }
}

/// ナビ区間の相関制約（behavior.md 7章 5.）。
fn check_correlation(nav: &NavigationData, high_risk_total: u64, subdivision: &Subdivision) {
    let d = &nav.vertex_distances_m;
    assert_eq!(d.len(), subdivision.edge_lengths.len() + 1);
    assert_eq!(d[0], 0.0);
    assert_eq!(*d.last().unwrap(), nav.length_m);
    assert!(d.windows(2).all(|w| w[0] <= w[1]));
    let mut covered = 0u64;
    let mut previous: Option<u32> = None;
    for s in &nav.sections {
        assert!(0.0 <= s.start_m && s.start_m <= s.end_m && s.end_m <= nav.length_m);
        assert!(s.start_segment_index <= s.end_segment_index);
        if let Some(e) = previous {
            assert!(
                s.start_segment_index > e + 1,
                "区間は重複せず、非対象を挟む"
            );
        }
        assert!(s.severity_weight >= 3);
        covered += u64::from(s.end_segment_index - s.start_segment_index + 1);
        previous = Some(s.end_segment_index);
    }
    assert_eq!(covered, high_risk_total, "Σ(E−S+1)==high_risk_total");
}

#[test]
fn more_than_100_targets_are_all_navigation_sections() {
    // 300mごとに対象と非対象が交互：表示は100件で切るが、ナビは全対象
    let kx = METERS_PER_DEGREE * (35f64 * std::f64::consts::PI / 180.0).cos();
    let route = [
        LonLat::new(139.0, 35.0),
        LonLat::new(139.0 + 30_000.0 / kx, 35.0),
    ];
    let subdivision = Subdivision::new(&route);
    let n = subdivision.segments.len();
    assert_eq!(n, 400);
    let weights: Vec<u64> = (0..n)
        .map(|sid| {
            if sid % 2 == 0 {
                3 + (sid as u64 % 5)
            } else {
                2
            }
        })
        .collect();
    let (display, total) = display_sections(&subdivision, &weights);
    assert_eq!(total, 200);
    assert_eq!(display.len(), 100);
    // 重み降順・同値はsid昇順
    assert!(display.windows(2).all(|w| (
        std::cmp::Reverse(w[0].severity_weight),
        w[0].segment_index
    ) < (
        std::cmp::Reverse(w[1].severity_weight),
        w[1].segment_index
    )));
    let nav = navigation_data(&subdivision, &weights);
    assert_eq!(nav.sections.len(), 200);
    check_correlation(&nav, total, &subdivision);
}

#[test]
fn large_routes_are_not_truncated() {
    // 15,000頂点・約80km・0長辺多数。対象と非対象を交互にして区間数を最大にする
    let kx = METERS_PER_DEGREE * (35f64 * std::f64::consts::PI / 180.0).cos();
    let mut route = Vec::with_capacity(15_000);
    for i in 0..15_000 {
        // 2頂点ずつ同じ座標（0長辺）にする
        let step = (i / 2) as f64;
        route.push(LonLat::new(139.0 + step * (80_000.0 / 7_500.0) / kx, 35.0));
    }
    let subdivision = Subdivision::new(&route);
    let n = subdivision.segments.len();
    assert!(n >= 14_999);
    let weights: Vec<u64> = (0..n).map(|sid| if sid % 2 == 0 { 3 } else { 0 }).collect();
    let (_, total) = display_sections(&subdivision, &weights);
    let nav = navigation_data(&subdivision, &weights);
    assert_eq!(nav.vertex_distances_m.len(), 15_000);
    assert!(nav.length_m <= 110_000.0);
    assert_eq!(nav.sections.len() as u64, total);
    check_correlation(&nav, total, &subdivision);

    // 全対象なら1区間。重みの和はu64
    let heavy = vec![u64::from(u32::MAX); n];
    let nav = navigation_data(&subdivision, &heavy);
    assert_eq!(nav.sections.len(), 1);
    assert_eq!(
        nav.sections[0].severity_weight,
        u64::from(u32::MAX) * n as u64
    );
}

#[test]
fn all_zero_length_route_has_zero_length() {
    let route = [
        LonLat::new(139.0, 35.0),
        LonLat::new(139.0, 35.0),
        LonLat::new(139.0, 35.0),
    ];
    let subdivision = Subdivision::new(&route);
    let nav = navigation_data(&subdivision, &[3, 4]);
    assert_eq!(nav.length_m, 0.0);
    assert_eq!(nav.vertex_distances_m, [0.0, 0.0, 0.0]);
    assert_eq!(nav.sections.len(), 1);
    assert_eq!((nav.sections[0].start_m, nav.sections[0].end_m), (0.0, 0.0));
}

#[test]
fn maximum_section_count_at_the_schema_bound() {
    // 関数単体の上限試験：behavior.md 7章の保守的な上限（元辺14,999本、小線分16,466本、
    // 結合後8,233区間）に届く入力。1,467本の辺は100m（2分割）、残りは0長辺（1本）にする。
    // 有効な経路（80km以下）ではここまで届かないため、経路の検証を通さずに関数へ渡す
    let kx = METERS_PER_DEGREE * (35f64 * std::f64::consts::PI / 180.0).cos();
    let mut route = vec![LonLat::new(139.0, 35.0)];
    let mut lon = 139.0;
    for edge in 0..14_999 {
        if edge < 1_467 {
            lon += 100.0 / kx;
        }
        route.push(LonLat::new(lon, 35.0));
    }
    let subdivision = Subdivision::new(&route);
    assert_eq!(subdivision.edge_lengths.len(), 14_999);
    assert_eq!(subdivision.segments.len(), 16_466);
    let weights: Vec<u64> = (0..16_466)
        .map(|sid| if sid % 2 == 0 { 3 } else { 2 })
        .collect();
    let (display, total) = display_sections(&subdivision, &weights);
    assert_eq!(total, 8_233);
    assert_eq!(display.len(), 100);
    let nav = navigation_data(&subdivision, &weights);
    assert_eq!(nav.sections.len(), 8_233);
    assert_eq!(nav.vertex_distances_m.len(), 15_000);
    check_correlation(&nav, total, &subdivision);
}
