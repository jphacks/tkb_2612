//! 評価と計測のコマンド（担当：②、形式書18章）。
//!
//! ```sh
//! cargo run --release --manifest-path backend/Cargo.toml --bin routing_eval -- <サブコマンド> [--名前 値]...
//! ```
//!
//! | サブコマンド | 内容 |
//! |---|---|
//! | `legality` | 経路の合法性の検査（`domain::routing::legality`） |
//! | `bench` | 探索の計測（時間・メモリ） |
//! | `od-sample` | 出発地・目的地の組の抽出 |
//! | `y-metrics` | リスク指標の評価 |
//! | `graph-stats` | グラフ一式（`graph.bin`・`restrictions.bin`・`topology.bin`・`odindex.bin`）を読み込んで、件数とRSSを表示する（M1の受け入れ条件） |
//!
//! `graph-stats` は `--dir <DIR>`（4つの `.bin` があるディレクトリ）を受け取る。`--parts-only true` を
//! 付けると、`Graph::from_parts` と各 `validate` を省いて節だけを読む（検査にかかる時間の切り分け用）。
//! ほかのサブコマンドは骨組みのまま（`todo!()`）。

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use accident_route_backend::contract::Mode;
use accident_route_backend::data::routing_files::{
    decode_graph, decode_graph_parts, decode_odindex, decode_restrictions, decode_topology,
};
use accident_route_backend::domain::routing::graph::restrictions::TurnAutomaton;

const USAGE: &str =
    "使い方: routing_eval <legality|bench|od-sample|y-metrics|graph-stats> [--名前 値]...";

/// `--名前 値` の組。名前は先頭の `--` を除いて持つ。
struct Options(BTreeMap<String, String>);

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

/// `/proc/self/status` の VmRSS・VmHWM（MB）。
fn memory_mb() -> (f64, f64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|kb| kb.parse::<f64>().ok())
            .map_or(f64::NAN, |kb| kb / 1024.0)
    };
    (field("VmRSS:"), field("VmHWM:"))
}

fn read_bin(dir: &Path, name: &str) -> Result<Vec<u8>, String> {
    std::fs::read(dir.join(name)).map_err(|e| format!("{name} を読めません: {e}"))
}

/// グラフ一式を読み込んで、件数とRSSを表示する。
fn graph_stats(options: &Options) -> Result<(), String> {
    let dir = options
        .0
        .get("dir")
        .map(Path::new)
        .ok_or("--dir が必要です")?;
    let parts_only = options.0.get("parts-only").is_some_and(|v| v == "true");
    let started = Instant::now();
    let report = |label: &str| {
        let (rss, hwm) = memory_mb();
        println!(
            "[{:>6.1}s] {label:<28} VmRSS={rss:.0}MB VmHWM={hwm:.0}MB",
            started.elapsed().as_secs_f64()
        );
    };
    report("開始");

    let bytes = read_bin(dir, "graph.bin")?;
    report("graph.bin を読んだ");
    let graph = if parts_only {
        None
    } else {
        Some(decode_graph(&bytes).map_err(|e| e.to_string())?.value)
    };
    let parts_copy;
    let (nodes, edges, shapes) = match &graph {
        Some(g) => {
            let p = g.parts();
            (p.node_ll.len(), p.edges.len(), p.shape.len())
        }
        None => {
            parts_copy = decode_graph_parts(&bytes).map_err(|e| e.to_string())?.value;
            (
                parts_copy.node_ll.len(),
                parts_copy.edges.len(),
                parts_copy.shape.len(),
            )
        }
    };
    drop(bytes);
    report("graph を復号した");
    println!("  ノード {nodes} 辺 {edges} 形状点 {shapes}");

    let bytes = read_bin(dir, "restrictions.bin")?;
    let restrictions = decode_restrictions(&bytes)
        .map_err(|e| e.to_string())?
        .value;
    drop(bytes);
    if let Some(g) = &graph {
        restrictions.validate(g).map_err(|e| e.to_string())?;
    }
    report("restrictions.bin");
    let seq_len: usize = restrictions.items.iter().map(|r| r.seq.len()).sum();
    println!(
        "  禁止 {} 件（列の長さの合計 {seq_len}）",
        restrictions.items.len()
    );
    for mode in Mode::ALL {
        let automaton = TurnAutomaton::build(&restrictions, mode);
        println!(
            "  {} の照合器の状態数 {}",
            mode.as_str(),
            automaton.state_count()
        );
    }
    report("照合器を作った");

    let bytes = read_bin(dir, "topology.bin")?;
    let topology = decode_topology(&bytes).map_err(|e| e.to_string())?.value;
    drop(bytes);
    if let Some(g) = &graph {
        topology.validate(g).map_err(|e| e.to_string())?;
    }
    report("topology.bin");
    let snappable = |mode: Mode| {
        (0..edges as u32)
            .filter(|&e| topology.snappable(e, mode))
            .count()
    };
    println!(
        "  クラスタ {} 双子の記録 {} 歩道の親 {} 吸着可能な辺 car={} bike={} walk={}",
        topology.cluster_count(),
        topology.twins.len(),
        topology.sidewalk_parents.len(),
        snappable(Mode::Car),
        snappable(Mode::Bike),
        snappable(Mode::Walk)
    );

    let bytes = read_bin(dir, "odindex.bin")?;
    let odindex = decode_odindex(&bytes).map_err(|e| e.to_string())?.value;
    drop(bytes);
    report("odindex.bin");
    println!(
        "  格子 {}x{} 空でないセル {} 登録 {}",
        odindex.params.rows,
        odindex.params.cols,
        odindex.cell_keys.len(),
        odindex.cell_edges.len()
    );
    report("完了（4ファイルを保持）");
    // 読み込んだものを最後まで保持して、上のRSSが常駐の値になるようにする
    std::hint::black_box((&graph, &restrictions, &topology, &odindex));
    Ok(())
}

/// 経路の合法性の検査。
fn legality(options: &Options) -> Result<(), String> {
    let _ = options;
    todo!("legality")
}

/// 探索の計測。
fn bench(options: &Options) -> Result<(), String> {
    let _ = options;
    todo!("bench")
}

/// 出発地・目的地の組の抽出。
fn od_sample(options: &Options) -> Result<(), String> {
    let _ = options;
    todo!("od-sample")
}

/// リスク指標の評価。
fn y_metrics(options: &Options) -> Result<(), String> {
    let _ = options;
    todo!("y-metrics")
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(command) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let run: fn(&Options) -> Result<(), String> = match command.to_str() {
        Some("legality") => legality,
        Some("bench") => bench,
        Some("od-sample") => od_sample,
        Some("y-metrics") => y_metrics,
        Some("graph-stats") => graph_stats,
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
            eprintln!("評価に失敗しました: {message}");
            ExitCode::FAILURE
        }
    }
}
