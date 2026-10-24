//! モジュール間の依存方向を検査する（README.mdの依存方向）。
//!
//! ソース中のパス（`name::`）を探す簡易検査で、コメント行と、ファイル末尾の
//! `#[cfg(test)]` 以降（単体テスト）は除く。単体テストではスタブ（`testing`）や
//! `#[tokio::test]` を使ってよい。
//! `use crate::{api, ...}` のように末尾が `::` でない書き方は検出できないので、
//! 他モジュールは `name::item` の形で参照する。

use std::fs;
use std::path::{Path, PathBuf};

/// `src/<module>` 以下の.rsファイル（`src/<module>.rs` の単一ファイルも含む）。
/// まだ無いモジュール（②の `src/bin/` 等）は空。
fn rust_files(module: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, files);
            } else if path.extension().is_some_and(|e| e == "rs") {
                files.push(path);
            }
        }
    }
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    let dir = src.join(module);
    if dir.is_dir() {
        walk(&dir, &mut files);
    }
    let single = src.join(format!("{module}.rs"));
    if single.is_file() {
        files.push(single);
    }
    files
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `code` に識別子 `name` に続く `::` があれば、その直後の文字列を返す。
fn path_uses<'a>(code: &'a str, name: &str) -> Vec<&'a str> {
    let needle = format!("{name}::");
    code.match_indices(&needle)
        .filter(|(i, _)| !code[..*i].chars().next_back().is_some_and(is_ident_char))
        .map(|(i, _)| &code[i + needle.len()..])
        .collect()
}

/// コード行を (ファイル, 行番号, 内容) で列挙する。
fn code_lines(module: &str) -> Vec<(PathBuf, usize, String)> {
    rust_files(module)
        .into_iter()
        .flat_map(|file| {
            let source = fs::read_to_string(&file).unwrap();
            source
                .lines()
                .enumerate()
                .take_while(|(_, line)| line.trim() != "#[cfg(test)]")
                .filter(|(_, line)| !line.trim_start().starts_with("//"))
                .map(|(n, line)| (file.clone(), n + 1, line.to_owned()))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn assert_no_dependency(module: &str, forbidden: &[&str]) {
    for (file, line, code) in code_lines(module) {
        for name in forbidden {
            assert!(
                path_uses(&code, name).is_empty(),
                "{}:{line}: {module} は {name} に依存してはいけません",
                file.display()
            );
        }
    }
}

const RUNTIME_AND_HTTP: [&str; 4] = ["tokio", "axum", "reqwest", "hyper"];

#[test]
fn path_detection_respects_identifier_boundaries() {
    assert_eq!(
        path_uses("use crate::data::load_engine;", "data"),
        ["load_engine;"]
    );
    assert!(path_uses("let m = metadata::read();", "data").is_empty());
    assert_eq!(path_uses("tokio::spawn(x)", "tokio"), ["spawn(x)"]);
}

#[test]
fn contract_depends_on_no_implementation() {
    let mut forbidden = vec!["domain", "data", "api", "providers", "testing"];
    forbidden.extend(RUNTIME_AND_HTTP);
    assert_no_dependency("contract", &forbidden);
}

/// ③の起動・設定・ログの部品（`api` と同じく、エンジン側へは契約と `data` の入口だけで触れる）。
const SERVER_SIDE: [&str; 4] = ["config", "startup", "logging", "main"];

#[test]
fn engine_side_does_not_depend_on_api_side() {
    let mut forbidden = vec![
        "api",
        "providers",
        "testing",
        "config",
        "startup",
        "logging",
    ];
    forbidden.extend(RUNTIME_AND_HTTP);
    assert_no_dependency("domain", &forbidden);
    assert_no_dependency("data", &forbidden);
    // ②の前処理コマンド（`src/bin/`）も同じ規則（共通前提2章）
    assert_no_dependency("bin", &forbidden);
}

#[test]
fn providers_depend_only_on_contract() {
    assert_no_dependency(
        "providers",
        &[
            "domain", "data", "api", "testing", "config", "startup", "logging",
        ],
    );
}

#[test]
fn server_side_uses_engine_only_through_contract_and_data_entry_points() {
    for module in SERVER_SIDE {
        assert!(!rust_files(module).is_empty(), "src/{module}.rs がない");
        assert_no_dependency(module, &["domain", "testing"]);
        assert_data_entry_points_only(module);
    }
}

/// `use crate::{a, b}` のまとめ書きは検査をすり抜けるので使わない。
#[test]
fn grouped_crate_imports_are_not_used() {
    for module in [
        "contract",
        "domain",
        "data",
        "api",
        "providers",
        "testing",
        "bin",
        "config",
        "startup",
        "logging",
        "main",
        "lib",
    ] {
        for (file, line, code) in code_lines(module) {
            let compact: String = code.chars().filter(|c| !c.is_whitespace()).collect();
            assert!(
                !compact.contains("crate::{") && !compact.contains("super::{"),
                "{}:{line}: crate::name::item の形で参照してください",
                file.display()
            );
        }
    }
}

#[test]
fn api_uses_engine_only_through_contract_and_data_entry_points() {
    assert_no_dependency("api", &["domain", "testing"]);
    assert_data_entry_points_only("api");
}

fn assert_data_entry_points_only(module: &str) {
    let allowed = ["load_calendar", "load_engine", "LoadError"];
    for (file, line, code) in code_lines(module) {
        for rest in path_uses(&code, "data") {
            assert!(
                allowed.iter().any(|name| rest.starts_with(name)),
                "{}:{line}: {module}から使えるdataは {allowed:?} だけです",
                file.display()
            );
        }
    }
}
