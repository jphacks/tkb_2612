//! M4.2：schema 3のreleaseの起動時ロード（形式書4.3節）の試験。
//!
//! 人工releaseをコピーして1つずつ壊し、`data::load_engine` が `LoadError::Invalid` を返すことを確かめる。
//! あわせて、`graph.bin`・`alt.bin` の逐次の decode がバイト列からの decode と同じ値になることを確かめる。

use std::fs;
use std::path::{Path, PathBuf};

use accident_route_backend::data::binfile::BinFile;
use accident_route_backend::data::release::{
    ACCIDENTS_FILE, ALT_FILE, GRAPH_FILE, METADATA_FILE, ROUTING_FILES, sha256_hex,
    try_routing_release_id,
};
use accident_route_backend::data::routing_files::{
    Deps, decode_alt, decode_alt_from_file, decode_graph, decode_graph_from_file, encode_alt,
    encode_graph, encode_topology,
};
use accident_route_backend::data::{self, LoadError};
use accident_route_backend::domain::routing::graph::{Graph, GraphParts};
use accident_route_backend::domain::routing::prep::attrs::GraphAttrs;
use accident_route_backend::domain::routing::prep::topo::{TopologyConfig, build_topology};
use accident_route_backend::domain::routing::search::alt::{Landmarks, ModeLandmarks};
use accident_route_backend::testing::TestRelease;
use serde_json::{Value, json};
use tempfile::TempDir;

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir(&from, &to);
        } else {
            fs::copy(&from, &to).unwrap();
        }
    }
}

/// コピーしたファイルの読み取り専用を戻す（書いて壊す試験のため）。
fn make_writable(path: &Path) {
    let metadata = fs::symlink_metadata(path).unwrap();
    if metadata.is_dir() {
        let mut permissions = metadata.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions).unwrap();
        for entry in fs::read_dir(path).unwrap() {
            make_writable(&entry.unwrap().path());
        }
    } else {
        let mut permissions = metadata.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions).unwrap();
    }
}

/// 人工releaseを別の一時ディレクトリにコピーする（読み取り専用は戻す）。
fn copied_release() -> (TempDir, PathBuf) {
    let (source, _) = TestRelease::routing();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("release");
    copy_dir(source.path(), &dir);
    make_writable(&dir);
    (root, dir)
}

fn expect_invalid(dir: &Path) -> String {
    match data::load_engine(dir) {
        Err(LoadError::Invalid(message)) => message,
        Err(other) => panic!("Invalid以外の失敗: {other}"),
        Ok(_) => panic!("読めてはいけないreleaseを受け付けた"),
    }
}

fn read_metadata(dir: &Path) -> Value {
    serde_json::from_slice(&fs::read(dir.join(METADATA_FILE)).unwrap()).unwrap()
}

fn write_metadata(dir: &Path, metadata: &Value) {
    let mut bytes = serde_json::to_vec_pretty(metadata).unwrap();
    bytes.push(b'\n');
    fs::write(dir.join(METADATA_FILE), bytes).unwrap();
}

/// metadataの `routing.files[].sha256` から `release_id` を計算し直す。
fn refresh_release_id(metadata: &mut Value) {
    let dataset_id = metadata["dataset_id"].as_str().unwrap().to_owned();
    let holiday = metadata["holiday_table"]["sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut pairs: Vec<(String, String)> = Vec::new();
    for file in ROUTING_FILES {
        let entry = metadata["routing"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["file"] == json!(file))
            .unwrap();
        pairs.push((
            file.to_owned(),
            entry["sha256"].as_str().unwrap().to_owned(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(file, hash)| (file.as_str(), hash.as_str()))
        .collect();
    let release_id = try_routing_release_id(&dataset_id, &holiday, &borrowed).unwrap();
    metadata["release_id"] = json!(release_id);
}

#[test]
fn missing_bin_is_invalid() {
    let (_root, dir) = copied_release();
    fs::remove_file(dir.join(ALT_FILE)).unwrap();
    let message = expect_invalid(&dir);
    assert!(message.contains(ALT_FILE), "{message}");
}

#[test]
fn changed_bin_hash_is_invalid() {
    let (_root, dir) = copied_release();
    let path = dir.join(ALT_FILE);
    let mut bytes = fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(&path, bytes).unwrap();
    let message = expect_invalid(&dir);
    assert!(message.contains(ALT_FILE), "{message}");
    assert!(message.contains("SHA-256"), "{message}");
}

#[test]
fn wrong_release_id_is_invalid() {
    let (_root, dir) = copied_release();
    let mut metadata = read_metadata(&dir);
    metadata["release_id"] = json!("0".repeat(64));
    write_metadata(&dir, &metadata);
    let message = expect_invalid(&dir);
    assert!(message.contains("release_id"), "{message}");
}

#[test]
fn wrong_section_count_is_invalid() {
    let (_root, dir) = copied_release();
    let mut metadata = read_metadata(&dir);
    let files = metadata["routing"]["files"].as_array_mut().unwrap();
    let graph = files
        .iter_mut()
        .find(|entry| entry["file"] == json!(GRAPH_FILE))
        .unwrap();
    let count = graph["sections"]["EDGE"].as_u64().unwrap();
    graph["sections"]["EDGE"] = json!(count + 1);
    write_metadata(&dir, &metadata);
    let message = expect_invalid(&dir);
    assert!(message.contains(GRAPH_FILE), "{message}");
    assert!(message.contains("節"), "{message}");
}

fn attrs_for(graph: &Graph) -> GraphAttrs {
    let edges = graph.edge_count();
    let nodes = graph.node_count();
    GraphAttrs {
        params: json!({ "edges": edges, "nodes": nodes }),
        way_id: (0..edges as i64).collect(),
        node_id: (0..nodes as i64).collect(),
        ref_idx: vec![0; edges],
        name_idx: vec![0; edges],
        strtab: vec![0],
        lanes: vec![0; edges],
        width_dm: vec![0; edges],
        layer: vec![0; edges],
    }
}

/// 座標だけをずらした別のグラフ（ノード数・辺数・接続は同じ）。
fn shifted_graph(graph: &Graph) -> Graph {
    let mut parts: GraphParts = graph.parts().clone();
    for coord in &mut parts.node_ll {
        coord.lat_e7 += 100_000;
    }
    Graph::from_parts(parts).unwrap()
}

#[test]
fn deps_mismatch_is_invalid() {
    let (_root, dir) = copied_release();
    let graph_bytes = fs::read(dir.join(GRAPH_FILE)).unwrap();
    let graph = decode_graph(&graph_bytes).unwrap().value;
    let other = shifted_graph(&graph);
    let other_bytes = encode_graph(&other);
    assert_ne!(sha256_hex(&graph_bytes), sha256_hex(&other_bytes));

    // 別のグラフに対して作ったtopology.binに差し替える。DEPSのgraph.binだけが実物と違う。
    let attrs = attrs_for(&other);
    let topology = build_topology(
        other.parts(),
        &attrs,
        &TopologyConfig {
            scc_min_nodes: 1,
            ..TopologyConfig::default()
        },
    )
    .unwrap();
    let deps = Deps::from([(GRAPH_FILE.to_owned(), sha256_hex(&other_bytes))]);
    let bytes = encode_topology(&topology, &deps);
    let topology_path = dir.join("topology.bin");
    fs::write(&topology_path, &bytes).unwrap();

    let parsed = BinFile::parse(&bytes).unwrap();
    let mut metadata = read_metadata(&dir);
    let files = metadata["routing"]["files"].as_array_mut().unwrap();
    let entry = files
        .iter_mut()
        .find(|entry| entry["file"] == json!("topology.bin"))
        .unwrap();
    entry["sha256"] = json!(sha256_hex(&bytes));
    entry["bytes"] = json!(bytes.len());
    entry["sections"] = json!(parsed.section_counts());
    refresh_release_id(&mut metadata);
    write_metadata(&dir, &metadata);

    let message = expect_invalid(&dir);
    assert!(message.contains("topology.bin"), "{message}");
    assert!(message.contains("DEPS"), "{message}");
    assert!(message.contains("graph.bin"), "{message}");
}

#[test]
fn sequential_graph_decode_matches_the_byte_decoder() {
    let (release, _) = TestRelease::routing();
    let path = release.path().join(GRAPH_FILE);
    let bytes = fs::read(&path).unwrap();
    let from_bytes = decode_graph(&bytes).unwrap();
    let from_file = decode_graph_from_file(&path).unwrap();
    assert_eq!(from_file.value.parts(), from_bytes.value.parts());
    assert_eq!(from_file.deps, from_bytes.deps);
}

#[test]
fn sequential_alt_decode_matches_the_byte_decoder() {
    let (release, _) = TestRelease::routing();
    let graph_bytes = fs::read(release.path().join(GRAPH_FILE)).unwrap();
    let graph = decode_graph(&graph_bytes).unwrap().value;
    let nodes = graph.node_count();
    let table = ModeLandmarks {
        nodes: vec![0],
        from: vec![0; nodes],
        to: Some(vec![0; nodes]),
    };
    let mut tables: [Option<ModeLandmarks>; 3] = [None, None, None];
    tables[2] = Some(table);
    let landmarks = Landmarks::from_tables(40, tables);
    let deps = Deps::from([(GRAPH_FILE.to_owned(), sha256_hex(&graph_bytes))]);
    let bytes = encode_alt(&landmarks, &deps);

    let root = tempfile::tempdir().unwrap();
    let path = root.path().join(ALT_FILE);
    fs::write(&path, &bytes).unwrap();
    let from_bytes = decode_alt(&bytes).unwrap();
    let from_file = decode_alt_from_file(&path).unwrap();
    assert_eq!(from_file.value, from_bytes.value);
    assert_eq!(from_file.deps, from_bytes.deps);
}

#[test]
fn the_original_release_still_loads() {
    let (release, _) = TestRelease::routing();
    let engine = data::load_engine(release.path()).unwrap();
    assert_eq!(engine.dataset().release_id, release.ids.release_id);
    assert_eq!(engine.dataset().dataset_id, release.ids.dataset_id);
    assert_eq!(engine.dataset().accidents, 3);
    assert_eq!(engine.dataset().data_years, vec![2025]);
    // 事故CSVのハッシュとmetadataが一致している。
    let gzip = fs::read(release.path().join(ACCIDENTS_FILE)).unwrap();
    assert_eq!(engine.dataset().dataset_id, sha256_hex(&gzip));
}
