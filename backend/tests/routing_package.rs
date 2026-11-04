//! M4.1：P9 `package` がschema 2のreleaseと8つの `.bin` からschema 3のreleaseを作ることの試験。
//!
//! 小さな合成グラフと人工releaseを使い、11ファイル・読み取り専用・`release_id` の再計算・
//! 上書きしないこと・`current` を作らないこと・代表的な入力の不一致を確かめる。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use accident_route_backend::data::release::{
    ACCIDENTS_FILE, HOLIDAYS_FILE, METADATA_FILE, ROUTING_FILES, SCHEMA_VERSION_ROUTING,
    routing_release_id, sha256_hex,
};
use accident_route_backend::data::routing_files::{
    Deps, encode_accident_attrs, encode_alt, encode_assignment, encode_graph, encode_odindex,
    encode_restrictions, encode_risk, encode_topology,
};
use accident_route_backend::data::routing_package::{
    PackageInputs, build_routing_metadata, package_routing_release,
};
use accident_route_backend::domain::routing::accident::{
    AccidentAttr, AccidentAttrTable, Assignment, UnassignedReason, UnassignedRecord,
};
use accident_route_backend::domain::routing::graph::Graph;
use accident_route_backend::domain::routing::graph::synthetic::grid;
use accident_route_backend::domain::routing::graph::time::PerMode;
use accident_route_backend::domain::routing::prep::attrs::GraphAttrs;
use accident_route_backend::domain::routing::prep::odindex::build_odindex;
use accident_route_backend::domain::routing::prep::topo::{TopologyConfig, build_topology};
use accident_route_backend::domain::routing::risk::{
    CoefRecord, RiskModel, RiskParams, TABLE_LEN, WeatherMultiplier,
};
use accident_route_backend::domain::routing::search::alt::Landmarks;
use accident_route_backend::domain::routing::topology::Topology;
use accident_route_backend::testing::fixtures::route_example;
use accident_route_backend::testing::release::write_example_release;
use serde_json::{Value, json};
use tempfile::TempDir;

const GENERATED_AT: &str = "2026-11-03T09:00:00+09:00";

struct Fixture {
    root: TempDir,
    base_release: PathBuf,
    routing_files: [PathBuf; 8],
    output: PathBuf,
    base_release_id: String,
    rows: u64,
    node_count: u64,
    edge_count: u64,
}

impl Fixture {
    fn inputs(&self) -> PackageInputs {
        PackageInputs {
            base_release: self.base_release.clone(),
            routing_files: self.routing_files.clone(),
            generated_at: GENERATED_AT.to_owned(),
        }
    }
}

fn risk_params() -> RiskParams {
    RiskParams {
        train_source_years: vec![2019, 2020, 2021, 2022],
        years: 4,
        c: 1.5,
        c_groups: vec![5, 6, 7, 8],
        car_exposure: None,
        beta: PerMode {
            walk: 0.2,
            bike: 0.3,
            car: 0.4,
        },
        beta_cluster: PerMode {
            walk: 0.25,
            bike: 0.35,
            car: 0.3,
        },
        weather_multiplier: WeatherMultiplier {
            rain: 2.0,
            fog: 4.0,
            snow: 4.0,
        },
        urban: json!({ "cell_e7": 50_000 }),
        pooling: json!({ "method": "none" }),
        fit: json!({ "deviance": 1.0 }),
    }
}

fn attrs_for(graph: &Graph) -> GraphAttrs {
    let edges = graph.edge_count();
    GraphAttrs {
        params: json!({ "edges": edges, "nodes": graph.node_count() }),
        way_id: (0..edges as i64).collect(),
        node_id: (0..graph.node_count() as i64).collect(),
        ref_idx: vec![0; edges],
        name_idx: vec![0; edges],
        strtab: vec![0],
        lanes: vec![0; edges],
        width_dm: vec![0; edges],
        layer: vec![0; edges],
    }
}

/// base のschema 2のreleaseと、それに合う8つの `.bin` を一時ディレクトリに作る。
fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let base_release = root.path().join("base");
    let example = route_example("twoSelected");
    let ids = write_example_release(&example, &base_release).unwrap();
    let rows = example.accident_ids.len() as u64;
    assert_eq!(rows, 3);

    let (builder, _shape) = grid(3, 4, 150.0);
    let graph = builder.build();
    let node_count = graph.node_count() as u64;
    let edge_count = graph.edge_count() as u64;

    let graph_bytes = encode_graph(&graph);
    let graph_hash = sha256_hex(&graph_bytes);
    let graph_deps = Deps::from([("graph.bin".to_owned(), graph_hash.clone())]);

    let restrictions = accident_route_backend::domain::routing::graph::restrictions::Restrictions {
        // payload_sha256 の試験で、JSONとして妥当なまま1バイトだけ変えられる値。
        params: json!({ "marker": 0 }),
        items: Vec::new(),
    };
    let restrictions_bytes = encode_restrictions(&restrictions, &graph_deps);

    let attrs = attrs_for(&graph);
    let topology: Topology = build_topology(
        graph.parts(),
        &attrs,
        &TopologyConfig {
            scc_min_nodes: 1,
            ..TopologyConfig::default()
        },
    )
    .unwrap();
    let topology_bytes = encode_topology(&topology, &graph_deps);
    let topology_hash = sha256_hex(&topology_bytes);
    let cluster_count = topology.clusters.len() as u64;

    let odindex = build_odindex(graph.parts());
    let odindex_bytes = encode_odindex(&odindex, &graph_deps);

    let accidents_bytes = fs::read(base_release.join(ACCIDENTS_FILE)).unwrap();
    let accidents_hash = sha256_hex(&accidents_bytes);
    let accident_attrs = AccidentAttrTable {
        params: json!({ "rows": rows }),
        attrs: vec![AccidentAttr::MISSING; rows as usize],
    };
    let accident_attrs_deps = Deps::from([(ACCIDENTS_FILE.to_owned(), accidents_hash.clone())]);
    let accident_attrs_bytes = encode_accident_attrs(&accident_attrs, &accident_attrs_deps);
    let accident_attrs_hash = sha256_hex(&accident_attrs_bytes);

    let assignment = Assignment {
        params: json!({
            "accidents": rows,
            "targets": 0,
            "unassigned": rows,
            "edges": edge_count,
            "clusters": cluster_count,
        }),
        offsets: vec![0; rows as usize + 1],
        targets: Vec::new(),
        unassigned: (0..rows)
            .map(|accident| UnassignedRecord {
                accident: accident as u32,
                reason: UnassignedReason::NoCandidate,
            })
            .collect(),
    };
    let assignment_deps = Deps::from([
        ("graph.bin".to_owned(), graph_hash.clone()),
        ("topology.bin".to_owned(), topology_hash.clone()),
        ("accident_attrs.bin".to_owned(), accident_attrs_hash.clone()),
        (ACCIDENTS_FILE.to_owned(), accidents_hash.clone()),
    ]);
    let assignment_bytes = encode_assignment(&assignment, &assignment_deps);
    let assignment_hash = sha256_hex(&assignment_bytes);

    let risk = RiskModel {
        params: risk_params(),
        raw_params: json!({
            "inherited_from": "risk-fit2019-2022",
            "validate_year": 2023,
            "fit": { "deviance": 1.0 },
            "pooling": { "method": "none" },
            "urban": { "cell_e7": 50_000 },
        }),
        edge_k: vec![0; edge_count as usize],
        cluster_j: vec![0; cluster_count as usize],
        k_table: vec![CoefRecord::default(); TABLE_LEN],
        j_table: vec![CoefRecord::default(); TABLE_LEN],
    };
    let risk_deps = Deps::from([
        ("graph.bin".to_owned(), graph_hash.clone()),
        ("topology.bin".to_owned(), topology_hash.clone()),
        ("accident_attrs.bin".to_owned(), accident_attrs_hash.clone()),
        ("assignment.bin".to_owned(), assignment_hash.clone()),
        (ACCIDENTS_FILE.to_owned(), accidents_hash.clone()),
    ]);
    let risk_bytes = encode_risk(&risk, &risk_deps);

    let alt_bytes = encode_alt(&Landmarks::empty(), &graph_deps);

    let bins = root.path().join("bins");
    fs::create_dir_all(&bins).unwrap();
    let paths = ROUTING_FILES.map(|file| bins.join(file));
    let contents: [&[u8]; 8] = [
        &graph_bytes,
        &restrictions_bytes,
        &topology_bytes,
        &odindex_bytes,
        &accident_attrs_bytes,
        &assignment_bytes,
        &risk_bytes,
        &alt_bytes,
    ];
    for (path, bytes) in paths.iter().zip(contents) {
        fs::write(path, bytes).unwrap();
    }

    let output = root.path().join("out");
    Fixture {
        root,
        base_release,
        routing_files: paths,
        output,
        base_release_id: ids.release_id,
        rows,
        node_count,
        edge_count,
    }
}

fn release_file_names(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

fn expected_file_names() -> BTreeSet<String> {
    let mut names: BTreeSet<String> = ROUTING_FILES
        .iter()
        .map(|file| (*file).to_owned())
        .collect();
    names.insert(ACCIDENTS_FILE.to_owned());
    names.insert(HOLIDAYS_FILE.to_owned());
    names.insert(METADATA_FILE.to_owned());
    names
}

fn assert_no_partial_output(output: &Path) {
    assert!(
        !output.join("current").exists(),
        "currentを作ってはいけない"
    );
    let staging = output.join("staging");
    if staging.exists() {
        assert_eq!(
            fs::read_dir(&staging).unwrap().count(),
            0,
            "staging/に途中のディレクトリが残っている"
        );
    }
    let releases = output.join("releases");
    if releases.exists() {
        assert_eq!(
            fs::read_dir(&releases).unwrap().count(),
            0,
            "失敗したのにreleaseが残っている"
        );
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
        .unwrap_or_else(|| panic!("{:?} が見つからない", String::from_utf8_lossy(needle)))
}

#[test]
fn package_writes_eleven_read_only_files_and_a_schema_3_release_id() {
    let f = fixture();
    let inputs = f.inputs();

    // 検査とmetadataの組み立てだけでは、出力を書かない。
    let (built_id, built_metadata) = build_routing_metadata(&inputs).unwrap();
    assert!(!f.output.exists());

    let summary = package_routing_release(&inputs, &f.output).unwrap();
    assert_eq!(summary.release_id, built_id);
    assert_eq!(summary.metadata, built_metadata);
    assert_eq!(summary.dataset_id, built_metadata["dataset_id"]);
    assert_eq!(
        summary.release_dir,
        f.output.join("releases").join(&built_id)
    );

    // 11ファイルができ、どれも読み取り専用。
    assert_eq!(
        release_file_names(&summary.release_dir),
        expected_file_names()
    );
    for name in expected_file_names() {
        let path = summary.release_dir.join(&name);
        assert!(
            fs::metadata(&path).unwrap().permissions().readonly(),
            "{name} が読み取り専用ではない"
        );
    }
    // `current` は作らない。
    assert!(!f.output.join("current").exists());

    // base の2ファイルはバイト列のまま。
    assert_eq!(
        fs::read(summary.release_dir.join(ACCIDENTS_FILE)).unwrap(),
        fs::read(f.base_release.join(ACCIDENTS_FILE)).unwrap()
    );
    assert_eq!(
        fs::read(summary.release_dir.join(HOLIDAYS_FILE)).unwrap(),
        fs::read(f.base_release.join(HOLIDAYS_FILE)).unwrap()
    );

    let metadata: Value =
        serde_json::from_slice(&fs::read(summary.release_dir.join(METADATA_FILE)).unwrap())
            .unwrap();
    assert_eq!(metadata["schema_version"], SCHEMA_VERSION_ROUTING);
    assert_eq!(metadata["release_id"], summary.release_id);
    assert_eq!(metadata["dataset_id"], summary.dataset_id);
    assert_eq!(metadata["rows"], f.rows);
    assert_eq!(metadata["generated_at"], GENERATED_AT);
    assert_eq!(metadata["processor"]["name"], "routing_prep");
    assert_eq!(metadata["processor"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(metadata["base_release"]["release_id"], f.base_release_id);
    assert!(metadata.get("elapsed_seconds").is_none());

    // `release_id` を実際の11ファイルから計算し直す。
    let dataset_id = sha256_hex(&fs::read(summary.release_dir.join(ACCIDENTS_FILE)).unwrap());
    let holiday_sha = sha256_hex(&fs::read(summary.release_dir.join(HOLIDAYS_FILE)).unwrap());
    let hashes: Vec<(&str, String)> = ROUTING_FILES
        .iter()
        .map(|file| {
            (
                *file,
                sha256_hex(&fs::read(summary.release_dir.join(file)).unwrap()),
            )
        })
        .collect();
    let pairs: Vec<(&str, String)> = hashes.clone();
    let pairs: Vec<(&str, &str)> = pairs.iter().map(|(f, h)| (*f, h.as_str())).collect();
    assert_eq!(
        routing_release_id(&dataset_id, &holiday_sha, &pairs),
        summary.release_id
    );

    // `routing.files` は `ROUTING_FILES` の順で、ファイル全体のハッシュと節の件数を持つ。
    let files = metadata["routing"]["files"].as_array().unwrap();
    assert_eq!(files.len(), 8);
    for (i, file) in ROUTING_FILES.iter().enumerate() {
        assert_eq!(files[i]["file"], *file);
        assert_eq!(files[i]["sha256"], hashes[i].1);
        assert_eq!(
            files[i]["bytes"].as_u64().unwrap(),
            fs::metadata(summary.release_dir.join(file)).unwrap().len()
        );
        assert_eq!(files[i]["format_version"], 1);
        assert!(files[i]["sections"].is_object(), "{file}");
    }

    assert_eq!(metadata["routing"]["rules_version"], "synthetic");
    assert_eq!(metadata["routing"]["graph"]["nodes"], f.node_count);
    assert_eq!(metadata["routing"]["graph"]["edges"], f.edge_count);
    assert_eq!(metadata["routing"]["graph"]["shape_points"], Value::from(0));
    assert_eq!(
        metadata["routing"]["inputs"]["accidents"]["release_id"],
        f.base_release_id
    );
    assert_eq!(
        metadata["routing"]["inputs"]["accidents"]["dataset_id"],
        summary.dataset_id
    );
    assert_eq!(
        metadata["routing"]["inputs"]["holiday_table"],
        metadata["holiday_table"]
    );
    // risk の主要部分だけを写し、fit・pooling・urban は写さない。
    let risk = &metadata["routing"]["risk"];
    assert_eq!(risk["years"], 4);
    assert_eq!(risk["c"], 1.5);
    assert_eq!(risk["inherited_from"], "risk-fit2019-2022");
    assert_eq!(risk["validate_year"], 2023);
    for key in ["fit", "pooling", "urban"] {
        assert!(risk.get(key).is_none(), "{key} を写してはいけない");
    }
    assert_eq!(metadata["routing"]["audit"]["unverified_deps"], json!([]));

    // 2回目は上書きしない。
    assert!(summary.release_dir.exists());
    let error = package_routing_release(&inputs, &f.output).unwrap_err();
    assert!(error.contains("既にあります"), "{error}");
    assert!(!f.output.join("current").exists());
    assert_eq!(
        release_file_names(&summary.release_dir),
        expected_file_names()
    );
}

#[test]
fn a_changed_payload_is_rejected_and_leaves_no_staging_directory() {
    let f = fixture();
    let restrictions = &f.routing_files[1];
    let mut bytes = fs::read(restrictions).unwrap();
    let at = find(&bytes, b"\"marker\":0");
    bytes[at + b"\"marker\":0".len() - 1] = b'1';
    fs::write(restrictions, &bytes).unwrap();

    let error = package_routing_release(&f.inputs(), &f.output).unwrap_err();
    assert!(error.contains("restrictions.bin"), "{error}");
    assert!(error.contains("payload_sha256"), "{error}");
    assert_no_partial_output(&f.output);
}

#[test]
fn deps_from_another_graph_are_rejected_and_leave_no_staging_directory() {
    let a = fixture();
    // 別のグラフ（4×4の格子）に対して作った topology.bin。
    let (builder, _shape) = grid(4, 4, 150.0);
    let other_graph = builder.build();
    let other_graph_hash = sha256_hex(&encode_graph(&other_graph));
    let other_topology = build_topology(
        other_graph.parts(),
        &attrs_for(&other_graph),
        &TopologyConfig {
            scc_min_nodes: 1,
            ..TopologyConfig::default()
        },
    )
    .unwrap();
    let other_topology_path = a.root.path().join("other-topology.bin");
    fs::write(
        &other_topology_path,
        encode_topology(
            &other_topology,
            &Deps::from([("graph.bin".to_owned(), other_graph_hash)]),
        ),
    )
    .unwrap();

    let mut inputs = a.inputs();
    inputs.routing_files[2] = other_topology_path;

    let error = package_routing_release(&inputs, &a.output).unwrap_err();
    assert!(error.contains("topology.bin"), "{error}");
    assert!(error.contains("graph.bin"), "{error}");
    assert!(error.contains("DEPS"), "{error}");
    assert_no_partial_output(&a.output);
}

#[test]
fn a_wrong_magic_number_is_rejected_and_leaves_no_staging_directory() {
    let f = fixture();
    // restrictions.bin の位置に topology.bin の中身を置く。
    fs::copy(&f.routing_files[2], &f.routing_files[1]).unwrap();

    let error = package_routing_release(&f.inputs(), &f.output).unwrap_err();
    assert!(error.contains("restrictions.bin"), "{error}");
    assert!(error.contains("マジックナンバー"), "{error}");
    assert_no_partial_output(&f.output);
}

#[test]
fn a_base_dataset_id_mismatch_is_rejected_and_leaves_no_staging_directory() {
    let f = fixture();
    let bad_base = f.root.path().join("bad-base");
    fs::create_dir_all(&bad_base).unwrap();
    for name in [ACCIDENTS_FILE, HOLIDAYS_FILE, METADATA_FILE] {
        fs::copy(f.base_release.join(name), bad_base.join(name)).unwrap();
    }
    let metadata_path = bad_base.join(METADATA_FILE);
    let mut metadata: Value = serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
    metadata["dataset_id"] = json!("0".repeat(64));
    fs::write(
        &metadata_path,
        serde_json::to_vec_pretty(&metadata).unwrap(),
    )
    .unwrap();

    let mut inputs = f.inputs();
    inputs.base_release = bad_base;
    let error = package_routing_release(&inputs, &f.output).unwrap_err();
    assert!(error.contains("dataset_id"), "{error}");
    assert_no_partial_output(&f.output);
}
