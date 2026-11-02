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
//!
//! 骨組み：各サブコマンドの本体は未実装（`todo!()`）。引数の名前は実装時に決める。

use std::collections::BTreeMap;
use std::process::ExitCode;

const USAGE: &str = "使い方: routing_eval <legality|bench|od-sample|y-metrics> [--名前 値]...";

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
