//! schema 2のreleaseと8つの `.bin` から、schema 3のreleaseを組み立てる（形式書2章・4章）。
//!
//! [`package_routing_release`] は、`<output>/staging/<release_id>-<pid>/` に11ファイルを書いてから
//! `<output>/releases/<release_id>/` へ rename し、読み取り専用にする。`current` は変えない。
//! 検査とmetadataの組み立てだけを行う [`build_routing_metadata`] は書き出さない。

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::contract::Mode;
use crate::data::binfile::BinFile;
use crate::data::load::parse_accidents;
use crate::data::release::{
    ACCIDENTS_FILE, HOLIDAYS_FILE, METADATA_FILE, PROCESSOR_NAME_ROUTING, ROUTING_FILES,
    SCHEMA_VERSION, SCHEMA_VERSION_ROUTING, release_id, routing_file_spec, routing_release_id,
    sha256_hex,
};
use crate::data::routing_files::{
    decode_accident_attrs, decode_alt, decode_assignment, decode_graph, decode_odindex,
    decode_restrictions, decode_risk, decode_topology, required_deps,
};
use crate::domain::routing::graph::Graph;
use crate::domain::routing::risk::RiskParams;

/// package の入力。
#[derive(Debug, Clone)]
pub struct PackageInputs {
    /// schema 2のreleaseのディレクトリ（`accidents.csv.gz`・`syukujitsu.csv`・`metadata.json`）。
    pub base_release: PathBuf,
    /// 8つの `.bin` のパス。`ROUTING_FILES` の順（graph, restrictions, topology, odindex,
    /// accident_attrs, assignment, risk, alt）。
    pub routing_files: [PathBuf; 8],
    /// metadata の `generated_at`（RFC 3339）。テストでは固定値を渡す。
    pub generated_at: String,
}

/// 作ったreleaseの要約。
#[derive(Debug, Clone)]
pub struct PackageSummary {
    pub release_dir: PathBuf,
    pub release_id: String,
    pub dataset_id: String,
    pub metadata: Value,
}

/// schema 2のreleaseから写すキー（形式書4.1節）。base にないキーは足さない。
const BASE_KEYS: [&str; 11] = [
    "dataset_id",
    "rows",
    "holiday_table",
    "data_years",
    "files",
    "totals",
    "codebooks_xlsx",
    "codebooks_pdf",
    "holiday_uncovered_rows",
    "columns",
    "gzip_bytes",
];

/// releaseに置く11ファイル。読み取り専用にする順もこの並び。
const RELEASE_FILES: [&str; 11] = [
    ACCIDENTS_FILE,
    HOLIDAYS_FILE,
    METADATA_FILE,
    ROUTING_FILES[0],
    ROUTING_FILES[1],
    ROUTING_FILES[2],
    ROUTING_FILES[3],
    ROUTING_FILES[4],
    ROUTING_FILES[5],
    ROUTING_FILES[6],
    ROUTING_FILES[7],
];

fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("{} を読めません: {e}", path.display()))
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::write(path, bytes).map_err(|e| format!("{} に書けません: {e}", path.display()))
}

fn metadata_str<'a>(metadata: &'a Value, path: &[&str]) -> Result<&'a str, String> {
    let mut value = metadata;
    for key in path {
        value = value
            .get(key)
            .ok_or_else(|| format!("base releaseのmetadataに{}がありません", path.join(".")))?;
    }
    value.as_str().ok_or_else(|| {
        format!(
            "base releaseのmetadataの{}が文字列ではありません",
            path.join(".")
        )
    })
}

fn metadata_u64(metadata: &Value, path: &[&str]) -> Result<u64, String> {
    let mut value = metadata;
    for key in path {
        value = value
            .get(key)
            .ok_or_else(|| format!("base releaseのmetadataに{}がありません", path.join(".")))?;
    }
    value.as_u64().ok_or_else(|| {
        format!(
            "base releaseのmetadataの{}が非負の整数ではありません",
            path.join(".")
        )
    })
}

fn param_u64(params: &Value, key: &str) -> Result<u64, String> {
    params
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("PARAMSの{key}が非負の整数ではありません"))
}

fn param_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("PARAMSの{key}が文字列ではありません"))
}

/// 検査済みのschema 2のrelease。
struct BaseRelease {
    metadata: Value,
    dataset_id: String,
    holiday_sha256: String,
    release_id: String,
    rows: u64,
}

/// schema 2のreleaseを読み、形式書4.1節のとおりに検査する。
fn read_base_release(dir: &Path) -> Result<BaseRelease, String> {
    let metadata_path = dir.join(METADATA_FILE);
    let bytes = read_file(&metadata_path)?;
    let metadata: Value = serde_json::from_slice(&bytes)
        .map_err(|_| format!("{} がJSONではありません", metadata_path.display()))?;
    if !metadata.is_object() {
        return Err(format!(
            "{} がJSONオブジェクトではありません",
            metadata_path.display()
        ));
    }
    if metadata.get("schema_version").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
        return Err(format!(
            "{} のschema_versionが2ではありません",
            metadata_path.display()
        ));
    }

    let dataset_id = metadata_str(&metadata, &["dataset_id"])?.to_owned();
    let accidents_path = dir.join(ACCIDENTS_FILE);
    let accidents = read_file(&accidents_path)?;
    let actual_dataset_id = sha256_hex(&accidents);
    if actual_dataset_id != dataset_id {
        return Err(format!(
            "{} のSHA-256がmetadataのdataset_idと一致しません",
            accidents_path.display()
        ));
    }

    let holidays_path = dir.join(HOLIDAYS_FILE);
    let holidays = read_file(&holidays_path)?;
    let holiday_sha256 = sha256_hex(&holidays);
    let expected_holiday = metadata_str(&metadata, &["holiday_table", "sha256"])?;
    if holiday_sha256 != expected_holiday {
        return Err(format!(
            "{} のSHA-256がmetadataのholiday_table.sha256と一致しません",
            holidays_path.display()
        ));
    }

    let expected_release_id = release_id(&dataset_id, &holiday_sha256);
    if metadata_str(&metadata, &["release_id"])? != expected_release_id {
        return Err(
            "base releaseのrelease_idがdataset_idと祝日表のSHA-256から計算した値と一致しません"
                .to_owned(),
        );
    }

    let rows = metadata_u64(&metadata, &["rows"])?;
    let columns =
        parse_accidents(&accidents).map_err(|e| format!("{}: {e}", accidents_path.display()))?;
    if columns.len() as u64 != rows {
        return Err(format!(
            "{} の行数{}がmetadataのrows {}と一致しません",
            accidents_path.display(),
            columns.len(),
            rows
        ));
    }

    Ok(BaseRelease {
        metadata,
        dataset_id,
        holiday_sha256,
        release_id: expected_release_id,
        rows,
    })
}

/// `metadata.json` の `routing.files[]` に書く1ファイルの情報。
struct RoutingFileInfo {
    file: &'static str,
    sha256: String,
    bytes: u64,
    magic: String,
    format_version: u32,
    sections: BTreeMap<String, u64>,
}

impl RoutingFileInfo {
    fn to_json(&self) -> Value {
        json!({
            "file": self.file,
            "sha256": self.sha256,
            "bytes": self.bytes,
            "magic": self.magic,
            "format_version": self.format_version,
            "sections": self.sections,
        })
    }
}

/// `graph.bin` の `PARAMS` から写す値。
struct GraphSummary {
    nodes: u64,
    edges: u64,
    shape_points: u64,
    rules_version: String,
    osm: Value,
}

/// `risk.bin` の `PARAMS` から写す値。
struct RiskSummary {
    params: RiskParams,
    raw: Value,
}

/// 照合しなかった `DEPS` のキー（監査欄に残す）。
struct UnverifiedDep {
    file: &'static str,
    key: String,
    sha256: String,
}

impl UnverifiedDep {
    fn to_json(&self) -> Value {
        json!({
            "file": self.file,
            "key": self.key,
            "sha256": self.sha256,
        })
    }
}

/// 検査とmetadataの組み立ての結果。`package_routing_release` が書き出しにも使う。
struct PreparedRelease {
    release_id: String,
    dataset_id: String,
    holiday_sha256: String,
    metadata: Value,
    files: Vec<RoutingFileInfo>,
}

/// 8つの `.bin` を1ファイルずつ読み、形式・payload・DEPS・件数を検査する。
fn inspect_routing_files(
    inputs: &PackageInputs,
    base: &BaseRelease,
) -> Result<
    (
        Vec<RoutingFileInfo>,
        GraphSummary,
        RiskSummary,
        Vec<UnverifiedDep>,
    ),
    String,
> {
    let mut hashes: BTreeMap<String, String> = BTreeMap::new();
    hashes.insert(ACCIDENTS_FILE.to_owned(), base.dataset_id.clone());
    hashes.insert(HOLIDAYS_FILE.to_owned(), base.holiday_sha256.clone());

    let mut files = Vec::with_capacity(ROUTING_FILES.len());
    let mut unverified = Vec::new();
    // release内でも、まだ読んでいないファイルへのDEPSは、8ファイルを読み終えてから照合する。
    let mut deferred: Vec<(&'static str, String, String)> = Vec::new();
    let mut graph_summary: Option<GraphSummary> = None;
    let mut risk_summary: Option<RiskSummary> = None;
    let mut cluster_count: Option<u64> = None;
    // `restrictions.bin`・`topology.bin` の `validate` に使う。topology の検査が終わったら捨てる。
    let mut graph_for_checks: Option<Graph> = None;

    for (index, &file) in ROUTING_FILES.iter().enumerate() {
        let path = &inputs.routing_files[index];
        let bytes = read_file(path)?;
        let sha256 = sha256_hex(&bytes);
        let (magic, format_version) = routing_file_spec(file)
            .ok_or_else(|| format!("{file}はreleaseの8ファイルに含まれません"))?;

        let parsed = BinFile::parse(&bytes).map_err(|e| format!("{file}: {e}"))?;
        parsed
            .expect(magic, format_version)
            .map_err(|e| format!("{file}: {e}"))?;
        parsed
            .verify_payload_sha256()
            .map_err(|e| format!("{file}: {e}"))?;
        let deps = parsed.deps().map_err(|e| format!("{file}: {e}"))?;
        let sections = parsed.section_counts();

        let required = required_deps(file);
        for key in required {
            let actual = hashes.get(*key).ok_or_else(|| {
                format!("{file}のDEPSの{key}が、releaseに入れる実ファイルのSHA-256と照合できません")
            })?;
            match deps.get(*key) {
                Some(value) if value == actual => {}
                Some(_) => {
                    return Err(format!(
                        "{file}のDEPSの{key}が、releaseに入れる実ファイルのSHA-256と一致しません"
                    ));
                }
                None => return Err(format!("{file}のDEPSに{key}がありません")),
            }
        }
        for (key, value) in &deps {
            if required.contains(&key.as_str()) {
                continue;
            }
            match hashes.get(key) {
                Some(actual) if actual == value => {}
                Some(_) => {
                    return Err(format!(
                        "{file}のDEPSの{key}が、releaseに入れる実ファイルのSHA-256と一致しません"
                    ));
                }
                None if ROUTING_FILES.contains(&key.as_str())
                    || key == ACCIDENTS_FILE
                    || key == HOLIDAYS_FILE =>
                {
                    deferred.push((file, key.clone(), value.clone()));
                }
                // release外の中間生成物（graph_attrs.bin など）は照合せず、監査欄に残す。
                None => unverified.push(UnverifiedDep {
                    file,
                    key: key.clone(),
                    sha256: value.clone(),
                }),
            }
        }

        match index {
            0 => {
                let graph = decode_graph(&bytes)
                    .map_err(|e| format!("{file}: {e}"))?
                    .value;
                let params = graph.params();
                let nodes = param_u64(params, "nodes")?;
                let edges = param_u64(params, "edges")?;
                let shape_points = param_u64(params, "shape_points")?;
                if nodes != graph.node_count() as u64
                    || edges != graph.edge_count() as u64
                    || shape_points != graph.parts().shape.len() as u64
                {
                    return Err(format!("{file}のPARAMSの件数が節の件数と一致しません"));
                }
                let rules_version = param_str(params, "rules_version")?.to_owned();
                let osm = params.get("osm").cloned().unwrap_or(Value::Null);
                graph_summary = Some(GraphSummary {
                    nodes,
                    edges,
                    shape_points,
                    rules_version,
                    osm,
                });
                graph_for_checks = Some(graph);
            }
            1 => {
                let restrictions = decode_restrictions(&bytes)
                    .map_err(|e| format!("{file}: {e}"))?
                    .value;
                let graph = graph_for_checks.as_ref().ok_or_else(|| {
                    format!("{file}の検査に必要なgraph.binが先に読まれていません")
                })?;
                restrictions
                    .validate(graph)
                    .map_err(|e| format!("{file}: {e}"))?;
            }
            2 => {
                let topology = decode_topology(&bytes)
                    .map_err(|e| format!("{file}: {e}"))?
                    .value;
                let graph = graph_for_checks.as_ref().ok_or_else(|| {
                    format!("{file}の検査に必要なgraph.binが先に読まれていません")
                })?;
                topology
                    .validate(graph)
                    .map_err(|e| format!("{file}: {e}"))?;
                cluster_count = Some(topology.clusters.len() as u64);
                // graphは約1GBなので、topologyの検査が終わったら保持しない。
                graph_for_checks = None;
            }
            3 => {
                let index = decode_odindex(&bytes)
                    .map_err(|e| format!("{file}: {e}"))?
                    .value;
                let graph = graph_summary
                    .as_ref()
                    .ok_or_else(|| format!("{file}の検査に必要なgraph.binがありません"))?;
                if index
                    .cell_edges
                    .iter()
                    .any(|&edge| u64::from(edge) >= graph.edges)
                {
                    return Err(format!(
                        "{file}のCELLEDGにgraph.binの辺数の範囲外があります"
                    ));
                }
                if let Some(edges) = index.raw_params.get("edges").and_then(Value::as_u64)
                    && edges != graph.edges
                {
                    return Err(format!(
                        "{file}のPARAMSのedgesがgraph.binの辺数と一致しません"
                    ));
                }
            }
            4 => {
                let table = decode_accident_attrs(&bytes)
                    .map_err(|e| format!("{file}: {e}"))?
                    .value;
                if table.attrs.len() as u64 != base.rows {
                    return Err(format!(
                        "{file}の事故数{}がmetadataのrows {}と一致しません",
                        table.attrs.len(),
                        base.rows
                    ));
                }
            }
            5 => {
                let assignment = decode_assignment(&bytes)
                    .map_err(|e| format!("{file}: {e}"))?
                    .value;
                let graph = graph_summary
                    .as_ref()
                    .ok_or_else(|| format!("{file}の検査に必要なgraph.binがありません"))?;
                let clusters = cluster_count
                    .ok_or_else(|| format!("{file}の検査に必要なtopology.binがありません"))?;
                if assignment.accident_count() as u64 != base.rows {
                    return Err(format!(
                        "{file}の事故数{}がmetadataのrows {}と一致しません",
                        assignment.accident_count(),
                        base.rows
                    ));
                }
                assignment
                    .validate(graph.edges as usize, clusters as usize)
                    .map_err(|e| format!("{file}: {e}"))?;
            }
            6 => {
                let model = decode_risk(&bytes)
                    .map_err(|e| format!("{file}: {e}"))?
                    .value;
                let graph = graph_summary
                    .as_ref()
                    .ok_or_else(|| format!("{file}の検査に必要なgraph.binがありません"))?;
                let clusters = cluster_count
                    .ok_or_else(|| format!("{file}の検査に必要なtopology.binがありません"))?;
                if model.edge_k.len() as u64 != graph.edges {
                    return Err(format!(
                        "{file}のEDGEKの件数{}がgraph.binの辺数{}と一致しません",
                        model.edge_k.len(),
                        graph.edges
                    ));
                }
                if model.cluster_j.len() as u64 != clusters {
                    return Err(format!(
                        "{file}のCLUSJの件数{}がtopology.binのクラスタ数{clusters}と一致しません",
                        model.cluster_j.len()
                    ));
                }
                risk_summary = Some(RiskSummary {
                    params: model.params,
                    raw: model.raw_params,
                });
            }
            7 => {
                let landmarks = decode_alt(&bytes)
                    .map_err(|e| format!("{file}: {e}"))?
                    .value;
                let graph = graph_summary
                    .as_ref()
                    .ok_or_else(|| format!("{file}の検査に必要なgraph.binがありません"))?;
                for mode in Mode::ALL {
                    let Some(table) = landmarks.table(mode) else {
                        continue;
                    };
                    if table.count() > 0
                        && table.from.len() as u64 != graph.nodes * table.count() as u64
                    {
                        return Err(format!(
                            "{file}の{}の表の件数がgraph.binのノード数と一致しません",
                            mode.as_str()
                        ));
                    }
                    if let Some(to) = &table.to
                        && to.len() != table.from.len()
                    {
                        return Err(format!(
                            "{file}の{}のTOの件数がFRと一致しません",
                            mode.as_str()
                        ));
                    }
                    if table
                        .nodes
                        .iter()
                        .any(|&node| u64::from(node) >= graph.nodes)
                    {
                        return Err(format!(
                            "{file}の{}のランドマークがgraph.binのノード数の範囲外です",
                            mode.as_str()
                        ));
                    }
                }
            }
            _ => unreachable!("ROUTING_FILESは8要素"),
        }

        hashes.insert(file.to_owned(), sha256.clone());
        files.push(RoutingFileInfo {
            file,
            sha256,
            bytes: bytes.len() as u64,
            magic: String::from_utf8(magic.to_vec()).expect("マジックナンバーはASCII"),
            format_version,
            sections,
        });
    }

    for (file, key, value) in deferred {
        let actual = hashes.get(&key).ok_or_else(|| {
            format!("{file}のDEPSの{key}が、releaseに入れる実ファイルのSHA-256と照合できません")
        })?;
        if actual != &value {
            return Err(format!(
                "{file}のDEPSの{key}が、releaseに入れる実ファイルのSHA-256と一致しません"
            ));
        }
    }

    let graph_summary = graph_summary.ok_or_else(|| "graph.binを読みませんでした".to_owned())?;
    let risk_summary = risk_summary.ok_or_else(|| "risk.binを読みませんでした".to_owned())?;
    Ok((files, graph_summary, risk_summary, unverified))
}

/// `risk.bin` の `PARAMS` から `metadata.routing.risk` に写すキーだけを取り出す。
fn risk_metadata(summary: &RiskSummary) -> Result<Value, String> {
    let mut value = serde_json::to_value(&summary.params)
        .map_err(|e| format!("risk.binのPARAMSをJSONにできません: {e}"))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "risk.binのPARAMSがJSONオブジェクトではありません".to_owned())?;
    // 大きな監査値は写さない（形式書4.1節）。
    for key in ["fit", "pooling", "urban"] {
        object.remove(key);
    }
    for key in ["inherited_from", "validate_year"] {
        if let Some(value) = summary.raw.get(key) {
            object.insert(key.to_owned(), value.clone());
        }
    }
    Ok(value)
}

fn prepare(inputs: &PackageInputs) -> Result<PreparedRelease, String> {
    let base = read_base_release(&inputs.base_release)?;
    let (files, graph, risk, unverified) = inspect_routing_files(inputs, &base)?;

    let hashes: Vec<(&str, &str)> = files
        .iter()
        .map(|info| (info.file, info.sha256.as_str()))
        .collect();
    let schema3_release_id = routing_release_id(&base.dataset_id, &base.holiday_sha256, &hashes);

    let files_json: Vec<Value> = files.iter().map(RoutingFileInfo::to_json).collect();
    let unverified_json: Vec<Value> = unverified.iter().map(UnverifiedDep::to_json).collect();

    let base_release_json = json!({
        "release_id": base.release_id,
        "processor": base.metadata.get("processor").cloned().unwrap_or(Value::Null),
        "generated_at": base.metadata.get("generated_at").cloned().unwrap_or(Value::Null),
    });
    let holiday_table = base
        .metadata
        .get("holiday_table")
        .cloned()
        .unwrap_or(Value::Null);
    let accidents_input = json!({
        "release_id": base.release_id,
        "dataset_id": base.dataset_id,
    });

    let risk_json = risk_metadata(&risk)?;
    let routing = json!({
        "files": files_json,
        "rules_version": graph.rules_version,
        "inputs": {
            "osm": graph.osm,
            "accidents": accidents_input,
            "holiday_table": holiday_table,
        },
        "graph": {
            "nodes": graph.nodes,
            "edges": graph.edges,
            "shape_points": graph.shape_points,
        },
        "risk": risk_json,
        "audit": {
            "unverified_deps": unverified_json,
        },
    });

    let mut metadata = Map::new();
    for key in BASE_KEYS {
        if let Some(value) = base.metadata.get(key) {
            metadata.insert(key.to_owned(), value.clone());
        }
    }
    metadata.insert("schema_version".to_owned(), json!(SCHEMA_VERSION_ROUTING));
    metadata.insert("release_id".to_owned(), json!(schema3_release_id));
    metadata.insert(
        "processor".to_owned(),
        json!({ "name": PROCESSOR_NAME_ROUTING, "version": env!("CARGO_PKG_VERSION") }),
    );
    metadata.insert("generated_at".to_owned(), json!(inputs.generated_at));
    metadata.insert("base_release".to_owned(), base_release_json);
    metadata.insert("routing".to_owned(), routing);

    Ok(PreparedRelease {
        release_id: schema3_release_id,
        dataset_id: base.dataset_id,
        holiday_sha256: base.holiday_sha256,
        metadata: Value::Object(metadata),
        files,
    })
}

/// 検査とmetadataの組み立てだけ（書き出さない）。テストと [`package_routing_release`] が使う。
pub fn build_routing_metadata(inputs: &PackageInputs) -> Result<(String, Value), String> {
    let prepared = prepare(inputs)?;
    Ok((prepared.release_id, prepared.metadata))
}

fn copy_checked(source: &Path, target: &Path, expected_sha256: &str) -> Result<(), String> {
    let bytes = read_file(source)?;
    if sha256_hex(&bytes) != expected_sha256 {
        return Err(format!(
            "{} のSHA-256が検査した値と一致しません（検査後にファイルが変わりました）",
            source.display()
        ));
    }
    write_file(target, &bytes)
}

fn make_release_read_only(dir: &Path) -> Result<(), String> {
    for name in RELEASE_FILES {
        let path = dir.join(name);
        let mut permissions = fs::metadata(&path)
            .map_err(|e| format!("{} の権限を読めません: {e}", path.display()))?
            .permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions)
            .map_err(|e| format!("{} を読み取り専用にできません: {e}", path.display()))?;
    }
    Ok(())
}

fn write_prepared(
    prepared: &PreparedRelease,
    inputs: &PackageInputs,
    staging: &Path,
) -> Result<(), String> {
    copy_checked(
        &inputs.base_release.join(ACCIDENTS_FILE),
        &staging.join(ACCIDENTS_FILE),
        &prepared.dataset_id,
    )?;
    copy_checked(
        &inputs.base_release.join(HOLIDAYS_FILE),
        &staging.join(HOLIDAYS_FILE),
        &prepared.holiday_sha256,
    )?;
    for (index, info) in prepared.files.iter().enumerate() {
        copy_checked(
            &inputs.routing_files[index],
            &staging.join(info.file),
            &info.sha256,
        )?;
    }
    let mut metadata = serde_json::to_vec_pretty(&prepared.metadata)
        .map_err(|e| format!("metadataを書けません: {e}"))?;
    metadata.push(b'\n');
    let metadata_path = staging.join(METADATA_FILE);
    let mut file = fs::File::create(&metadata_path)
        .map_err(|e| format!("{} を作れません: {e}", metadata_path.display()))?;
    file.write_all(&metadata)
        .map_err(|e| format!("{} に書けません: {e}", metadata_path.display()))?;
    file.sync_all()
        .map_err(|e| format!("{} を同期できません: {e}", metadata_path.display()))?;
    Ok(())
}

/// 入力を検査し、`output_root/staging/<release_id>-<pid>/` に11ファイルを書いてから
/// `output_root/releases/<release_id>/` へ rename し、読み取り専用にする。`current` は触らない。
/// 同じ `release_id` のディレクトリが既にあれば Err（上書きしない）。
pub fn package_routing_release(
    inputs: &PackageInputs,
    output_root: &Path,
) -> Result<PackageSummary, String> {
    let prepared = prepare(inputs)?;
    let target = output_root.join("releases").join(&prepared.release_id);
    if target.exists() {
        return Err(format!(
            "{} は既にあります（releaseは不変なので上書きしません）",
            target.display()
        ));
    }

    let staging =
        output_root
            .join("staging")
            .join(format!("{}-{}", prepared.release_id, std::process::id()));
    // 前回の同じ名前の一時ディレクトリが残っていれば片付ける（releaseの識別子とpidで一意）。
    if staging.exists() {
        fs::remove_dir_all(&staging)
            .map_err(|e| format!("{} を片付けられません: {e}", staging.display()))?;
    }
    fs::create_dir_all(&staging).map_err(|e| format!("{} を作れません: {e}", staging.display()))?;

    let mut renamed = false;
    let result = (|| {
        write_prepared(&prepared, inputs, &staging)?;
        let parent = target
            .parent()
            .ok_or_else(|| format!("{} の親ディレクトリがありません", target.display()))?;
        fs::create_dir_all(parent)
            .map_err(|e| format!("{} を作れません: {e}", parent.display()))?;
        fs::rename(&staging, &target).map_err(|e| {
            format!(
                "{} を {} に移せません: {e}",
                staging.display(),
                target.display()
            )
        })?;
        renamed = true;
        make_release_read_only(&target)
    })();

    if let Err(message) = result {
        // renameの前ならstaging、後なら（この呼び出しが作った）targetを片付ける。
        let _ = fs::remove_dir_all(&staging);
        if renamed {
            let _ = fs::remove_dir_all(&target);
        }
        return Err(message);
    }

    Ok(PackageSummary {
        release_dir: target,
        release_id: prepared.release_id,
        dataset_id: prepared.dataset_id,
        metadata: prepared.metadata,
    })
}
