//! fixtureの例から人工releaseを永続ディレクトリに書き出す（C4の起動手順用。担当：②）。
//!
//! ```sh
//! cargo run --manifest-path backend/Cargo.toml --example make_test_release -- <出力先> <例の名前>
//! ```
//!
//! 出力先に `accidents.csv.gz`・`metadata.json`・`syukujitsu.csv` ができる。
//! `ACCIDENT_DATA_DIR` にこのディレクトリを指定すると、実エンジンで起動できる。
//! 本番バイナリには入らない。

use std::path::PathBuf;
use std::process::ExitCode;

use accident_route_backend::testing::fixtures::{route_examples, route_response_names};
use accident_route_backend::testing::write_example_release;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [output, name] = args.as_slice() else {
        eprintln!("使い方: make_test_release <出力先> <例の名前>");
        eprintln!("例の名前: {}", route_response_names().join(", "));
        return ExitCode::from(2);
    };
    let Some(example) = route_examples().into_iter().find(|e| &e.name == name) else {
        eprintln!(
            "例 {name} がありません（{}）",
            route_response_names().join(", ")
        );
        return ExitCode::from(2);
    };
    match write_example_release(&example, &PathBuf::from(output)) {
        Ok(ids) => {
            println!("release_dir={output}");
            println!("dataset_id={}", ids.dataset_id);
            println!("release_id={}", ids.release_id);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("人工releaseを書けません: {e}");
            ExitCode::FAILURE
        }
    }
}
