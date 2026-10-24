//! S3：エラーJSON・共通ヘッダー・404/405・health。

mod common;

use axum::body::Body;
use axum::http::{Method, StatusCode};

use common::{TestAppBuilder, assert_api_headers, request};

#[tokio::test]
async fn health_and_status_return_the_same_json() {
    let app = TestAppBuilder::default().build();
    let health = app.get("/api/health").await;
    let status = app.get("/api/status").await;
    assert_eq!(health.status, StatusCode::OK);
    assert_eq!(status.status, StatusCode::OK);
    assert_eq!(health.body, status.body);
    assert_api_headers(&health);
    assert_api_headers(&status);
    let json = health.json();
    let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    // grid_cellsはグリッド索引を採用した場合だけ（fixtureはNone）
    assert_eq!(
        keys,
        [
            "accidents",
            "api_version",
            "data_years",
            "dataset_id",
            "load_seconds",
            "release_id",
            "status"
        ]
    );
}

#[tokio::test]
async fn health_rounds_load_seconds_and_reports_grid_cells_only_when_present() {
    use accident_route_backend::testing::fixtures::fixture_dataset_info;
    use accident_route_backend::testing::{StubEngine, StubEvaluation};
    use std::sync::Arc;

    let mut dataset = fixture_dataset_info();
    dataset.load_seconds = 1.23456;
    dataset.grid_cells = Some(42);
    let engine = StubEngine::new(dataset, StubEvaluation::SpinUntilInterrupted);
    let app = TestAppBuilder {
        engine: Some(Arc::new(engine)),
        ..Default::default()
    }
    .build();
    let json = app.get("/api/health").await.json();
    assert_eq!(json["load_seconds"], 1.235);
    assert_eq!(json["grid_cells"], 42);
}

#[tokio::test]
async fn not_ready_is_503_data_unavailable() {
    let app = TestAppBuilder {
        engine: None,
        ..Default::default()
    }
    .build();
    for target in ["/api/health", "/api/status"] {
        let response = app.get(target).await;
        assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.error_code(), "data_unavailable");
        assert_api_headers(&response);
    }
    let route = app
        .post_json("/api/route", common::valid_route_body().to_string())
        .await;
    assert_eq!(route.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(route.error_code(), "data_unavailable");
}

#[tokio::test]
async fn unknown_api_paths_are_404_json() {
    let app = TestAppBuilder::default().build();
    for (method, target) in [
        (Method::GET, "/api/unknown"),
        (Method::POST, "/api/unknown"),
        (Method::GET, "/api"),
        (Method::GET, "/api/"),
        (Method::GET, "/api/health/extra"),
        (Method::DELETE, "/api/a/b/c"),
        (Method::GET, "/api/route/"),
    ] {
        let response = app
            .send(request(method.clone(), target, Body::empty()))
            .await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{method} {target}");
        assert_eq!(response.error_code(), "not_found");
        assert_api_headers(&response);
    }
}

#[tokio::test]
async fn wrong_methods_on_defined_apis_are_405_with_allow() {
    let app = TestAppBuilder::default().build();
    let cases = [
        (Method::POST, "/api/health", "GET"),
        (Method::HEAD, "/api/health", "GET"),
        (Method::HEAD, "/api/status", "GET"),
        (Method::PUT, "/api/conditions", "GET"),
        (Method::HEAD, "/api/conditions", "GET"),
        (Method::POST, "/api/geocode", "GET"),
        (Method::HEAD, "/api/geocode", "GET"),
        (Method::GET, "/api/route", "POST"),
        (Method::HEAD, "/api/route", "POST"),
        (Method::DELETE, "/api/route", "POST"),
        (Method::OPTIONS, "/api/route", "POST"),
    ];
    for (method, target, allow) in cases {
        let response = app
            .send(request(method.clone(), target, Body::empty()))
            .await;
        assert_eq!(
            response.status,
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} {target}"
        );
        assert_eq!(response.header("allow"), Some(allow), "{method} {target}");
        assert_api_headers(&response);
        if method != Method::HEAD {
            assert_eq!(response.error_code(), "method_not_allowed");
        }
    }
}

#[tokio::test]
async fn error_messages_are_japanese_and_never_echo_the_input() {
    let app = TestAppBuilder::default().build();
    let response = app
        .post_json(
            "/api/route",
            r#"{"origin":{"lat":35.123456,"lon":999},"dest":{"lat":35.1,"lon":139.1}}"#,
        )
        .await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    let message = response.json()["error"]["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!message.contains("35.123456") && !message.contains("999"));
    assert!(
        message
            .chars()
            .any(|c| ('\u{3040}'..='\u{30ff}').contains(&c))
    );
}
