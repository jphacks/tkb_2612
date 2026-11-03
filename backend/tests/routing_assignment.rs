//! M2b：事故の割り当て（P5 `accident-snap`、`assignment.bin`、起動時の索引）。
//!
//! 合成グラフ（ランダムな道路網）と合成の事故で `routing_prep accident-snap` を通し、
//! 書いた `assignment.bin` を読み直して検査する（M0.6「割り当ての重み」：1件の重みの合計が1以下、
//! 各重みが0.1以上、候補が3個以下、高速・自専道のコードの事故は高速・自専道の辺にしか載らない、
//! 割り当てのある事故と帰属不明が排他で合わせて全事故）。同じ入力から同じバイト列になること
//! （スレッド数によらない）、入力の版が食い違えば失敗すること、確認用の抽出が作られることも確かめる。
//!
//! 全国のファイルを全件検査する重い試験は、末尾の `#[ignore]` の1件。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use accident_route_backend::data::load::parse_accidents;
use accident_route_backend::data::release::sha256_hex;
use accident_route_backend::data::routing_files::{
    Deps, decode_accident_attrs, decode_assignment, decode_graph_attrs, decode_graph_parts,
    decode_topology, encode_accident_attrs, encode_graph, encode_graph_attrs, encode_topology,
};
use accident_route_backend::domain::routing::accident::{
    AccidentAttr, AccidentAttrTable, EdgeAccidentIndex, MISSING_U8, RouteClass, UnassignedGrid,
    UnassignedReason,
};
use accident_route_backend::domain::routing::graph::synthetic::{
    RandomGraphConfig, Rng, random_graph,
};
use accident_route_backend::domain::routing::graph::{Graph, HighwayClass};
use accident_route_backend::domain::routing::prep::assign::{
    AssignParams, SnapInput, snap_accidents,
};
use accident_route_backend::domain::routing::prep::assign_audit::verify_assignment;
use accident_route_backend::domain::routing::prep::attrs::GraphAttrs;
use accident_route_backend::domain::routing::prep::topo::{TopologyConfig, build_topology};
use accident_route_backend::domain::routing::topology::Topology;
use accident_route_backend::domain::snapshot::AccidentSnapshot;
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Value, json};

const ACCIDENTS: usize = 700;

struct Fixture {
    dir: tempfile::TempDir,
    graph: Graph,
    attrs: GraphAttrs,
    topology: Topology,
    lat: Vec<f64>,
    lon: Vec<f64>,
    accident_attrs: Vec<AccidentAttr>,
}

impl Fixture {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
}

fn attr(route_upper: u16, road_shape: u8) -> AccidentAttr {
    AccidentAttr {
        route_upper,
        route_lower: 0,
        route_class: RouteClass::from_route_upper(route_upper).map_or(MISSING_U8, RouteClass::code),
        road_shape,
        carriageway_width: MISSING_U8,
        sidewalk_division: MISSING_U8,
        terrain: MISSING_U8,
    }
}

/// ランダムな道路網（高速・国道・住宅街・歩道などを含む）と、その近くの合成の事故。
/// `graph.bin`・`graph_attrs.bin`・`topology.bin`・`accident_attrs.bin`・`accidents.csv.gz` を書く。
fn fixture() -> Fixture {
    let mut config = RandomGraphConfig::new(90);
    config.extra_edges = 70;
    config.lat_e7 = (350_000_000, 350_020_000);
    config.lon_e7 = (1_390_000_000, 1_390_020_000);
    config.classes = vec![
        HighwayClass::Motorway,
        HighwayClass::MotorwayLink,
        HighwayClass::Trunk,
        HighwayClass::Primary,
        HighwayClass::Residential,
        HighwayClass::Residential,
        HighwayClass::Service,
        HighwayClass::Footway,
        HighwayClass::Track,
    ];
    config.oneway_prob.car = 0.3;
    let graph = random_graph(21, &config).build();
    let e = graph.edge_count();
    let attrs = GraphAttrs {
        params: json!({"edges": e, "nodes": graph.node_count()}),
        way_id: (0..e as i64).collect(),
        node_id: (0..graph.node_count() as i64).collect(),
        ref_idx: vec![0; e],
        name_idx: vec![0; e],
        strtab: vec![0],
        lanes: vec![0; e],
        width_dm: vec![0; e],
        layer: vec![0; e],
    };
    let topology = build_topology(
        graph.parts(),
        &attrs,
        &TopologyConfig {
            scc_min_nodes: 1,
            ..TopologyConfig::default()
        },
    )
    .unwrap();

    let mut rng = Rng::new(5);
    let routes = [16, 1010, 2100, 3100, 3100, 4100, 5100, 9900];
    let shapes = [1, 7, 11, 12, 13, 14, 14, 0, 31, 21, 37];
    let (mut lat, mut lon, mut accident_attrs) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..ACCIDENTS {
        let node = graph.node_coord(rng.below(graph.node_count()) as u32);
        lat.push(f64::from(node.lat_e7 + rng.range_i32(-300, 300)) / 1e7);
        lon.push(f64::from(node.lon_e7 + rng.range_i32(-300, 300)) / 1e7);
        accident_attrs.push(attr(*rng.pick(&routes), *rng.pick(&shapes)));
    }

    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, bytes: &[u8]| fs::write(dir.path().join(name), bytes).unwrap();
    let graph_bytes = encode_graph(&graph);
    let graph_deps = Deps::from([("graph.bin".to_owned(), sha256_hex(&graph_bytes))]);
    write("graph.bin", &graph_bytes);
    write("graph_attrs.bin", &encode_graph_attrs(&attrs, &graph_deps));
    write("topology.bin", &encode_topology(&topology, &graph_deps));

    let mut csv = String::from(
        "lat,lon,modes,period,weather,day_type,deaths,injuries,source_year,date,hour,minute,daynight,accident_type,injury_a,injury_b,severity\n",
    );
    for i in 0..ACCIDENTS {
        csv.push_str(&format!(
            "{:.7},{:.7},4,1,2,0,0,1,2024,20240101,10,10,12,21,4,4,負傷\n",
            lat[i], lon[i]
        ));
    }
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(csv.as_bytes()).unwrap();
    let gz = gz.finish().unwrap();
    write("accidents.csv.gz", &gz);
    let table = AccidentAttrTable {
        params: json!({"rows": ACCIDENTS}),
        attrs: accident_attrs.clone(),
    };
    let attrs_deps = Deps::from([("accidents.csv.gz".to_owned(), sha256_hex(&gz))]);
    write(
        "accident_attrs.bin",
        &encode_accident_attrs(&table, &attrs_deps),
    );
    Fixture {
        dir,
        graph,
        attrs,
        topology,
        lat,
        lon,
        accident_attrs,
    }
}

/// `accident-snap` を、`out` に書く設定で動かす。
fn run_snap(f: &Fixture, out: &Path, extra: &[&str]) -> Output {
    fs::create_dir_all(out).unwrap();
    Command::new(env!("CARGO_BIN_EXE_routing_prep"))
        .arg("accident-snap")
        .arg("--graph")
        .arg(f.path("graph.bin"))
        .arg("--attrs")
        .arg(f.path("graph_attrs.bin"))
        .arg("--topology")
        .arg(f.path("topology.bin"))
        .arg("--accident-attrs")
        .arg(f.path("accident_attrs.bin"))
        .arg("--accidents")
        .arg(f.path("accidents.csv.gz"))
        .arg("--output")
        .arg(out.join("assignment.bin"))
        .args(extra)
        .output()
        .unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn accident_snap_writes_a_verified_deterministic_assignment() {
    let f = fixture();
    let out1 = f.path("out1");
    let result = run_snap(
        &f,
        &out1,
        &[
            "--threads",
            "1",
            "--review-dir",
            out1.to_str().unwrap(),
            "--review-count",
            "40",
        ],
    );
    assert!(result.status.success(), "{}", stderr(&result));
    let bytes = fs::read(out1.join("assignment.bin")).unwrap();
    let decoded = decode_assignment(&bytes).unwrap();
    let a = &decoded.value;

    // M0.6「割り当ての重み」：全件の検査
    let stats = verify_assignment(a, f.graph.parts(), &f.topology, &f.accident_attrs).unwrap();
    assert_eq!(stats.accidents, ACCIDENTS);
    assert_eq!(stats.assigned + stats.unassigned, ACCIDENTS);
    assert_eq!(stats.unassigned, a.unassigned.len());
    assert!(stats.motorway_coded_assigned > 0 && stats.cluster_targets > 0);
    for accident in 0..ACCIDENTS {
        let targets = a.targets_of(accident);
        let direct: Vec<_> = targets.iter().filter(|t| !t.is_mirror()).collect();
        assert!(direct.len() <= 3);
        assert!(direct.iter().map(|t| f64::from(t.weight)).sum::<f64>() <= 1.0 + 1e-6);
        assert!(direct.iter().all(|t| t.weight >= 0.1));
        assert_eq!(
            targets.is_empty(),
            a.unassigned
                .binary_search_by_key(&(accident as u32), |r| r.accident)
                .is_ok()
        );
    }
    // 高速・自専道のコードの事故は、高速・自専道の辺（クラスタ）にしか載らず、逆も載らない
    let parts = f.graph.parts();
    let is_motorway_edge = |e: u32| {
        let r = &parts.edges[e as usize];
        r.class <= 1 || r.flags & 1 != 0
    };
    for (i, attr) in f.accident_attrs.iter().enumerate() {
        for t in a.targets_of(i).iter().filter(|t| t.kind == 0) {
            assert_eq!(
                is_motorway_edge(t.target),
                attr.is_motorway_code(),
                "事故{i}"
            );
        }
    }
    // 道路形状0は理由4
    for r in &a.unassigned {
        let shape = f.accident_attrs[r.accident as usize].road_shape;
        assert_eq!(
            shape == 0,
            r.reason == UnassignedReason::GeneralTrafficPlace
        );
    }

    // DEPS：入力のファイルのSHA-256
    for name in [
        "graph.bin",
        "topology.bin",
        "accident_attrs.bin",
        "accidents.csv.gz",
    ] {
        let actual = sha256_hex(&fs::read(f.path(name)).unwrap());
        assert_eq!(decoded.deps[name], actual, "{name}");
    }
    assert_eq!(decoded.deps.len(), 4);

    // ライブラリを直接呼んだ結果と一致する
    let input = SnapInput {
        graph: parts,
        graph_attrs: &f.attrs,
        topology: &f.topology,
        lat: &f.lat,
        lon: &f.lon,
        accident_attrs: &f.accident_attrs,
    };
    let direct = snap_accidents(&input, &AssignParams::default(), &|_| {}).unwrap();
    assert_eq!(&direct.assignment, a);

    // スレッド数によらず同じバイト列
    for threads in ["4", "9"] {
        let out = f.path(&format!("out_t{threads}"));
        let result = run_snap(&f, &out, &["--threads", threads]);
        assert!(result.status.success(), "{}", stderr(&result));
        assert_eq!(
            fs::read(out.join("assignment.bin")).unwrap(),
            bytes,
            "threads={threads}"
        );
    }

    // 監査JSON
    let audit: Value =
        serde_json::from_slice(&fs::read(out1.join("assignment.audit.json")).unwrap()).unwrap();
    assert_eq!(audit["command"], "accident-snap");
    assert_eq!(audit["verify"]["passed"], true);
    assert_eq!(audit["verify"]["accidents"], ACCIDENTS);
    assert_eq!(audit["output"]["sha256"], sha256_hex(&bytes));
    assert_eq!(audit["params"]["sigma0_m"], 3.5);
    assert_eq!(audit["summary"]["overall"]["accidents"], ACCIDENTS);
    assert!(
        audit["summary"]["by_route_group_and_shape_group"]
            .as_array()
            .unwrap()
            .len()
            > 5
    );
    assert!(audit["summary"]["route_number_match"]["national_route"].is_object());
    assert!(audit["params"]["tables"]["route_by_kind"].is_object());

    // 確認用の抽出
    let sample: Value =
        serde_json::from_slice(&fs::read(out1.join("review_sample.json")).unwrap()).unwrap();
    assert_eq!(sample["assignment_sha256"], sha256_hex(&bytes));
    let records = sample["records"].as_array().unwrap();
    assert_eq!(records.len(), 40);
    assert_eq!(sample["count"], 40);
    for record in records {
        for key in [
            "id",
            "stratum",
            "lat",
            "lon",
            "max_weight",
            "attrs",
            "targets",
            "others",
            "accident",
        ] {
            assert!(record.get(key).is_some(), "{key}");
        }
        let id = record["id"].as_u64().unwrap() as usize;
        assert_eq!(
            record["targets"].as_array().unwrap().len(),
            a.targets_of(id).len()
        );
        assert_eq!(
            record["unassigned_reason"].is_null(),
            !a.targets_of(id).is_empty()
        );
    }
}

#[test]
fn the_startup_index_and_grid_come_from_the_written_file() {
    let f = fixture();
    let out = f.path("out");
    let result = run_snap(&f, &out, &[]);
    assert!(result.status.success(), "{}", stderr(&result));
    let a = decode_assignment(&fs::read(out.join("assignment.bin")).unwrap())
        .unwrap()
        .value;
    let index = EdgeAccidentIndex::build(&a, f.graph.edge_count(), f.topology.cluster_count());
    assert_eq!(
        index.edge_entries.len() + index.cluster_entries.len(),
        a.targets.len()
    );
    // 辺ごとの要素は位置の昇順、各事故の割り当てがすべて索引に入っている
    for e in 0..f.graph.edge_count() as u32 {
        let entries = index.edge_entries(e);
        assert!(entries.windows(2).all(|w| w[0].pos_dm <= w[1].pos_dm));
    }
    for accident in 0..ACCIDENTS {
        for t in a.targets_of(accident) {
            let found = if t.kind == 0 {
                index.edge_entries(t.target).iter().any(|x| {
                    x.accident == accident as u32
                        && x.pos_dm == t.pos_dm
                        && x.weight == t.weight
                        && x.mirror == t.is_mirror()
                })
            } else {
                index
                    .cluster_entries(t.target)
                    .iter()
                    .any(|x| x.accident == accident as u32 && x.weight == t.weight)
            };
            assert!(found, "事故{accident}");
        }
    }
    // 帰属不明の事故の格子：全件が、事故の座標のセルに昇順で入る
    let columns = parse_accidents(&fs::read(f.path("accidents.csv.gz")).unwrap()).unwrap();
    let snapshot = AccidentSnapshot::new(columns);
    let grid = UnassignedGrid::build(&a, &snapshot);
    let in_grid: usize = grid.cells.values().map(Vec::len).sum();
    assert_eq!(in_grid, a.unassigned.len());
    for ids in grid.cells.values() {
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
    }
    for r in &a.unassigned {
        let c = &snapshot.columns();
        let cell = accident_route_backend::domain::snapshot::cell_of(
            c.lat[r.accident as usize],
            c.lon[r.accident as usize],
        );
        assert!(grid.cell(cell).contains(&r.accident));
    }
}

#[test]
fn accident_snap_rejects_mixed_versions_and_bad_arguments() {
    let f = fixture();
    // 別のグラフ用のtopology.bin（DEPSのgraph.binが合わない）
    let wrong = Deps::from([("graph.bin".to_owned(), sha256_hex(b"another graph"))]);
    fs::write(f.path("topology.bin"), encode_topology(&f.topology, &wrong)).unwrap();
    let out = f.path("out_a");
    let result = run_snap(&f, &out, &[]);
    assert!(!result.status.success());
    assert!(stderr(&result).contains("DEPS"), "{}", stderr(&result));
    assert!(!out.join("assignment.bin").exists());

    // 別の事故の表用のaccident_attrs.bin
    let f = fixture();
    let table = AccidentAttrTable {
        params: json!({"rows": ACCIDENTS}),
        attrs: f.accident_attrs.clone(),
    };
    let wrong = Deps::from([("accidents.csv.gz".to_owned(), sha256_hex(b"other"))]);
    fs::write(
        f.path("accident_attrs.bin"),
        encode_accident_attrs(&table, &wrong),
    )
    .unwrap();
    let result = run_snap(&f, &f.path("out_b"), &[]);
    assert!(!result.status.success());
    assert!(stderr(&result).contains("DEPS"), "{}", stderr(&result));

    // 件数の違い
    let f = fixture();
    let table = AccidentAttrTable {
        params: json!({"rows": ACCIDENTS - 1}),
        attrs: f.accident_attrs[..ACCIDENTS - 1].to_vec(),
    };
    let deps = Deps::from([(
        "accidents.csv.gz".to_owned(),
        sha256_hex(&fs::read(f.path("accidents.csv.gz")).unwrap()),
    )]);
    fs::write(
        f.path("accident_attrs.bin"),
        encode_accident_attrs(&table, &deps),
    )
    .unwrap();
    let result = run_snap(&f, &f.path("out_c"), &[]);
    assert!(!result.status.success());
    assert!(!f.path("out_c/assignment.bin").exists());

    // 引数の不足、不明な引数、不正な値
    let f = fixture();
    let routing_prep = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_routing_prep"))
            .arg("accident-snap")
            .args(args)
            .output()
            .unwrap()
    };
    assert!(!routing_prep(&[]).status.success());
    assert!(!routing_prep(&["--bogus", "x"]).status.success());
    let result = run_snap(&f, &f.path("out_d"), &["--sigma0", "abc"]);
    assert!(!result.status.success());
    assert!(!f.path("out_d/assignment.bin").exists());
    let result = run_snap(&f, &f.path("out_e"), &["--sigma0", "5.0"]);
    assert!(result.status.success(), "{}", stderr(&result));
    let a = decode_assignment(&fs::read(f.path("out_e/assignment.bin")).unwrap())
        .unwrap()
        .value;
    assert_eq!(a.params["sigma0_m"], 5.0);
}

/// 全国のファイル（M1とM2a、`accident-snap` の出力）を全件検査する。
///
/// ```sh
/// W=data/work
/// ROUTING_OSM_DIR=$W/M1/osm ROUTING_ACCIDENT_ATTRS=$W/M2a/accident_attrs.bin \
/// ROUTING_ACCIDENTS_GZ=$W/M2a/base/releases/<release_id>/accidents.csv.gz ROUTING_ASSIGNMENT=$W/M2/assignment.bin \
///   cargo test --release --manifest-path backend/Cargo.toml --test routing_assignment -- --ignored --nocapture
/// ```
#[test]
#[ignore = "全国のファイルが必要（ROUTING_OSM_DIR・ROUTING_ACCIDENT_ATTRS・ROUTING_ACCIDENTS_GZ・ROUTING_ASSIGNMENT）"]
fn nationwide_assignment_satisfies_every_invariant() {
    let env = |name: &str| {
        PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("{name} を指定してください")))
    };
    let osm = env("ROUTING_OSM_DIR");
    let read = |p: &Path| fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let graph_bytes = read(&osm.join("graph.bin"));
    let topology_bytes = read(&osm.join("topology.bin"));
    let accident_attrs_bytes = read(&env("ROUTING_ACCIDENT_ATTRS"));
    let accidents_bytes = read(&env("ROUTING_ACCIDENTS_GZ"));
    let assignment_bytes = read(&env("ROUTING_ASSIGNMENT"));

    let graph = decode_graph_parts(&graph_bytes).unwrap().value;
    let topology = decode_topology(&topology_bytes).unwrap().value;
    let accident_attrs = decode_accident_attrs(&accident_attrs_bytes)
        .unwrap()
        .value
        .attrs;
    let decoded = decode_assignment(&assignment_bytes).unwrap();
    let attrs = decode_graph_attrs(&read(&osm.join("graph_attrs.bin"))).unwrap();
    assert_eq!(attrs.value.ref_idx.len(), graph.edges.len());

    // 版の一致（DEPS）
    for (name, bytes) in [
        ("graph.bin", &graph_bytes),
        ("topology.bin", &topology_bytes),
        ("accident_attrs.bin", &accident_attrs_bytes),
        ("accidents.csv.gz", &accidents_bytes),
    ] {
        assert_eq!(decoded.deps[name], sha256_hex(bytes), "{name}");
    }

    // 全事故の件数
    let columns = parse_accidents(&accidents_bytes).unwrap();
    assert_eq!(columns.len(), accident_attrs.len());
    let a = &decoded.value;
    assert_eq!(a.accident_count(), columns.len());

    // M0.6「割り当ての重み」と、排他・全事故
    let stats = verify_assignment(a, &graph, &topology, &accident_attrs).unwrap();
    assert_eq!(stats.assigned + stats.unassigned, columns.len());
    eprintln!(
        "NATIONWIDE: accidents={} assigned={} unassigned={} targets={} edge_targets={} cluster_targets={} mirrors={} motorway_coded_assigned={}",
        stats.accidents,
        stats.assigned,
        stats.unassigned,
        stats.targets,
        stats.edge_targets,
        stats.cluster_targets,
        stats.mirror_targets,
        stats.motorway_coded_assigned
    );

    // 起動時の索引
    let index = EdgeAccidentIndex::build(a, graph.edges.len(), topology.cluster_count());
    assert_eq!(
        index.edge_entries.len() + index.cluster_entries.len(),
        a.targets.len()
    );
    let snapshot = AccidentSnapshot::new(columns);
    let grid = UnassignedGrid::build(a, &snapshot);
    assert_eq!(
        grid.cells.values().map(Vec::len).sum::<usize>(),
        a.unassigned.len()
    );
}
