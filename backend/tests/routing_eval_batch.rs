//! `routing_eval` の `od-sample`・`fastest-batch` の結合試験（作業書M1.7・M1.8）。
//!
//! 小さな合成グラフ（8×8の格子と、離れた2ノードだけの孤立した成分）の4ファイルを一時ディレクトリに書き、
//! 実際のコマンドを起動して、出力のJSON Linesを確かめる。
//!
//! - `od-sample`：件数、距離帯、シード固定で同じ出力、不正な引数の拒否。
//! - `fastest-batch`：成功した経路の合法性（違反なし）・時間の正本との一致・座標、吸着できない組、
//!   別の連結成分、同じ位置、手段ごとの結果。

use std::fs;
use std::path::Path;
use std::process::Command;

use accident_route_backend::contract::geo::straight_distance_m;
use accident_route_backend::contract::{LatLon, Mode};
use accident_route_backend::data::release::{
    GRAPH_FILE, ODINDEX_FILE, RESTRICTIONS_FILE, TOPOLOGY_FILE, sha256_hex,
};
use accident_route_backend::data::routing_files::{
    Deps, encode_graph, encode_odindex, encode_restrictions, encode_topology,
};
use accident_route_backend::domain::routing::graph::synthetic::{
    EdgeSpec, GraphBuilder, ORIGIN, grid,
};
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

/// 8×8の格子（間隔300m）に、北へ約3km離れた2ノードの辺（孤立した成分）を足した世界を書く。
/// 戻り値は、格子の南西端の座標（度）と、孤立した辺の中点の座標（度）。
fn write_world(dir: &Path) -> (LatLon, LatLon) {
    let (mut builder, _shape): (GraphBuilder, _) = grid(8, 8, 300.0);
    let far_a = Coord::new(ORIGIN.lat_e7 + 300_000, ORIGIN.lon_e7);
    let far_b = Coord::new(ORIGIN.lat_e7 + 300_000, ORIGIN.lon_e7 + 5_000);
    let a = builder.add_node(far_a);
    let b = builder.add_node(far_b);
    builder.add_edge(EdgeSpec::new(a, b));
    let graph = builder.build();
    let parts = builder.parts();

    let graph_bytes = encode_graph(&graph);
    let mut deps = Deps::new();
    deps.insert(GRAPH_FILE.to_owned(), sha256_hex(&graph_bytes));
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
    let mid = |p: Coord, q: Coord| {
        let half = |a: i32, b: i32| ((i64::from(a) + i64::from(b)) / 2) as i32;
        Coord::new(half(p.lat_e7, q.lat_e7), half(p.lon_e7, q.lon_e7)).to_latlon()
    };
    (ORIGIN.to_latlon(), mid(far_a, far_b))
}

fn eval(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_routing_eval"))
        .args(args)
        .output()
        .expect("routing_evalを起動できる")
}

fn eval_ok(args: &[&str]) {
    let out = eval(args);
    assert!(
        out.status.success(),
        "routing_eval {args:?} が失敗しました\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn read_lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).expect("1行が1つのJSON"))
        .collect()
}

fn latlon(v: &Value) -> LatLon {
    LatLon::new(v["lat"].as_f64().unwrap(), v["lon"].as_f64().unwrap())
}

fn od_line(id: &str, mode: &str, o: LatLon, d: LatLon) -> String {
    format!(
        r#"{{"id":"{id}","mode":"{mode}","o":{{"lat":{},"lon":{}}},"d":{{"lat":{},"lon":{}}}}}"#,
        o.lat, o.lon, d.lat, d.lon
    )
}

#[test]
fn od_sample_is_deterministic_and_inside_the_band() {
    let dir = tempfile::tempdir().unwrap();
    let (sw, _) = write_world(dir.path());
    let d = dir.path().to_str().unwrap();
    let a = dir.path().join("a.jsonl");
    let b = dir.path().join("b.jsonl");
    for out in [&a, &b] {
        eval_ok(&[
            "od-sample",
            "--dir",
            d,
            "--mode",
            "car",
            "--count",
            "25",
            "--seed",
            "7",
            "--min-m",
            "300",
            "--max-m",
            "1500",
            "--prefix",
            "t-",
            "--out",
            out.to_str().unwrap(),
        ]);
    }
    assert_eq!(
        fs::read(&a).unwrap(),
        fs::read(&b).unwrap(),
        "シードが同じなら同じ"
    );
    let lines = read_lines(&a);
    assert_eq!(lines.len(), 25);
    for (i, l) in lines.iter().enumerate() {
        assert_eq!(l["id"], format!("t-car-{:04}", i + 1));
        assert_eq!(l["mode"], "car");
        let straight = straight_distance_m(latlon(&l["o"]), latlon(&l["d"]));
        assert!((300.0..=1500.0).contains(&straight), "{straight}");
        assert!((straight - l["straight_m"].as_f64().unwrap()).abs() < 0.1);
        // 格子の内側（南西端の近く）
        assert!((latlon(&l["o"]).lat - sw.lat).abs() < 0.05);
    }
    // 別のシードなら別の組
    let c = dir.path().join("c.jsonl");
    eval_ok(&[
        "od-sample",
        "--dir",
        d,
        "--mode",
        "car",
        "--count",
        "25",
        "--seed",
        "8",
        "--min-m",
        "300",
        "--max-m",
        "1500",
        "--out",
        c.to_str().unwrap(),
    ]);
    assert_ne!(read_lines(&c)[0]["o"], lines[0]["o"]);
}

#[test]
fn od_sample_rejects_bad_arguments() {
    let dir = tempfile::tempdir().unwrap();
    write_world(dir.path());
    let d = dir.path().to_str().unwrap();
    let out = dir.path().join("x.jsonl");
    let o = out.to_str().unwrap();
    let base = |extra: &[&str]| {
        let mut args = vec![
            "od-sample",
            "--dir",
            d,
            "--seed",
            "1",
            "--count",
            "1",
            "--out",
            o,
        ];
        args.extend_from_slice(extra);
        eval(&args)
    };
    // 距離帯が契約の範囲（20〜30,000m）の外、または逆
    assert!(
        !base(&["--mode", "car", "--min-m", "5", "--max-m", "100"])
            .status
            .success()
    );
    assert!(
        !base(&["--mode", "car", "--min-m", "500", "--max-m", "40000"])
            .status
            .success()
    );
    assert!(
        !base(&["--mode", "car", "--min-m", "900", "--max-m", "800"])
            .status
            .success()
    );
    // 手段・範囲の指定が不正
    assert!(
        !base(&["--mode", "boat", "--min-m", "100", "--max-m", "800"])
            .status
            .success()
    );
    assert!(
        !base(&[
            "--mode", "car", "--min-m", "100", "--max-m", "800", "--bbox", "1,2,3"
        ])
        .status
        .success()
    );
    assert!(!out.exists());
}

#[test]
fn fastest_batch_reports_each_outcome() {
    let dir = tempfile::tempdir().unwrap();
    let (sw, far) = write_world(dir.path());
    let d = dir.path().to_str().unwrap();
    let origin = |dlat: f64, dlon: f64| LatLon::new(sw.lat + dlat, sw.lon + dlon);
    let ods = [
        // 格子の中：成功
        od_line(
            "ok-car",
            "car",
            origin(0.0005, 0.0005),
            origin(0.0150, 0.0160),
        ),
        od_line(
            "ok-bike",
            "bike",
            origin(0.0005, 0.0005),
            origin(0.0150, 0.0160),
        ),
        od_line(
            "ok-walk",
            "walk",
            origin(0.0005, 0.0005),
            origin(0.0150, 0.0160),
        ),
        // 目的地が海の上（吸着できない）
        od_line("no-snap", "car", origin(0.0005, 0.0005), origin(0.30, 0.30)),
        // 孤立した成分への経路（探索しない）
        od_line("island", "car", origin(0.0005, 0.0005), far),
        // 出発と到着が同じ位置
        od_line(
            "same",
            "car",
            origin(0.0005, 0.0005),
            origin(0.0005, 0.0005),
        ),
    ];
    let ods_path = dir.path().join("ods.jsonl");
    fs::write(&ods_path, ods.join("\n") + "\n").unwrap();
    let out = dir.path().join("out.jsonl");
    let args = |threads: &str, out: &Path| {
        vec![
            "fastest-batch".to_owned(),
            "--dir".to_owned(),
            d.to_owned(),
            "--ods".to_owned(),
            ods_path.to_str().unwrap().to_owned(),
            "--out".to_owned(),
            out.to_str().unwrap().to_owned(),
            "--threads".to_owned(),
            threads.to_owned(),
            "--twins".to_owned(),
            "true".to_owned(),
        ]
    };
    let run = |threads: &str, out: &Path| {
        let a = args(threads, out);
        let a: Vec<&str> = a.iter().map(String::as_str).collect();
        eval_ok(&a);
    };
    run("1", &out);
    let rows = read_lines(&out);
    assert_eq!(rows.len(), ods.len(), "入力と同じ順序・同じ件数");
    let by_id = |id: &str| rows.iter().find(|r| r["id"] == id).unwrap();

    for (id, mode) in [
        ("ok-car", Mode::Car),
        ("ok-bike", Mode::Bike),
        ("ok-walk", Mode::Walk),
    ] {
        let r = by_id(id);
        assert_eq!(r["status"], "ok", "{id}: {r}");
        assert_eq!(r["mode"], mode.as_str());
        assert_eq!(r["violations"], Value::Array(vec![]), "{id}");
        assert_eq!(r["time_matches_canonical"], true, "{id}");
        assert!(r["time_s"].as_f64().unwrap() > 0.0);
        let distance = r["distance_m"].as_f64().unwrap();
        let straight = r["straight_m"].as_f64().unwrap();
        // 格子の道は東西・南北だけなので、道のりは直線距離以上（1e-3の丸めを許す）、2倍未満
        assert!(
            distance >= straight - 1.0 && distance < straight * 2.0,
            "{distance} {straight}"
        );
        assert!(r["snap_ms"].as_f64().unwrap() >= 0.0);
        assert!(r["search_ms"].as_f64().unwrap() >= 0.0);
        assert!(r["total_ms"].as_f64().unwrap() >= r["search_ms"].as_f64().unwrap());
        assert_eq!(r["snap_o"]["ok"], true);
        assert!(r["snap_o"]["dist_m"].as_f64().unwrap() <= 300.0);
        // 座標：出発の吸着点から到着の吸着点まで。折れ線の長さが道のりと（丸めの範囲で）一致する
        let coords = r["coords"].as_array().unwrap();
        assert!(coords.len() >= 2);
        assert_eq!(r["vertices"], coords.len());
        let point = |v: &Value| LatLon::new(v[0].as_f64().unwrap(), v[1].as_f64().unwrap());
        let length: f64 = coords
            .windows(2)
            .map(|w| straight_distance_m(point(&w[0]), point(&w[1])))
            .sum();
        assert!(
            (length - distance).abs() < distance * 0.01 + 1.0,
            "{length} {distance}"
        );
        let (first, last) = (point(&coords[0]), point(&coords[coords.len() - 1]));
        assert!(straight_distance_m(first, latlon(&r["snap_o"])) < 1.0);
        assert!(straight_distance_m(last, latlon(&r["snap_d"])) < 1.0);
        // 回帰用の説明変数：走行距離の合計が道のりと一致する
        let f = &r["features"];
        let by_class: f64 = f["class_m"]
            .as_object()
            .unwrap()
            .values()
            .map(|v| v.as_f64().unwrap())
            .sum();
        let by_ms: f64 = f["ms_m"]
            .as_object()
            .unwrap()
            .values()
            .map(|v| v.as_f64().unwrap())
            .sum();
        assert!((by_class + by_ms - distance).abs() < 0.5, "{id}");
        assert_eq!(f["node_masks"].as_array().unwrap().len(), 8);
        // 双子は付けていない合成グラフ
        assert_eq!(r["twin_lines"], Value::Array(vec![]));
    }
    // 自動車は、歩行者より速い
    assert!(by_id("ok-car")["time_s"].as_f64() < by_id("ok-walk")["time_s"].as_f64());

    let r = by_id("no-snap");
    assert_eq!(r["status"], "no_snap_dest");
    assert_eq!(r["snap_o"]["ok"], true);
    assert_eq!(r["snap_d"]["ok"], false);
    assert!(r.get("time_s").is_none());

    let r = by_id("island");
    assert_eq!(r["status"], "no_route");
    assert_eq!(r["reason"], "disconnected_components");
    assert_eq!(r["search_skipped"], true);
    assert_eq!(r["comp_nodes_d"], 2);
    assert!(r["comp_nodes_o"].as_u64().unwrap() >= 64);

    assert_eq!(by_id("same")["status"], "same_position");

    // スレッド数を変えても、時間・距離・座標は同じ（処理時間の欄だけが違う）
    let out4 = dir.path().join("out4.jsonl");
    run("4", &out4);
    let strip = |rows: Vec<Value>| -> Vec<Value> {
        rows.into_iter()
            .map(|mut r| {
                let o = r.as_object_mut().unwrap();
                for k in [
                    "snap_ms",
                    "search_ms",
                    "total_ms",
                    "threads",
                    "touched_nodes",
                    "touched_states",
                ] {
                    o.remove(k);
                }
                r
            })
            .collect()
    };
    assert_eq!(strip(read_lines(&out4)), strip(rows));
}

#[test]
fn fastest_batch_rejects_bad_input() {
    let dir = tempfile::tempdir().unwrap();
    write_world(dir.path());
    let d = dir.path().to_str().unwrap();
    let out = dir.path().join("out.jsonl");
    let bad = dir.path().join("bad.jsonl");
    fs::write(&bad, "{\"id\":\"x\",\"mode\":\"boat\"}\n").unwrap();
    let r = eval(&[
        "fastest-batch",
        "--dir",
        d,
        "--ods",
        bad.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(!r.status.success());
    assert!(!out.exists());
    // --ods がない
    let r = eval(&["fastest-batch", "--dir", d, "--out", out.to_str().unwrap()]);
    assert!(!r.status.success());
    // --threads 0
    fs::write(&bad, "").unwrap();
    let r = eval(&[
        "fastest-batch",
        "--dir",
        d,
        "--ods",
        bad.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--threads",
        "0",
    ]);
    assert!(!r.status.success());
}
