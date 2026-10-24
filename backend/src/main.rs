//! APIサーバ（担当：③）。設定は環境変数（architecture.md 6章）から読む。

use std::process::ExitCode;

use accident_route_backend::config::Config;
use accident_route_backend::logging;
use accident_route_backend::startup::{self, Loaders};

#[tokio::main]
async fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = logging::init(&config.rust_log) {
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }
    match startup::run(&config, Loaders::production(), startup::shutdown_signal()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "起動または実行に失敗しました");
            ExitCode::FAILURE
        }
    }
}
