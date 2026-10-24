//! S1：スタブを注入したアプリの組立てと、実 `data::load_*` での起動失敗。

mod common;

use accident_route_backend::startup::{self, Loaders, StartupError};
use axum::http::StatusCode;

use common::{TestAppBuilder, assert_json_matches};

#[tokio::test]
async fn stub_app_answers_health_with_the_fixture_dataset() {
    let app = TestAppBuilder::default().build();
    let response = app.get("/api/health").await;
    assert_eq!(response.status, StatusCode::OK);
    let zero = "0".repeat(64);
    assert_json_matches(
        &response.json(),
        &serde_json::json!({
            "api_version": "2.0",
            "status": "ok",
            "release_id": zero,
            "dataset_id": zero,
            "accidents": 3,
            "load_seconds": 0,
            "data_years": [2025],
        }),
    );
}

/// 実 `data::load_*` は②の実装まで常に失敗する仮実装なので、起動処理は `Err` を返す。
#[tokio::test]
async fn startup_with_the_real_loaders_fails_until_data_is_implemented() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("key");
    std::fs::write(&key, "dummy-key").unwrap();
    let data = dir.path().join("release");
    std::fs::create_dir(&data).unwrap();
    let config = common::test_config(&data, &key);
    let result = startup::run(&config, Loaders::production(), std::future::pending()).await;
    assert!(matches!(result, Err(StartupError::Data(_))), "{result:?}");
}
