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
//! 実装済み：`osm-graph`（P1）・`restrictions`（P2）・`topology`（P3）・`od-index`（P7）。
//! 残りは骨組みのまま（`todo!()`）。
//!
//! | サブコマンド | 引数 |
//! |---|---|
//! | `osm-graph` | `--pbf <PBF> --output-dir <DIR> [--threads N]`：`graph.bin`・`graph_attrs.bin`・`graph.audit.json` |
//! | `restrictions` | `--pbf <PBF> --graph <graph.bin> --attrs <graph_attrs.bin> --output <restrictions.bin>`：監査は `<output>` の拡張子を `audit.json` にしたもの |
//! | `topology` | `--graph <graph.bin> --attrs <graph_attrs.bin> --output <topology.bin> [--threads N]` |
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
    let topology = build_topology(&parts, &attrs, &TopologyConfig::default())?;
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

/// P4：事故の属性 `accident_attrs.bin` を作る（形式書12章）。
fn accident_attrs(options: &Options) -> Result<(), String> {
    let _ = options;
    todo!("P4 accident-attrs")
}

/// P5：事故を道路に割り当てて `assignment.bin` を作る（形式書13章）。
fn accident_snap(options: &Options) -> Result<(), String> {
    let _ = options;
    todo!("P5 accident-snap")
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
