//! ログの初期化（architecture.md 8章）。
//!
//! `RUST_LOG` のレベルに従うが、URL・クエリ・ヘッダー・本文を出し得る依存ライブラリ
//! （HTTPサーバ・クライアント・TLS）のログは、レベルによらず出さない。
//! アプリ自身のログには住所・座標・秘密・上流URLを含めない。

use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// レベルによらずログを出さない依存ライブラリ。
pub const BLOCKED_TARGETS: [&str; 8] = [
    "hyper",
    "hyper_util",
    "reqwest",
    "h2",
    "rustls",
    "axum",
    "tower",
    "tower_http",
];

/// `target` が出力しないライブラリのものか。
pub fn is_blocked_target(target: &str) -> bool {
    BLOCKED_TARGETS.iter().any(|name| {
        target == *name
            || target
                .strip_prefix(name)
                .is_some_and(|rest| rest.starts_with("::"))
    })
}

/// `RUST_LOG` を解釈できなかった。
#[derive(Debug)]
pub struct InvalidFilter;

impl std::fmt::Display for InvalidFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("環境変数 RUST_LOG が不正です")
    }
}

impl std::error::Error for InvalidFilter {}

/// 標準出力へのログを初期化する。プロセスで1回だけ呼ぶ。
pub fn init(rust_log: &str) -> Result<(), InvalidFilter> {
    init_with_writer(rust_log, std::io::stdout)
}

/// 書込み先を指定してログを初期化する（テストでメモリに捕捉するため）。プロセスで1回だけ呼ぶ。
pub fn init_with_writer<W>(rust_log: &str, writer: W) -> Result<(), InvalidFilter>
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let level = EnvFilter::try_new(rust_log).map_err(|_| InvalidFilter)?;
    let layer = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true)
        .with_filter(level)
        .with_filter(filter_fn(|meta| !is_blocked_target(meta.target())));
    tracing_subscriber::registry()
        .with(layer)
        .try_init()
        .map_err(|_| InvalidFilter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_targets_are_blocked_by_prefix() {
        assert!(is_blocked_target("hyper"));
        assert!(is_blocked_target("hyper::proto::h1"));
        assert!(is_blocked_target("hyper_util::client"));
        assert!(is_blocked_target("reqwest::connect"));
        assert!(!is_blocked_target("hyperx"));
        assert!(!is_blocked_target(
            "accident_route_backend::api::access_log"
        ));
        assert!(!is_blocked_target("access"));
    }
}
