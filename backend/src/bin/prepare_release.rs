//! 前処理コマンド：原本 `accident/` と祝日CSVから不変のreleaseを作る（担当：②）。
//!
//! ```sh
//! cargo run --release --manifest-path backend/Cargo.toml --bin prepare_release -- \
//!     --accident-dir accident --holidays accident/syukujitsu.csv --output backend/data
//! ```
//!
//! `<output>/staging/` で3ファイルを作ってから `<output>/releases/<release_id>/` へ移し、
//! 読み取り専用にする。`current` は変更しない（公開するreleaseの切替は配備手順で行う）。
//! 原本は読むだけで書き込まない。コード表は常に実ファイルで照合する。

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use accident_route_backend::data::preprocess::codebook::FileCodebooks;
use accident_route_backend::data::preprocess::prepare_release;
use accident_route_backend::data::release::{make_read_only, write_release_files};

const USAGE: &str = "使い方: prepare_release --accident-dir <原本ディレクトリ> --holidays <祝日CSV> --output <backend/data>";

struct Args {
    accident_dir: PathBuf,
    holidays: PathBuf,
    output: PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let mut accident_dir = None;
    let mut holidays = None;
    let mut output = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(flag) = args.next() {
        let slot = match flag.to_str() {
            Some("--accident-dir") => &mut accident_dir,
            Some("--holidays") => &mut holidays,
            Some("--output") => &mut output,
            _ => return Err(format!("不明な引数です: {}", flag.to_string_lossy())),
        };
        let value = args
            .next()
            .ok_or_else(|| format!("{} の値がありません", flag.to_string_lossy()))?;
        *slot = Some(PathBuf::from(value));
    }
    match (accident_dir, holidays, output) {
        (Some(accident_dir), Some(holidays), Some(output)) => Ok(Args {
            accident_dir,
            holidays,
            output,
        }),
        _ => Err("必須の引数が足りません".to_owned()),
    }
}

fn run(args: &Args) -> Result<(), String> {
    let holidays = std::fs::read(&args.holidays)
        .map_err(|e| format!("{} を読めません: {e}", args.holidays.display()))?;
    let generated_at = chrono::Local::now().to_rfc3339();
    let prepared = prepare_release(&args.accident_dir, holidays, &FileCodebooks, &generated_at)
        .map_err(|e| e.to_string())?;
    let release = &prepared.release;

    let target = args.output.join("releases").join(&release.release_id);
    if target.exists() {
        return Err(format!(
            "{} は既にあります（releaseは不変なので上書きしません）",
            target.display()
        ));
    }
    let staging =
        args.output
            .join("staging")
            .join(format!("{}-{}", release.release_id, std::process::id()));
    let io = |path: &Path, e: std::io::Error| format!("{} に書けません: {e}", path.display());
    write_release_files(&staging, release).map_err(|e| io(&staging, e))?;
    std::fs::create_dir_all(target.parent().expect("releases/の下")).map_err(|e| io(&target, e))?;
    std::fs::rename(&staging, &target).map_err(|e| io(&target, e))?;
    make_read_only(&target).map_err(|e| io(&target, e))?;

    let summary = serde_json::json!({
        "release_dir": target.display().to_string(),
        "release_id": release.release_id,
        "dataset_id": release.dataset_id,
        "rows": release.metadata["rows"],
        "totals": release.metadata["totals"],
        "holiday_uncovered_rows": release.metadata["holiday_uncovered_rows"],
        "elapsed_seconds": release.metadata["elapsed_seconds"],
        "files": prepared.files.iter().map(|f| serde_json::json!({
            "source_year": f.source_year,
            "input_rows": f.input_rows,
            "retained_rows": f.retained_rows,
            "excluded_rows": f.excluded_rows,
            "excluded_by_reason": f.excluded_by_reason.iter()
                .map(|(r, n)| (r.as_str().to_owned(), serde_json::json!(n)))
                .collect::<serde_json::Map<_, _>>(),
            "no_target_mode": f.no_target_mode,
        })).collect::<Vec<_>>(),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&summary).expect("JSONに書ける値")
    );
    Ok(())
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("前処理に失敗しました: {message}");
            ExitCode::FAILURE
        }
    }
}
