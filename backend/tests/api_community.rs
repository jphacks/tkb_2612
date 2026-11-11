//! 安全危険情報の共有（Issue #23）：`/api/reports` の入出力・重複投票・エラー。

mod common;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};

use common::{TestApp, TestAppBuilder, TestResponse};

const ALICE: &str = "alice-0123456789abcdef";
const BOB: &str = "bob-0123456789abcdefgh";
const TOKYO: &str = "/api/reports?bbox=139.7,35.6,139.8,35.7";

fn app() -> TestApp {
    TestAppBuilder::default().build()
}

async fn send(
    app: &TestApp,
    method: Method,
    target: &str,
    voter: Option<&str>,
    body: Option<Value>,
) -> TestResponse {
    let mut builder = Request::builder().method(method).uri(target);
    if let Some(voter) = voter {
        builder = builder.header("x-voter-id", voter);
    }
    let body = match body {
        Some(body) => {
            builder = builder.header("content-type", "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    app.send(builder.body(body).unwrap()).await
}

async fn create(app: &TestApp, voter: &str, body: Value) -> TestResponse {
    send(app, Method::POST, "/api/reports", Some(voter), Some(body)).await
}

async fn vote(app: &TestApp, id: &str, voter: &str, body: Value) -> TestResponse {
    let target = format!("/api/reports/{id}/votes");
    send(app, Method::POST, &target, Some(voter), Some(body)).await
}

async fn list(app: &TestApp, target: &str, voter: Option<&str>) -> TestResponse {
    send(app, Method::GET, target, voter, None).await
}

#[tokio::test]
async fn a_report_is_created_listed_and_voted_on() {
    let app = app();
    let created = create(
        &app,
        ALICE,
        json!({"lat": 35.68, "lon": 139.76, "kind": "danger", "text": "  夜は見通しが悪い  "}),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    common::assert_api_headers(&created);
    let report = &created.json()["report"];
    let id = report["id"].as_str().unwrap().to_owned();
    assert_eq!(id.len(), 32);
    assert_eq!(report["safe_count"], 0);
    assert_eq!(report["danger_count"], 1);
    assert_eq!(report["my_vote"], "danger");
    assert_eq!(report["created_at"], "2026-10-24T00:00:00.000Z");
    assert_eq!(report["comments"][0]["text"], "夜は見通しが悪い");
    assert_eq!(report["comments"][0]["kind"], "danger");

    // 別の投票者は「安全」に投票でき、票は種類ごとに数える
    let voted = vote(&app, &id, BOB, json!({"kind": "safe"})).await;
    assert_eq!(voted.status, StatusCode::OK);
    let report = &voted.json()["report"];
    assert_eq!(report["safe_count"], 1);
    assert_eq!(report["danger_count"], 1);
    assert_eq!(report["my_vote"], "safe");
    assert_eq!(report["comments"].as_array().unwrap().len(), 1);

    // 一覧は投票者ごとにmy_voteを返し、IDを送らなければnull
    let listed = list(&app, TOKYO, Some(ALICE)).await;
    assert_eq!(listed.status, StatusCode::OK);
    common::assert_api_headers(&listed);
    let reports = listed.json()["reports"].clone();
    assert_eq!(reports.as_array().unwrap().len(), 1);
    assert_eq!(reports[0]["id"], id);
    assert_eq!(reports[0]["my_vote"], "danger");
    assert_eq!(
        list(&app, TOKYO, None).await.json()["reports"][0]["my_vote"],
        Value::Null
    );
    let outside = list(&app, "/api/reports?bbox=135.0,34.0,135.1,34.1", None).await;
    assert_eq!(outside.json()["reports"], json!([]));
}

#[tokio::test]
async fn a_voter_votes_once_per_report() {
    let app = app();
    let created = create(
        &app,
        ALICE,
        json!({"lat": 35.68, "lon": 139.76, "kind": "safe"}),
    )
    .await;
    let id = created.json()["report"]["id"].as_str().unwrap().to_owned();
    assert!(
        created.json()["report"]["comments"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let again = vote(&app, &id, ALICE, json!({"kind": "danger", "text": "x"})).await;
    assert_eq!(again.status, StatusCode::CONFLICT);
    assert_eq!(again.error_code(), "already_voted");
    common::assert_api_headers(&again);

    assert_eq!(
        vote(&app, &id, BOB, json!({"kind": "safe"})).await.status,
        StatusCode::OK
    );
    let twice = vote(&app, &id, BOB, json!({"kind": "safe"})).await;
    assert_eq!(twice.error_code(), "already_voted");
    let report = &list(&app, TOKYO, None).await.json()["reports"][0];
    assert_eq!(report["safe_count"], 2);
    assert_eq!(report["danger_count"], 0);
}

#[tokio::test]
async fn unknown_reports_are_not_found() {
    let app = app();
    let missing = vote(&app, &"0".repeat(32), ALICE, json!({"kind": "safe"})).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.error_code(), "not_found");
    let malformed = vote(&app, "not-an-id", ALICE, json!({"kind": "safe"})).await;
    assert_eq!(malformed.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn invalid_posts_are_rejected() {
    let app = app();
    let cases = [
        json!({"lat": 35.68, "lon": 139.76, "kind": "maybe"}),
        json!({"lat": 35.68, "lon": 139.76}),
        json!({"lat": 10.0, "lon": 139.76, "kind": "safe"}),
        json!({"lat": "35.68", "lon": 139.76, "kind": "safe"}),
        json!({"lat": 35.68, "lon": 139.76, "kind": "safe", "text": 1}),
        json!({"lat": 35.68, "lon": 139.76, "kind": "safe", "text": "あ".repeat(501)}),
        json!({"lat": 35.68, "lon": 139.76, "kind": "safe", "extra": true}),
        json!([1, 2]),
    ];
    for body in cases {
        let response = create(&app, ALICE, body.clone()).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(response.error_code(), "invalid_request", "{body}");
    }
    // 500文字ちょうどは受け付ける
    let longest = create(
        &app,
        ALICE,
        json!({"lat": 35.68, "lon": 139.76, "kind": "safe", "text": "あ".repeat(500)}),
    )
    .await;
    assert_eq!(longest.status, StatusCode::CREATED);

    let body = json!({"lat": 35.68, "lon": 139.76, "kind": "safe"});
    for voter in [None, Some("short"), Some("has space 0123456789")] {
        let response = send(
            &app,
            Method::POST,
            "/api/reports",
            voter,
            Some(body.clone()),
        )
        .await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{voter:?}");
    }
    let plain = app
        .send(
            Request::builder()
                .method(Method::POST)
                .uri("/api/reports")
                .header("x-voter-id", ALICE)
                .header("content-type", "text/plain")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await;
    assert_eq!(plain.status, StatusCode::BAD_REQUEST);
    let duplicate_key = app
        .send(
            Request::builder()
                .method(Method::POST)
                .uri("/api/reports")
                .header("x-voter-id", ALICE)
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"lat":35.68,"lon":139.76,"kind":"safe","kind":"danger"}"#,
                ))
                .unwrap(),
        )
        .await;
    assert_eq!(duplicate_key.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn invalid_bboxes_are_rejected() {
    let app = app();
    for target in [
        "/api/reports",
        "/api/reports?bbox=1,2,3",
        "/api/reports?bbox=139.8,35.6,139.7,35.7",
        "/api/reports?bbox=139.7,35.6,139.8,95",
        "/api/reports?bbox=a,b,c,d",
        "/api/reports?bbox=139.7,35.6,139.8,35.7&bbox=139.7,35.6,139.8,35.7",
    ] {
        let response = list(&app, target, None).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{target}");
        assert_eq!(response.error_code(), "invalid_request", "{target}");
    }
}

#[tokio::test]
async fn bboxes_wider_than_one_degree_are_rejected() {
    let app = app();
    for target in [
        "/api/reports?bbox=139.0,35.0,140.5,35.5",
        "/api/reports?bbox=139.0,35.0,139.5,36.5",
    ] {
        let response = list(&app, target, None).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{target}");
        assert_eq!(response.error_code(), "invalid_request", "{target}");
    }
    // ちょうど1度（引き算の誤差が出る値も）は受け付ける
    for target in [
        "/api/reports?bbox=139.0,35.0,140.0,36.0",
        "/api/reports?bbox=139.70000,35.60000,140.70000,36.60000",
    ] {
        assert_eq!(
            list(&app, target, None).await.status,
            StatusCode::OK,
            "{target}"
        );
    }
}

#[tokio::test]
async fn other_methods_are_not_allowed() {
    let app = app();
    let put = send(&app, Method::PUT, "/api/reports", Some(ALICE), None).await;
    assert_eq!(put.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(put.header("allow"), Some("GET, POST"));
    let head = send(&app, Method::HEAD, TOKYO, None, None).await;
    assert_eq!(head.status, StatusCode::METHOD_NOT_ALLOWED);
    let get_votes = send(
        &app,
        Method::GET,
        &format!("/api/reports/{}/votes", "0".repeat(32)),
        None,
        None,
    )
    .await;
    assert_eq!(get_votes.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(get_votes.header("allow"), Some("POST"));
}

#[tokio::test]
async fn the_newest_reports_come_first() {
    let clock = common::ManualClock::new(common::fixed_now());
    let app = TestAppBuilder {
        clock: clock.clone(),
        ..Default::default()
    }
    .build();
    let first = create(
        &app,
        ALICE,
        json!({"lat": 35.65, "lon": 139.71, "kind": "safe"}),
    )
    .await;
    clock.advance(std::time::Duration::from_secs(60));
    let second = create(
        &app,
        ALICE,
        json!({"lat": 35.66, "lon": 139.72, "kind": "danger"}),
    )
    .await;
    let reports = list(&app, TOKYO, None).await.json()["reports"].clone();
    assert_eq!(reports[0]["id"], second.json()["report"]["id"]);
    assert_eq!(reports[1]["id"], first.json()["report"]["id"]);
}
