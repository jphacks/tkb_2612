//! `routing_eval` の地域指定つき `od-sample` の結合試験（M5.2）。
//!
//! 8×8の合成格子（間隔300m）の4ファイルを一時ディレクトリに書き、格子を囲む小さな多角形を
//! GeoJSONで与えて、実際のコマンドを起動する。出力の全ODが多角形の内側・距離帯の中にあり、
//! 同じシードなら同じ出力になることを確かめる。

use std::fs;
use std::path::Path;
use std::process::Command;

use accident_route_backend::contract::LatLon;
use accident_route_backend::contract::geo::straight_distance_m;
use accident_route_backend::data::release::{
    GRAPH_FILE, ODINDEX_FILE, RESTRICTIONS_FILE, TOPOLOGY_FILE, sha256_hex,
};
use accident_route_backend::data::routing_files::{
    Deps, encode_graph, encode_odindex, encode_restrictions, encode_topology,
};
use accident_route_backend::domain::routing::eval::region::{
    Polygon, RegionFeature, RegionGeometry, feature_collection,
};
use accident_route_backend::domain::routing::graph::synthetic::grid;
use accident_route_backend::domain::routing::graph::{Coord, Graph};
use accident_route_backend::domain::routing::prep::odindex::build_odindex;
use accident_route_backend::domain::routing::topology::Topology;
use accident_route_backend::domain::routing::{NONE, Restrictions};
use serde_json::Value;

/// 全辺を全手段で吸着可にした位相。
fn all_snappable(graph: &Graph) -> Topology {
    let edges = graph.edge_count();
    let mut bits = vec![u64::MAX; edges.div_ceil(64)];
    if edges & 63 != 0
        && let Some(last) = bits.last_mut()
    {
        *last = (1_u64 << (edges % 64)) - 1;
    }
    let topology = Topology {
        params: Value::Null,
        snap_car: bits.clone(),
        snap_bike: bits.clone(),
        snap_walk: bits,
        node_cluster: vec![NONE; graph.node_count()],
        clusters: vec![],
        cluster_nodes: vec![],
        twins: vec![],
        sidewalk_parents: vec![],
    };
    topology.validate(graph).expect("位相が不正");
    topology
}

fn eval(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_routing_eval"))
        .args(args)
        .output()
        .expect("routing_evalを起動できる")
}

/// 格子の4ファイルを書き、格子を囲む多角形のGeoJSONを返す。
fn write_world(dir: &Path) -> String {
    let (builder, _) = grid(8, 8, 300.0);
    let graph = builder.build();
    let parts = builder.parts();
    let graph_bytes = encode_graph(&graph);
    let deps = Deps::from([(GRAPH_FILE.to_owned(), sha256_hex(&graph_bytes))]);
    fs::write(dir.join(GRAPH_FILE), &graph_bytes).unwrap();
    fs::write(
        dir.join(RESTRICTIONS_FILE),
        encode_restrictions(&Restrictions::empty(), &deps),
    )
    .unwrap();
    fs::write(
        dir.join(TOPOLOGY_FILE),
        encode_topology(&all_snappable(&graph), &deps),
    )
    .unwrap();
    fs::write(
        dir.join(ODINDEX_FILE),
        encode_odindex(&build_odindex(&parts), &deps),
    )
    .unwrap();

    let mut lat_min = i32::MAX;
    let mut lon_min = i32::MAX;
    let mut lat_max = i32::MIN;
    let mut lon_max = i32::MIN;
    for node in 0..graph.node_count() as u32 {
        let c = graph.node_coord(node);
        lat_min = lat_min.min(c.lat_e7);
        lon_min = lon_min.min(c.lon_e7);
        lat_max = lat_max.max(c.lat_e7);
        lon_max = lon_max.max(c.lon_e7);
    }
    let margin = 500_000; // 0.05度（約5km）。格子を確実に囲む。
    let outer = vec![
        Coord::new(lat_min - margin, lon_min - margin),
        Coord::new(lat_min - margin, lon_max + margin),
        Coord::new(lat_max + margin, lon_max + margin),
        Coord::new(lat_max + margin, lon_min - margin),
    ];
    let geometry = RegionGeometry::new(vec![Polygon::new(outer, vec![]).unwrap()]).unwrap();
    let text = serde_json::to_string(&feature_collection(&[RegionFeature {
        region: "test".to_owned(),
        relations: vec![(1, "テスト".to_owned())],
        geometry,
    }]))
    .unwrap();
    fs::write(dir.join("regions.geojson"), &text).unwrap();
    text
}

fn read_lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("1行が1つのJSON"))
        .collect()
}

fn latlon(value: &Value) -> LatLon {
    LatLon::new(
        value["lat"].as_f64().unwrap(),
        value["lon"].as_f64().unwrap(),
    )
}

#[test]
fn region_od_sample_is_inside_the_polygon_and_the_band_and_is_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let regions_text = write_world(dir.path());
    let regions = dir.path().join("regions.geojson");
    let a = dir.path().join("a.jsonl");
    let b = dir.path().join("b.jsonl");
    for out in [&a, &b] {
        let output = eval(&[
            "od-sample",
            "--dir",
            dir.path().to_str().unwrap(),
            "--mode",
            "walk",
            "--count",
            "20",
            "--seed",
            "5",
            "--min-m",
            "300",
            "--max-m",
            "1500",
            "--region",
            regions.to_str().unwrap(),
            "--region-id",
            "test",
            "--prefix",
            "test",
            "--out",
            out.to_str().unwrap(),
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("候補のノード"), "{stderr}");
    }
    assert_eq!(
        fs::read(&a).unwrap(),
        fs::read(&b).unwrap(),
        "同じシードなら同じ"
    );
    let region = RegionGeometry::from_geojson(&regions_text, "test").unwrap();
    let lines = read_lines(&a);
    assert_eq!(lines.len(), 20);
    for (i, line) in lines.iter().enumerate() {
        assert_eq!(line["id"], format!("test-walk-{:04}", i + 1));
        assert_eq!(line["mode"], "walk");
        assert_eq!(line["region"], "test");
        assert!(line["origin_node"].as_u64().is_some());
        assert!(line["dest_node"].as_u64().is_some());
        let origin = latlon(&line["o"]);
        let dest = latlon(&line["d"]);
        assert!(region.contains(Coord::from_latlon(origin)), "{origin:?}");
        assert!(region.contains(Coord::from_latlon(dest)), "{dest:?}");
        let straight = straight_distance_m(origin, dest);
        assert!((300.0..=1500.0).contains(&straight), "{straight}");
        assert!((straight - line["straight_m"].as_f64().unwrap()).abs() < 0.1);
    }
}

#[test]
fn region_od_sample_rejects_jitter_bbox_and_missing_region_id() {
    let dir = tempfile::tempdir().unwrap();
    write_world(dir.path());
    let regions = dir.path().join("regions.geojson");
    let out = dir.path().join("out.jsonl");

    let jittered = eval(&[
        "od-sample",
        "--dir",
        dir.path().to_str().unwrap(),
        "--mode",
        "walk",
        "--count",
        "1",
        "--seed",
        "5",
        "--min-m",
        "300",
        "--max-m",
        "1500",
        "--region",
        regions.to_str().unwrap(),
        "--region-id",
        "test",
        "--jitter-m",
        "5",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!jittered.status.success());
    assert!(
        String::from_utf8_lossy(&jittered.stderr).contains("--jitter-m"),
        "{}",
        String::from_utf8_lossy(&jittered.stderr)
    );

    let missing = eval(&[
        "od-sample",
        "--dir",
        dir.path().to_str().unwrap(),
        "--mode",
        "walk",
        "--count",
        "1",
        "--seed",
        "5",
        "--min-m",
        "300",
        "--max-m",
        "1500",
        "--region",
        regions.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("--region-id"),
        "{}",
        String::from_utf8_lossy(&missing.stderr)
    );
    assert!(!out.exists());

    let both = eval(&[
        "od-sample",
        "--dir",
        dir.path().to_str().unwrap(),
        "--mode",
        "walk",
        "--count",
        "1",
        "--seed",
        "5",
        "--min-m",
        "300",
        "--max-m",
        "1500",
        "--bbox",
        "35.0,139.0,36.0,140.0",
        "--region",
        regions.to_str().unwrap(),
        "--region-id",
        "test",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!both.status.success());
    assert!(
        String::from_utf8_lossy(&both.stderr).contains("--bbox"),
        "{}",
        String::from_utf8_lossy(&both.stderr)
    );
    assert!(!out.exists());
}
