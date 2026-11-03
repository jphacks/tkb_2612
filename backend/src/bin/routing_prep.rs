//! 前処理コマンド：自前経路探索のreleaseを段階ごとに作る（担当：②、形式書2章・18章）。
//!
//! ```sh
//! cargo run --release --manifest-path backend/Cargo.toml --bin routing_prep -- <サブコマンド> [--名前 値]...
//! ```
//!
//! サブコマンドとファイルの対応：
//!
//! | サブコマンド | 段階 | 作るファイル |
//! |---|---|---|
//! | `osm-graph` | P1 | `graph.bin` |
//! | `restrictions` | P2 | `restrictions.bin` |
//! | `topology` | P3 | `topology.bin` |
//! | `accident-attrs` | P4 | `accident_attrs.bin` |
//! | `accident-snap` | P5 | `assignment.bin` |
//! | `risk-fit` | P6 | `risk.bin` |
//! | `od-index` | P7 | `odindex.bin` |
//! | `alt` | P8 | `alt.bin` |
//! | `package` | P9 | `metadata.json` とreleaseディレクトリ |
//!
//! 実装済み：`osm-graph`（P1）・`restrictions`（P2）・`topology`（P3）・`accident-attrs`（P4）・
//! `accident-snap`（P5）・`od-index`（P7）。残りは骨組みのまま（`todo!()`）。
//!
//! | サブコマンド | 引数 |
//! |---|---|
//! | `osm-graph` | `--pbf <PBF> --output-dir <DIR> [--threads N]`：`graph.bin`・`graph_attrs.bin`・`graph.audit.json` |
//! | `restrictions` | `--pbf <PBF> --graph <graph.bin> --attrs <graph_attrs.bin> --output <restrictions.bin>`：監査は `<output>` の拡張子を `audit.json` にしたもの |
//! | `topology` | `--graph <graph.bin> --attrs <graph_attrs.bin> --output <topology.bin> [--scc-min-nodes N] [--threads N]`（`--scc-min-nodes` の既定は200。小さなグラフのテスト用） |
//! | `accident-snap` | `--graph <graph.bin> --attrs <graph_attrs.bin> --topology <topology.bin> --accident-attrs <accident_attrs.bin> --accidents <accidents.csv.gz> --output <assignment.bin> [--sigma0 M] [--threads N] [--review-dir DIR --review-count N --review-seed N]`：監査は `<output>` の拡張子を `audit.json` にしたもの。`--review-dir` を付けると人手判定用の `review_sample.json` を書く |
//! | `od-index` | `--graph <graph.bin> --output <odindex.bin> [--threads N]` |

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use accident_route_backend::data::osm;
use accident_route_backend::data::release::{GRAPH_FILE, sha256_hex};
use accident_route_backend::data::routing_files::{
    Deps, decode_graph_attrs, decode_graph_parts, encode_graph_attrs, encode_graph_parts,
    encode_odindex, encode_restrictions, encode_topology,
};
use accident_route_backend::domain::routing::prep::odindex::build_odindex;
use accident_route_backend::domain::routing::prep::restrict;
use accident_route_backend::domain::routing::prep::topo::{TopologyConfig, build_topology};
use serde_json::{Value, json};

const USAGE: &str = "使い方: routing_prep <osm-graph|restrictions|topology|accident-attrs|accident-snap|risk-fit|od-index|alt|package> [--名前 値]...";

/// `--名前 値` の組。名前は先頭の `--` を除いて持つ。
struct Options(BTreeMap<String, String>);

impl Options {
    fn require(&self, name: &str) -> Result<&str, String> {
        self.0
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| format!("--{name} が必要です"))
    }

    fn path(&self, name: &str) -> Result<PathBuf, String> {
        self.require(name).map(PathBuf::from)
    }
}

/// 進捗の表示。
fn log(message: &str) {
    eprintln!("{message}");
}

/// `/proc/self/status` の VmRSS・VmHWM（MB）。読めなければ空。
fn memory_summary() -> String {
    let Ok(status) = fs::read_to_string("/proc/self/status") else {
        return String::new();
    };
    let field = |name: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|kb| kb.parse::<f64>().ok())
            .map_or_else(|| "?".to_owned(), |kb| format!("{:.0}MB", kb / 1024.0))
    };
    format!("VmRSS={} VmHWM={}", field("VmRSS:"), field("VmHWM:"))
}

/// 並列処理のスレッド数を決める（既定16。他の作業と同時に動かすことがあるため、全コアは使わない）。
fn setup_threads(options: &Options) -> Result<(), String> {
    let threads = match options.0.get("threads") {
        Some(v) => v
            .parse::<usize>()
            .ok()
            .filter(|&n| n >= 1)
            .ok_or_else(|| "--threads は1以上の整数".to_owned())?,
        None => std::thread::available_parallelism().map_or(4, |n| n.get().min(16)),
    };
    // すでに設定されていれば（テストなど）そのまま使う
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build_global();
    log(&format!("スレッド数: {threads}"));
    Ok(())
}

fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("{} を読めません: {e}", path.display()))
}

/// 一時ファイルに書いてから置き換える。
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).map_err(|e| format!("{} に書けません: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("{} に置けません: {e}", path.display()))
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    write_file(path, &bytes)
}

/// `<output>` の拡張子を `audit.json` にした監査ファイルのパス（`restrictions.bin` → `restrictions.audit.json`）。
fn audit_path(output: &Path) -> PathBuf {
    output.with_extension("audit.json")
}

fn deps_of(graph_bytes: &[u8]) -> Deps {
    Deps::from([(GRAPH_FILE.to_owned(), sha256_hex(graph_bytes))])
}

fn parse_options(args: impl Iterator<Item = std::ffi::OsString>) -> Result<Options, String> {
    let mut options = BTreeMap::new();
    let mut args = args;
    while let Some(flag) = args.next() {
        let flag = flag
            .to_str()
            .and_then(|f| f.strip_prefix("--"))
            .ok_or_else(|| format!("不明な引数です: {}", flag.to_string_lossy()))?
            .to_owned();
        let value = args
            .next()
            .ok_or_else(|| format!("--{flag} の値がありません"))?;
        let value = value.into_string().map_err(|v| {
            format!(
                "--{flag} の値が文字列ではありません: {}",
                v.to_string_lossy()
            )
        })?;
        if options.insert(flag.clone(), value).is_some() {
            return Err(format!("--{flag} が重複しています"));
        }
    }
    Ok(Options(options))
}

/// P1：OSM PBFから `graph.bin` を作る（形式書5〜8章）。中間生成物 `graph_attrs.bin` も書く。
fn osm_graph(options: &Options) -> Result<(), String> {
    setup_threads(options)?;
    let pbf = options.path("pbf")?;
    let out_dir = options.path("output-dir")?;
    let started = Instant::now();
    log("PBFのヘッダーとSHA-256を読む");
    let info = osm::pbf_info(&pbf).map_err(|e| e.to_string())?;
    log(&format!(
        "  {} {}バイト sha256={} timestamp={:?}",
        info.file_name, info.bytes, info.sha256, info.replication_timestamp
    ));

    let result = osm::osm_to_graph(&pbf, &info, &log).map_err(|e| e.to_string())?;
    let built = result.built;

    log("graph.bin と graph_attrs.bin を書く");
    let graph_bytes = encode_graph_parts(&built.parts);
    let graph_hash = sha256_hex(&graph_bytes);
    write_file(&out_dir.join(GRAPH_FILE), &graph_bytes)?;
    let attrs_bytes = encode_graph_attrs(&built.attrs, &deps_of(&graph_bytes));
    write_file(&out_dir.join("graph_attrs.bin"), &attrs_bytes)?;

    let audit = json!({
        "command": "osm-graph",
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "memory": memory_summary(),
        "graph": { "file": GRAPH_FILE, "bytes": graph_bytes.len(), "sha256": graph_hash },
        "graph_attrs": { "file": "graph_attrs.bin", "bytes": attrs_bytes.len() },
        "params": built.parts.params,
        "audit": built.audit,
        "restriction_relations_read": result.relations.len(),
    });
    write_json(&out_dir.join("graph.audit.json"), &audit)?;
    log(&format!(
        "完了 {:.1}秒 {} graph.bin {}バイト",
        started.elapsed().as_secs_f64(),
        memory_summary(),
        graph_bytes.len()
    ));
    Ok(())
}

/// P2：turn restrictionを `restrictions.bin` にする（形式書9章）。
fn restrictions(options: &Options) -> Result<(), String> {
    setup_threads(options)?;
    let pbf = options.path("pbf")?;
    let graph_path = options.path("graph")?;
    let attrs_path = options.path("attrs")?;
    let output = options.path("output")?;
    let started = Instant::now();

    let graph_bytes = read_file(&graph_path)?;
    let parts = decode_graph_parts(&graph_bytes)
        .map_err(|e| e.to_string())?
        .value;
    let attrs = decode_graph_attrs(&read_file(&attrs_path)?)
        .map_err(|e| e.to_string())?
        .value;
    log(&format!(
        "graph.bin: ノード{} 辺{}",
        parts.node_ll.len(),
        parts.edges.len()
    ));
    log("右左折禁止のrelationを読む");
    let relations = osm::read_restriction_relations(&pbf).map_err(|e| e.to_string())?;
    log(&format!("  relation {}件", relations.len()));
    let resolved = restrict::resolve(&parts, &attrs, &relations);
    let bytes = encode_restrictions(&resolved.restrictions, &deps_of(&graph_bytes));
    write_file(&output, &bytes)?;
    let audit = json!({
        "command": "restrictions",
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "memory": memory_summary(),
        "bytes": bytes.len(),
        "sha256": sha256_hex(&bytes),
        "restrictions": resolved.audit,
    });
    write_json(&audit_path(&output), &audit)?;
    log(&format!(
        "完了 {:.1}秒 {} 禁止{}件",
        started.elapsed().as_secs_f64(),
        memory_summary(),
        resolved.restrictions.items.len()
    ));
    Ok(())
}

/// P3：`topology.bin` を作る（形式書10章）。
fn topology(options: &Options) -> Result<(), String> {
    setup_threads(options)?;
    let graph_path = options.path("graph")?;
    let attrs_path = options.path("attrs")?;
    let output = options.path("output")?;
    let started = Instant::now();

    let graph_bytes = read_file(&graph_path)?;
    let parts = decode_graph_parts(&graph_bytes)
        .map_err(|e| e.to_string())?
        .value;
    let attrs = decode_graph_attrs(&read_file(&attrs_path)?)
        .map_err(|e| e.to_string())?
        .value;
    log(&format!(
        "graph.bin: ノード{} 辺{}",
        parts.node_ll.len(),
        parts.edges.len()
    ));
    let mut config = TopologyConfig::default();
    if let Some(v) = options.0.get("scc-min-nodes") {
        config.scc_min_nodes = v
            .parse::<u32>()
            .map_err(|_| "--scc-min-nodes は整数".to_owned())?;
    }
    let topology = build_topology(&parts, &attrs, &config)?;
    let bytes = encode_topology(&topology, &deps_of(&graph_bytes));
    write_file(&output, &bytes)?;
    let audit = json!({
        "command": "topology",
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "memory": memory_summary(),
        "bytes": bytes.len(),
        "sha256": sha256_hex(&bytes),
        "topology": topology.params,
    });
    write_json(&audit_path(&output), &audit)?;
    log(&format!(
        "完了 {:.1}秒 {} クラスタ{} 双子{} 歩道の親{}",
        started.elapsed().as_secs_f64(),
        memory_summary(),
        topology.clusters.len(),
        topology.twins.len(),
        topology.sidewalk_parents.len()
    ));
    Ok(())
}

/// P4：事故の属性 `accident_attrs.bin` を作る（形式書12章、作業書M2a）。
///
/// ```sh
/// routing_prep accident-attrs --accident-dir accident --base-release <schema 2のreleaseのディレクトリ> \
///     --output <accident_attrs.binの書き出し先> [--audit <audit.jsonの書き出し先>]
/// ```
///
/// 入力は、既存の `prepare_release` が作ったschema 2のrelease（`accidents.csv.gz`・`syukujitsu.csv`・
/// `metadata.json`）と事故原本。原本を `prepare_release` と同じ保持判定で走査して事故CSVを作り直し、
/// releaseの `accidents.csv.gz` と一致することを確かめてから、保持した行と同じ順番（＝事故ID）で
/// 属性を書く。`DEPS` に `accidents.csv.gz` のSHA-256を入れる。コード表は常に実ファイルで照合する。
/// 一致しない（事故IDを保証できない）場合は、何も書かずに失敗する。
fn accident_attrs(options: &Options) -> Result<(), String> {
    use accident_route_backend::data::preprocess::attrs::{AttrCodebooks, build_accident_attrs};
    use accident_route_backend::data::routing_files::encode_accident_attrs;

    const USAGE: &str = "使い方: routing_prep accident-attrs --accident-dir <原本> --base-release <schema 2のrelease> --output <accident_attrs.bin> [--audit <audit.json>]";
    const KNOWN: [&str; 4] = ["accident-dir", "base-release", "output", "audit"];
    if let Some(unknown) = options.0.keys().find(|k| !KNOWN.contains(&k.as_str())) {
        return Err(format!("不明な引数です: --{unknown}\n{USAGE}"));
    }
    let required = |name: &str| {
        options
            .0
            .get(name)
            .map(std::path::PathBuf::from)
            .ok_or_else(|| format!("--{name} が必要です\n{USAGE}"))
    };
    let accident_dir = required("accident-dir")?;
    let base_release = required("base-release")?;
    let output = required("output")?;
    let audit_path = options.0.get("audit").map(std::path::PathBuf::from);

    let built = build_accident_attrs(&accident_dir, &base_release, &AttrCodebooks)
        .map_err(|e| e.to_string())?;
    let bytes = encode_accident_attrs(&built.table, &built.deps);

    // 途中の失敗で壊れたファイルを残さないよう、同じ場所の一時ファイルに書いてから置き換える
    let write = |path: &std::path::Path, content: &[u8]| -> Result<(), String> {
        let io = |e: std::io::Error| format!("{} に書けません: {e}", path.display());
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let mut temp = path.as_os_str().to_owned();
        temp.push(".tmp");
        let temp = std::path::PathBuf::from(temp);
        std::fs::write(&temp, content).map_err(io)?;
        std::fs::rename(&temp, path).map_err(io)
    };
    write(&output, &bytes)?;
    if let Some(path) = &audit_path {
        let mut json = serde_json::to_vec_pretty(&built.audit).expect("JSONに書ける値");
        json.push(b'\n');
        write(path, &json)?;
    }

    let totals = &built.audit["totals"];
    let summary = serde_json::json!({
        "output": output.display().to_string(),
        "audit": audit_path.as_ref().map(|p| p.display().to_string()),
        "bytes": bytes.len(),
        "rows": built.table.attrs.len(),
        "dataset_id": built.dataset_id,
        "gzip_identical": built.gzip_identical,
        "missing": totals["missing"],
        "unknown": totals["unknown"],
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&summary).expect("JSONに書ける値")
    );
    Ok(())
}

/// P5：事故を道路に割り当てて `assignment.bin` を作る（形式書13章、作業書M2b）。
///
/// 入力のファイルの `DEPS` が、渡した実ファイルのハッシュと一致しなければ失敗する。書いたあとに
/// 読み直して、同じ割り当てになること、`verify_assignment`（全件の検査）に通ることを確かめる。
fn accident_snap(options: &Options) -> Result<(), String> {
    use accident_route_backend::data::load::parse_accidents;
    use accident_route_backend::data::release::{
        ACCIDENT_ATTRS_FILE, ACCIDENTS_FILE, TOPOLOGY_FILE,
    };
    use accident_route_backend::data::routing_files::{
        decode_accident_attrs, decode_assignment, decode_topology, encode_assignment,
    };
    use accident_route_backend::domain::routing::prep::assign::{
        AssignParams, RoadIndex, SnapInput, snap_accidents,
    };
    use accident_route_backend::domain::routing::prep::assign_audit::{
        summarize, verify_assignment,
    };
    use accident_route_backend::domain::routing::prep::assign_review::{
        AccidentInfo, pick_review, review_record,
    };

    const USAGE: &str = "使い方: routing_prep accident-snap --graph <graph.bin> --attrs <graph_attrs.bin> --topology <topology.bin> --accident-attrs <accident_attrs.bin> --accidents <accidents.csv.gz> --output <assignment.bin> [--sigma0 M] [--threads N] [--review-dir DIR --review-count N --review-seed N]";
    const KNOWN: [&str; 12] = [
        "graph",
        "attrs",
        "topology",
        "accident-attrs",
        "accidents",
        "output",
        "sigma0",
        "threads",
        "review-dir",
        "review-count",
        "review-seed",
        "audit",
    ];
    if let Some(unknown) = options.0.keys().find(|k| !KNOWN.contains(&k.as_str())) {
        return Err(format!("不明な引数です: --{unknown}\n{USAGE}"));
    }
    setup_threads(options)?;
    let started = Instant::now();
    let graph_path = options.path("graph").map_err(|e| format!("{e}\n{USAGE}"))?;
    let attrs_path = options.path("attrs").map_err(|e| format!("{e}\n{USAGE}"))?;
    let topology_path = options
        .path("topology")
        .map_err(|e| format!("{e}\n{USAGE}"))?;
    let accident_attrs_path = options
        .path("accident-attrs")
        .map_err(|e| format!("{e}\n{USAGE}"))?;
    let accidents_path = options
        .path("accidents")
        .map_err(|e| format!("{e}\n{USAGE}"))?;
    let output = options
        .path("output")
        .map_err(|e| format!("{e}\n{USAGE}"))?;
    let mut params = AssignParams::default();
    if let Some(v) = options.0.get("sigma0") {
        params.sigma0_m = v
            .parse::<f64>()
            .ok()
            .filter(|x| x.is_finite() && *x > 0.0)
            .ok_or_else(|| "--sigma0 は正の数".to_owned())?;
    }
    let parse_usize = |name: &str, default: usize| -> Result<usize, String> {
        match options.0.get(name) {
            Some(v) => v.parse::<usize>().map_err(|_| format!("--{name} は整数")),
            None => Ok(default),
        }
    };
    let review_dir = options.0.get("review-dir").map(PathBuf::from);
    let review_count = parse_usize("review-count", 200)?;
    let review_seed = parse_usize("review-seed", 1)? as u64;

    log("入力を読む");
    let graph_bytes = read_file(&graph_path)?;
    let graph_hash = sha256_hex(&graph_bytes);
    let parts = decode_graph_parts(&graph_bytes)
        .map_err(|e| e.to_string())?
        .value;
    drop(graph_bytes);
    let attrs_bytes = read_file(&attrs_path)?;
    let graph_attrs = decode_graph_attrs(&attrs_bytes).map_err(|e| e.to_string())?;
    drop(attrs_bytes);
    let topology_bytes = read_file(&topology_path)?;
    let topology_hash = sha256_hex(&topology_bytes);
    let topology = decode_topology(&topology_bytes).map_err(|e| e.to_string())?;
    drop(topology_bytes);
    let accident_attrs_bytes = read_file(&accident_attrs_path)?;
    let accident_attrs_hash = sha256_hex(&accident_attrs_bytes);
    let accident_attrs = decode_accident_attrs(&accident_attrs_bytes).map_err(|e| e.to_string())?;
    drop(accident_attrs_bytes);
    let accidents_bytes = read_file(&accidents_path)?;
    let accidents_hash = sha256_hex(&accidents_bytes);
    let columns = parse_accidents(&accidents_bytes).map_err(|e| e.to_string())?;
    drop(accidents_bytes);
    log(&format!(
        "  ノード{} 辺{} クラスタ{} 事故{} {}",
        parts.node_ll.len(),
        parts.edges.len(),
        topology.value.clusters.len(),
        columns.len(),
        memory_summary()
    ));

    // 入力どうしの版の一致
    let expect = |what: &str, deps: &Deps, key: &str, hash: &str| -> Result<(), String> {
        match deps.get(key) {
            Some(h) if h == hash => Ok(()),
            _ => Err(format!(
                "{what}のDEPSの{key}が、渡された実ファイルのSHA-256と一致しません（別の版を混ぜていないか確かめてください）"
            )),
        }
    };
    expect("topology.bin", &topology.deps, GRAPH_FILE, &graph_hash)?;
    expect(
        "graph_attrs.bin",
        &graph_attrs.deps,
        GRAPH_FILE,
        &graph_hash,
    )?;
    expect(
        "accident_attrs.bin",
        &accident_attrs.deps,
        ACCIDENTS_FILE,
        &accidents_hash,
    )?;
    if accident_attrs.value.attrs.len() != columns.len() {
        return Err(format!(
            "accident_attrs.binの件数{}がaccidents.csv.gzの件数{}と違います",
            accident_attrs.value.attrs.len(),
            columns.len()
        ));
    }

    let input = SnapInput {
        graph: &parts,
        graph_attrs: &graph_attrs.value,
        topology: &topology.value,
        lat: &columns.lat,
        lon: &columns.lon,
        accident_attrs: &accident_attrs.value.attrs,
    };
    let result = snap_accidents(&input, &params, &log)?;
    log(&format!("割り当て完了 {}", memory_summary()));

    let deps = Deps::from([
        (GRAPH_FILE.to_owned(), graph_hash.clone()),
        (TOPOLOGY_FILE.to_owned(), topology_hash.clone()),
        (ACCIDENT_ATTRS_FILE.to_owned(), accident_attrs_hash.clone()),
        (ACCIDENTS_FILE.to_owned(), accidents_hash.clone()),
    ]);
    let bytes = encode_assignment(&result.assignment, &deps);
    write_file(&output, &bytes)?;
    let output_hash = sha256_hex(&bytes);

    log("書いたファイルを読み直して検査する");
    let decoded = decode_assignment(&bytes).map_err(|e| e.to_string())?;
    let (a, b) = (&decoded.value, &result.assignment);
    for (what, same) in [
        ("PARAMS", a.params == b.params),
        ("ACCOFS", a.offsets == b.offsets),
        ("TARGET", a.targets == b.targets),
        ("UNASSGN", a.unassigned == b.unassigned),
        ("DEPS", decoded.deps == deps),
    ] {
        if !same {
            return Err(format!(
                "書いたassignment.binを読み直した結果の{what}が、書く前の割り当てと一致しません"
            ));
        }
    }
    let verified = verify_assignment(
        &decoded.value,
        &parts,
        &topology.value,
        &accident_attrs.value.attrs,
    )
    .map_err(|e| format!("割り当ての検査に失敗しました:\n{e}"))?;
    let summary = summarize(
        &decoded.value,
        &accident_attrs.value.attrs,
        &parts,
        &graph_attrs.value,
    );

    let mut review_info = Value::Null;
    if let Some(dir) = &review_dir {
        log("人手判定用の抽出");
        let picks = pick_review(&accident_attrs.value.attrs, review_count, review_seed);
        let index = RoadIndex::build(&parts, &graph_attrs.value, &topology.value, params.clone());
        let mut records = Vec::with_capacity(picks.len());
        for pick in &picks {
            let i = pick.accident as usize;
            let info = AccidentInfo {
                lat: columns.lat[i],
                lon: columns.lon[i],
                modes: columns.modes[i],
                period: columns.period[i],
                weather: columns.weather[i],
                deaths: columns.deaths[i],
                injuries: columns.injuries[i],
                source_year: columns.source_year[i],
                date: columns.date[i],
            };
            records.push(review_record(
                &index,
                &result.tables,
                &decoded.value,
                pick,
                &accident_attrs.value.attrs[i],
                &info,
            )?);
        }
        let sample = json!({
            "assignment_sha256": output_hash,
            "seed": review_seed,
            "count": records.len(),
            "sigma0_m": params.sigma0_m,
            "records": records,
        });
        write_json(&dir.join("review_sample.json"), &sample)?;
        review_info = json!({ "dir": dir.display().to_string(), "records": picks.len(), "seed": review_seed });
    }

    let audit_file = options
        .0
        .get("audit")
        .map_or_else(|| audit_path(&output), PathBuf::from);
    let file_info = |path: &Path, bytes: usize, hash: &str| json!({ "file": path.display().to_string(), "bytes": bytes, "sha256": hash });
    let audit = json!({
        "command": "accident-snap",
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "memory": memory_summary(),
        "output": file_info(&output, bytes.len(), &output_hash),
        "inputs": {
            "graph": file_info(&graph_path, 0, &graph_hash),
            "topology": file_info(&topology_path, 0, &topology_hash),
            "accident_attrs": file_info(&accident_attrs_path, 0, &accident_attrs_hash),
            "accidents": file_info(&accidents_path, 0, &accidents_hash),
        },
        "params": result.assignment.params,
        "verify": {
            "passed": true,
            "accidents": verified.accidents,
            "assigned": verified.assigned,
            "unassigned": verified.unassigned,
            "targets": verified.targets,
            "edge_targets": verified.edge_targets,
            "cluster_targets": verified.cluster_targets,
            "mirror_targets": verified.mirror_targets,
            "motorway_coded_assigned": verified.motorway_coded_assigned,
        },
        "summary": summary,
        "review": review_info,
    });
    write_json(&audit_file, &audit)?;
    log(&format!(
        "完了 {:.1}秒 {} assignment.bin {}バイト 割り当て{} 帰属不明{}",
        started.elapsed().as_secs_f64(),
        memory_summary(),
        bytes.len(),
        verified.assigned,
        verified.unassigned
    ));
    Ok(())
}

/// P6：リスクの係数を推定して `risk.bin` を作る（形式書14章）。
fn risk_fit(options: &Options) -> Result<(), String> {
    let _ = options;
    todo!("P6 risk-fit")
}

/// P7：吸着の格子 `odindex.bin` を作る（形式書11章）。
fn od_index(options: &Options) -> Result<(), String> {
    setup_threads(options)?;
    let graph_path = options.path("graph")?;
    let output = options.path("output")?;
    let started = Instant::now();

    let graph_bytes = read_file(&graph_path)?;
    let parts = decode_graph_parts(&graph_bytes)
        .map_err(|e| e.to_string())?
        .value;
    log(&format!(
        "graph.bin: ノード{} 辺{}",
        parts.node_ll.len(),
        parts.edges.len()
    ));
    let index = build_odindex(&parts);
    let bytes = encode_odindex(&index, &deps_of(&graph_bytes));
    write_file(&output, &bytes)?;
    let audit = json!({
        "command": "od-index",
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "memory": memory_summary(),
        "bytes": bytes.len(),
        "sha256": sha256_hex(&bytes),
        "odindex": index.raw_params,
    });
    write_json(&audit_path(&output), &audit)?;
    log(&format!(
        "完了 {:.1}秒 {} セル{} 登録{}",
        started.elapsed().as_secs_f64(),
        memory_summary(),
        index.cell_keys.len(),
        index.cell_edges.len()
    ));
    Ok(())
}

/// P8：ALTの距離表 `alt.bin` を作る（形式書15章）。
fn alt(options: &Options) -> Result<(), String> {
    let _ = options;
    todo!("P8 alt")
}

/// P9：schema 2のreleaseに8つの `.bin` を加え、`metadata.json`（schema 3）を作る（形式書2章・4章）。
fn package(options: &Options) -> Result<(), String> {
    let _ = options;
    todo!("P9 package")
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(command) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let run: fn(&Options) -> Result<(), String> = match command.to_str() {
        Some("osm-graph") => osm_graph,
        Some("restrictions") => restrictions,
        Some("topology") => topology,
        Some("accident-attrs") => accident_attrs,
        Some("accident-snap") => accident_snap,
        Some("risk-fit") => risk_fit,
        Some("od-index") => od_index,
        Some("alt") => alt,
        Some("package") => package,
        _ => {
            eprintln!(
                "不明なサブコマンドです: {}\n{USAGE}",
                command.to_string_lossy()
            );
            return ExitCode::from(2);
        }
    };
    let options = match parse_options(args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("前処理に失敗しました: {message}");
            ExitCode::FAILURE
        }
    }
}
