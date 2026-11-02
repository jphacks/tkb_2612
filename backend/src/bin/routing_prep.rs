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
//! 骨組み：各サブコマンドの本体は未実装（`todo!()`）。引数の名前は実装時に各段階で決める。

use std::collections::BTreeMap;
use std::process::ExitCode;

const USAGE: &str = "使い方: routing_prep <osm-graph|restrictions|topology|accident-attrs|accident-snap|risk-fit|od-index|alt|package> [--名前 値]...";

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

/// P1：OSM PBFから `graph.bin` を作る（形式書5〜8章）。
fn osm_graph(options: &Options) -> Result<(), String> {
    let _ = options;
    todo!("P1 osm-graph")
}

/// P2：turn restrictionを `restrictions.bin` にする（形式書9章）。
fn restrictions(options: &Options) -> Result<(), String> {
    let _ = options;
    todo!("P2 restrictions")
}

/// P3：`topology.bin` を作る（形式書10章）。
fn topology(options: &Options) -> Result<(), String> {
    let _ = options;
    todo!("P3 topology")
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
    let _ = options;
    todo!("P7 od-index")
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
