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
// 骨組み：サブコマンドの実装が読むまで未使用
#[allow(dead_code)]
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
