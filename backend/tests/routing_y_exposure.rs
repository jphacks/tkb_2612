//! `routing_eval y-metrics` の曝露の補正（`--exposure-factors`・`--exposure-clusters`）の結合試験。
//!
//! 合成グラフ（自専道と生活道路の2辺）の4ファイル・割り当て・事故を一時ディレクトリに書き、
//! 実際のコマンドを起動して、引数の検査と、補正なしの出力の不変・Y1x〜Y3xの値を確かめる。

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use accident_route_backend::data::release::{
    ACCIDENT_ATTRS_FILE, ACCIDENTS_FILE, ASSIGNMENT_FILE, GRAPH_FILE, ODINDEX_FILE, TOPOLOGY_FILE,
    sha256_hex,
};
use accident_route_backend::data::routing_files::{
    Deps, encode_assignment, encode_graph, encode_odindex, encode_topology,
};
use accident_route_backend::domain::routing::NONE;
use accident_route_backend::domain::routing::accident::{AssignTarget, Assignment, TargetKind};
use accident_route_backend::domain::routing::graph::synthetic::{EdgeSpec, GraphBuilder};
use accident_route_backend::domain::routing::graph::{Graph, HighwayClass};
use accident_route_backend::domain::routing::prep::odindex::build_odindex;
use accident_route_backend::domain::routing::snap::point_on_edge;
use accident_route_backend::domain::routing::topology::Topology;
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Value, json};
use std::io::Write;

/// 自専道の辺0と生活道路の辺1がつながったグラフ。
fn graph() -> Graph {
    let mut builder = GraphBuilder::new();
    let n0 = builder.add_node_deg(35.0, 139.0);
    let n1 = builder.add_node_deg(35.0, 139.001);
    let n2 = builder.add_node_deg(35.0, 139.002);
    builder.add_edge(EdgeSpec::new(n0, n1).class(HighwayClass::Motorway));
    builder.add_edge(EdgeSpec::new(n1, n2).class(HighwayClass::Residential));
    builder.build()
}

/// 全辺を全手段で吸着可にした位相（クラスタなし）。
fn topology(graph: &Graph) -> Topology {
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
        clusters: Vec::new(),
        cluster_nodes: Vec::new(),
        twins: Vec::new(),
        sidewalk_parents: Vec::new(),
    };
    topology.validate(graph).expect("位相が不正");
    topology
}

/// 4ファイル・割り当て・事故を書く。事故は辺0の450dmと辺1の450dmに1件ずつ。
fn write_world(dir: &Path, graph: &Graph) {
    let parts = graph.parts();
    let graph_bytes = encode_graph(graph);
    let deps = Deps::from([(GRAPH_FILE.to_owned(), sha256_hex(&graph_bytes))]);
    fs::write(dir.join(GRAPH_FILE), &graph_bytes).unwrap();
    let topology = topology(graph);
    let topology_bytes = encode_topology(&topology, &deps);
    fs::write(dir.join(TOPOLOGY_FILE), &topology_bytes).unwrap();
    fs::write(
        dir.join(ODINDEX_FILE),
        encode_odindex(&build_odindex(parts), &deps),
    )
    .unwrap();

    let target = |edge: u32, pos_dm: u32, weight: f32| AssignTarget {
        target: edge,
        pos_dm,
        weight,
        kind: TargetKind::Edge.code(),
        flags: 0,
        reserved: 0,
    };
    let assignment = Assignment {
        params: json!({"edges": graph.edge_count(), "clusters": 0}),
        offsets: vec![0, 1, 2],
        targets: vec![target(0, 450, 0.5), target(1, 450, 0.25)],
        unassigned: Vec::new(),
    };
    assignment.validate(graph.edge_count(), 0).unwrap();

    let p0 = point_on_edge(graph, 0, 450).to_latlon();
    let p1 = point_on_edge(graph, 1, 450).to_latlon();
    let mut csv = String::from(
        "lat,lon,modes,period,weather,day_type,deaths,injuries,source_year,date,hour,minute,daynight,accident_type,injury_a,injury_b,severity\n",
    );
    for p in [p0, p1] {
        csv.push_str(&format!(
            "{:.7},{:.7},7,0,1,0,0,1,2024,20240101,10,10,12,21,4,4,負傷\n",
            p.lat, p.lon
        ));
    }
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(csv.as_bytes()).unwrap();
    let gz = gz.finish().unwrap();
    fs::write(dir.join(ACCIDENTS_FILE), &gz).unwrap();

    let assignment_deps = Deps::from([
        (GRAPH_FILE.to_owned(), sha256_hex(&graph_bytes)),
        (TOPOLOGY_FILE.to_owned(), sha256_hex(&topology_bytes)),
        (
            ACCIDENT_ATTRS_FILE.to_owned(),
            sha256_hex(b"accident_attrs"),
        ),
        (ACCIDENTS_FILE.to_owned(), sha256_hex(&gz)),
    ]);
    fs::write(
        dir.join(ASSIGNMENT_FILE),
        encode_assignment(&assignment, &assignment_deps),
    )
    .unwrap();
}

fn route_line(graph: &Graph, id: &str, mode: &str) -> Value {
    let end = graph.edge(1).length_dm;
    json!({
        "id": id,
        "mode": mode,
        "candidates": [{
            "path": {
                "start": {"edge": 0, "offset_dm": 0},
                "end": {"edge": 1, "offset_dm": end},
                "dedges": [0, 2],
            },
            "tag": "a",
        }],
    })
}

fn run(dir: &Path, routes: &Path, out: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_routing_eval"))
        .args([
            "y-metrics",
            "--dir",
            dir.to_str().unwrap(),
            "--assignment",
            dir.join(ASSIGNMENT_FILE).to_str().unwrap(),
            "--accidents",
            dir.join(ACCIDENTS_FILE).to_str().unwrap(),
            "--routes",
            routes.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--years",
            "2024",
        ])
        .args(extra)
        .output()
        .expect("routing_evalを起動できる")
}

fn write_routes(dir: &Path, records: &[Value]) -> std::path::PathBuf {
    let path = dir.join("routes.jsonl");
    let text = records
        .iter()
        .map(|record| format!("{record}\n"))
        .collect::<String>();
    fs::write(&path, text).unwrap();
    path
}

fn read_records(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("1行が1つのJSON"))
        .collect()
}

#[test]
fn y_metrics_without_factors_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let graph = graph();
    write_world(dir.path(), &graph);
    let route = route_line(&graph, "car-1", "car");
    let routes = write_routes(dir.path(), std::slice::from_ref(&route));
    let out = dir.path().join("out.jsonl");
    let result = run(dir.path(), &routes, &out, &[]);
    assert!(
        result.status.success(),
        "y-metricsが失敗しました\n{}",
        String::from_utf8_lossy(&result.stderr)
    );

    // 係数がなければ、今までの `y`（Y1・Y2・Y3・length_m）だけを、同じ形で書く。
    let length_m = f64::from(graph.edge(0).length_dm + graph.edge(1).length_dm) / 10.0;
    let mut expected = route;
    expected["candidates"][0]["y"] = json!({"y1": 2, "y2": 2, "y3": 0.75, "length_m": length_m});
    assert_eq!(
        fs::read_to_string(&out).unwrap(),
        format!("{expected}\n"),
        "補正なしの出力が変わっています"
    );
}

#[test]
fn y_metrics_adds_the_weighted_y_with_the_group_factors() {
    let dir = tempfile::tempdir().unwrap();
    let graph = graph();
    write_world(dir.path(), &graph);
    let car = route_line(&graph, "car-1", "car");
    let walk = route_line(&graph, "walk-1", "walk");
    let routes = write_routes(dir.path(), &[car, walk]);
    let out = dir.path().join("out.jsonl");
    let result = run(
        dir.path(),
        &routes,
        &out,
        &["--exposure-factors", "motorway=0.27,arterial=1,local=15.0"],
    );
    assert!(
        result.status.success(),
        "y-metricsが失敗しました\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("曝露係数:"), "stderr={stderr}");
    assert!(stderr.contains("0=0.27"), "stderr={stderr}");
    assert!(stderr.contains("6=15"), "stderr={stderr}");

    let records = read_records(&out);
    let car = &records[0]["candidates"][0]["y"];
    assert_eq!(car["y1"], 2);
    assert_eq!(car["y2"], 2);
    assert!((car["y3"].as_f64().unwrap() - 0.75).abs() < 1e-9);
    assert!((car["y1x"].as_f64().unwrap() - (0.27 + 15.0)).abs() < 1e-9);
    assert!((car["y2x"].as_f64().unwrap() - (0.27 + 15.0)).abs() < 1e-9);
    assert!((car["y3x"].as_f64().unwrap() - (0.5 * 0.27 + 0.25 * 15.0)).abs() < 1e-6);

    // 徒歩は係数1（Yと同じ値）。
    let walk = &records[1]["candidates"][0]["y"];
    assert_eq!(walk["y1"], 2);
    assert_eq!(walk["y2"], 2);
    assert_eq!(walk["y3"], 0.75);
    assert_eq!(walk["y1x"], 2.0);
    assert_eq!(walk["y2x"], 2.0);
    assert_eq!(walk["y3x"], 0.75);
}

#[test]
fn y_metrics_rejects_invalid_exposure_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let graph = graph();
    write_world(dir.path(), &graph);
    let routes = write_routes(dir.path(), &[route_line(&graph, "car-1", "car")]);
    let out = dir.path().join("out.jsonl");
    let cases: [(&[&str], &str); 5] = [
        (&["--exposure-factors", "motorway=0.27,arterial=1"], "3つ"),
        (
            &["--exposure-factors", "motorway=0.27,arterial=1,local=-1"],
            "正の数",
        ),
        (
            &["--exposure-factors", "motorway=0.27,arterial=1,local=NaN"],
            "正の数",
        ),
        (
            &[
                "--exposure-factors",
                "motorway=0.27,arterial=1,local=15,other=1",
            ],
            "名前",
        ),
        (&["--exposure-clusters", "none"], "--exposure-factors"),
    ];
    for (extra, message) in cases {
        let result = run(dir.path(), &routes, &out, extra);
        assert!(
            !result.status.success(),
            "{extra:?} が成功しました: {}",
            String::from_utf8_lossy(&result.stdout)
        );
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            stderr.contains(message),
            "{extra:?} のエラーに「{message}」がありません: {stderr}"
        );
    }
}
