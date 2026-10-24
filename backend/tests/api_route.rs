//! S6：routeハンドラの期限・取消し・settle・エラー変換・受付上限・gzip・転送期限。

mod common;

use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

use accident_route_backend::api::transfer::{ConnectionKill, TransferBody};
use accident_route_backend::contract::{EngineError, Interrupt, LonLat, RouteOutcome};
use accident_route_backend::testing::fixtures::{
    expected_route_outcome, fixture_dataset_info, route_example,
};
use accident_route_backend::testing::{
    ScriptedRouter, StallingRouter, StubEngine, StubEvaluation, valid_routes,
};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use bytes::Bytes;
use http_body_util::BodyExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use common::{ErrorEngine, FailAt, LiveServer, PanicEngine, TestApp, TestAppBuilder};

const SHORT: Duration = Duration::from_millis(150);

fn scripted() -> Arc<ScriptedRouter> {
    Arc::new(ScriptedRouter::new([valid_routes(
        &route_example("singleRoute").routes,
    )]))
}

fn app_with(
    engine: Arc<dyn accident_route_backend::contract::RouteEngine>,
    timeout: Duration,
) -> TestApp {
    let mut builder = TestAppBuilder {
        engine: Some(engine),
        router: scripted(),
        ..Default::default()
    };
    builder.settings.request_timeout = timeout;
    builder.build()
}

fn route_request(accept_encoding: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("/api/route")
        .header("content-type", "application/json");
    if let Some(value) = accept_encoding {
        builder = builder.header("accept-encoding", value);
    }
    builder
        .body(Body::from(common::valid_route_body().to_string()))
        .unwrap()
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "条件が満たされない"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn deadline_while_waiting_for_upstream_is_503() {
    let router = Arc::new(StallingRouter::new());
    let mut builder = TestAppBuilder {
        router: router.clone(),
        ..Default::default()
    };
    builder.settings.request_timeout = SHORT;
    let app = builder.build();
    let started = Instant::now();
    let response = app.send(route_request(None)).await;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.error_code(), "service_busy");
    assert!(started.elapsed() < SHORT + Duration::from_secs(1));
    assert_eq!(router.call_count(), 1);
}

#[tokio::test]
async fn deadline_during_aggregation_is_503_and_stops_the_aggregation() {
    let engine = Arc::new(StubEngine::new(
        fixture_dataset_info(),
        StubEvaluation::SpinUntilInterrupted,
    ));
    let app = app_with(engine.clone(), SHORT);
    let response = app.send(route_request(None)).await;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.error_code(), "service_busy");
    assert!(engine.evaluate_started());
    wait_until(|| engine.evaluate_finished()).await;
}

#[tokio::test]
async fn result_completed_after_the_deadline_is_not_200() {
    let engine = Arc::new(StubEngine::new(
        fixture_dataset_info(),
        StubEvaluation::ReturnAfterInterrupt(Box::new(expected_route_outcome("singleRoute"))),
    ));
    let app = app_with(engine.clone(), SHORT);
    let response = app.send(route_request(Some("gzip"))).await;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.error_code(), "service_busy");
    wait_until(|| engine.evaluate_finished()).await;
}

#[tokio::test]
async fn dropping_the_request_cancels_the_aggregation() {
    let engine = Arc::new(StubEngine::new(
        fixture_dataset_info(),
        StubEvaluation::SpinUntilInterrupted,
    ));
    let app = app_with(engine.clone(), Duration::from_secs(30));
    let task = tokio::spawn(async move { app.send(route_request(None)).await });
    wait_until(|| engine.evaluate_started()).await;
    task.abort();
    let started = Instant::now();
    wait_until(|| engine.evaluate_finished()).await;
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn engine_errors_map_to_the_readme_table() {
    let cases = [
        (EngineError::NoRoute, FailAt::Collect, 502, "no_route"),
        (
            EngineError::RouteTooLong,
            FailAt::Collect,
            422,
            "route_too_long",
        ),
        (
            EngineError::ExternalApi,
            FailAt::Collect,
            502,
            "external_api_error",
        ),
        (EngineError::Busy, FailAt::Collect, 503, "service_busy"),
        (
            EngineError::Interrupted(Interrupt::DeadlineExceeded),
            FailAt::Collect,
            503,
            "service_busy",
        ),
        (
            EngineError::Interrupted(Interrupt::DeadlineExceeded),
            FailAt::Evaluate,
            503,
            "service_busy",
        ),
        (
            EngineError::ContractViolation("相関".into()),
            FailAt::Evaluate,
            500,
            "internal_error",
        ),
    ];
    for (error, at, status, code) in cases {
        let app = app_with(ErrorEngine::new(error.clone(), at), Duration::from_secs(5));
        let response = app.send(route_request(None)).await;
        assert_eq!(response.status.as_u16(), status, "{error:?}");
        assert_eq!(response.error_code(), code, "{error:?}");
        common::assert_api_headers(&response);
        if code == "external_api_error" {
            assert_eq!(
                response.json()["error"]["message"],
                "経路サービスに接続できません。時間をおいて再試行してください。"
            );
        }
    }
    let app = app_with(PanicEngine::new(), Duration::from_secs(5));
    let response = app.send(route_request(None)).await;
    assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(response.error_code(), "internal_error");
}

#[tokio::test]
async fn admission_limit_returns_503_while_full() {
    let engine = Arc::new(StubEngine::new(
        fixture_dataset_info(),
        StubEvaluation::SpinUntilInterrupted,
    ));
    let mut builder = TestAppBuilder {
        engine: Some(engine.clone()),
        router: Arc::new(ScriptedRouter::new([
            valid_routes(&route_example("singleRoute").routes),
            valid_routes(&route_example("singleRoute").routes),
        ])),
        ..Default::default()
    };
    builder.settings.request_timeout = Duration::from_secs(1);
    builder.settings.route_admission_limit = 1;
    let app = builder.build();
    let first = {
        let app_router = app.router.clone();
        tokio::spawn(async move {
            use tower::ServiceExt;
            app_router
                .oneshot(route_request(None))
                .await
                .unwrap()
                .status()
        })
    };
    wait_until(|| engine.evaluate_started()).await;
    let second = app.send(route_request(None)).await;
    assert_eq!(second.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(second.error_code(), "service_busy");
    assert_eq!(first.await.unwrap(), StatusCode::SERVICE_UNAVAILABLE);
}

fn gunzip(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut out)
        .unwrap();
    out
}

#[tokio::test]
async fn gzip_is_required_when_allowed_and_absent_otherwise() {
    let send = |accept: Option<&'static str>| async move {
        let app = app_with(common::stub_engine("twoSelected"), Duration::from_secs(5));
        app.send(route_request(accept)).await
    };
    let identity = send(Some("identity")).await;
    assert_eq!(identity.status, StatusCode::OK);
    assert_eq!(identity.header("content-encoding"), None);
    for accept in ["gzip", "gzip, deflate, br", "br;q=1, gzip;q=0.5", "*"] {
        let gzip = send(Some(accept)).await;
        assert_eq!(gzip.status, StatusCode::OK, "{accept}");
        assert_eq!(gzip.header("content-encoding"), Some("gzip"), "{accept}");
        assert_eq!(gzip.header("vary"), Some("accept-encoding"));
        assert_eq!(gzip.header("cache-control"), Some("no-store"));
        assert_eq!(gunzip(&gzip.body), identity.body.to_vec(), "{accept}");
    }
    for accept in [None, Some("gzip;q=0"), Some("identity, *;q=0"), Some("br")] {
        let plain = send(accept).await;
        assert_eq!(plain.status, StatusCode::OK);
        assert_eq!(plain.header("content-encoding"), None, "{accept:?}");
        assert_eq!(plain.header("vary"), Some("accept-encoding"));
        assert_eq!(plain.body, identity.body);
    }
}

/// 未選定の候補（twoSelectedのroute-3）のgeometryを大きくして、送信バッファより大きい応答を作る。
fn huge_outcome(vertices: usize) -> RouteOutcome {
    let mut outcome = expected_route_outcome("twoSelected");
    let index = outcome
        .candidates
        .iter()
        .position(|c| c.navigation.is_none())
        .unwrap();
    outcome.candidates[index].path.coordinates = (0..vertices)
        .map(|i| {
            LonLat::new(
                139.0 + i as f64 * 1.234_567e-7,
                35.0 + i as f64 * 7.654_321e-8,
            )
        })
        .collect();
    outcome
}

/// 転送期限：送信開始から `ROUTE_TRANSFER_TIMEOUT_MS` で、読まないクライアントとの接続を切る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transfer_deadline_closes_a_stalled_connection() {
    let engine = Arc::new(StubEngine::new(
        fixture_dataset_info(),
        StubEvaluation::Fixed(Box::new(huge_outcome(1_000_000))),
    ));
    let mut builder = TestAppBuilder {
        engine: Some(engine),
        router: scripted(),
        ..Default::default()
    };
    builder.settings.route_transfer_timeout = Duration::from_millis(300);
    let app = builder.build();
    let server = LiveServer::start(app.router.clone()).await;
    let mut stream = tokio::net::TcpStream::connect(server.addr).await.unwrap();
    let body = common::valid_route_body().to_string();
    let request = format!(
        "POST /api/route HTTP/1.1\r\nhost: test\r\ncontent-type: application/json\r\naccept-encoding: identity\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    // 応答の先頭だけ読む
    let mut head = vec![0u8; 4096];
    let n = stream.read(&mut head).await.unwrap();
    let head_text = String::from_utf8_lossy(&head[..n]).to_ascii_lowercase();
    assert!(head_text.starts_with("http/1.1 200"), "{head_text}");
    assert!(head_text.contains("connection: close"));
    let length: usize = head_text
        .lines()
        .find_map(|l| l.strip_prefix("content-length: "))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // 読まずに待つ（サーバの送信が止まり、期限で接続が切られる）
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let mut received = n;
    let mut buf = vec![0u8; 1 << 16];
    loop {
        match tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf)).await {
            Ok(Ok(0)) | Ok(Err(_)) => break,
            Ok(Ok(k)) => received += k,
            Err(_) => panic!("接続が切られない"),
        }
    }
    assert!(
        received < length,
        "途中で切られていない: {received} >= {length}"
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reading_client_receives_the_whole_body_over_a_real_connection() {
    let app = app_with(common::stub_engine("twoSelected"), Duration::from_secs(5));
    let server = LiveServer::start(app.router.clone()).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = client
        .post(server.url("/api/route"))
        .header("content-type", "application/json")
        .body(common::valid_route_body().to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["connection"], "close");
    let json: serde_json::Value = response.json().await.unwrap();
    assert_eq!(json["fastest_id"], "route-1");
    server.stop().await;
}

/// 拡張（ConnectionKill）が無いときは、期限後に読まれた時点で本文をエラーにする。
#[tokio::test(start_paused = true)]
async fn transfer_body_fails_after_the_deadline_without_a_connection_kill() {
    let data = Bytes::from(vec![b'x'; 200 * 1024]);
    let mut body = TransferBody::new(data, Duration::from_millis(100), None);
    let first = body.frame().await.unwrap().unwrap();
    assert_eq!(first.into_data().unwrap().len(), 64 * 1024);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(body.frame().await.unwrap().is_err());
}

#[tokio::test(start_paused = true)]
async fn transfer_body_notifies_the_connection_kill_after_the_deadline() {
    let kill = ConnectionKill(Arc::new(tokio::sync::Notify::new()));
    let data = Bytes::from(vec![b'x'; 10]);
    let mut body = TransferBody::new(data, Duration::from_millis(100), Some(&kill));
    let frame = body.frame().await.unwrap().unwrap();
    assert_eq!(frame.into_data().unwrap().len(), 10);
    assert!(body.frame().await.is_none());
    let notified = kill.0.notified();
    tokio::time::sleep(Duration::from_millis(150)).await;
    tokio::time::timeout(Duration::from_millis(1), notified)
        .await
        .expect("期限後に接続の切断が通知される");
}

/// 約12MBに近い合成応答のJSON準備・圧縮の時間とバイト数を記録する（合格基準はC5）。
/// `cargo test --release --test api_route -- --ignored --nocapture large_response` で実行する。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "計測用。PRに結果を記録する"]
async fn large_response_measurement() {
    let outcome = common_large::large_outcome();
    let engine = Arc::new(StubEngine::new(
        fixture_dataset_info(),
        StubEvaluation::Fixed(Box::new(outcome)),
    ));
    for accept in ["identity", "gzip"] {
        let app = app_with(engine.clone(), Duration::from_secs(35));
        let started = Instant::now();
        let response = app.send(route_request(Some(accept))).await;
        let elapsed = started.elapsed();
        assert_eq!(response.status, StatusCode::OK);
        let raw = if accept == "gzip" {
            gunzip(&response.body).len()
        } else {
            response.body.len()
        };
        println!(
            "{accept}: 応答本文{}バイト（展開後{raw}バイト）、受付から本文受信まで{elapsed:?}",
            response.body.len()
        );
    }
}

mod common_large {
    use accident_route_backend::contract::{
        HighRiskSegment, LonLat, NavigationData, NavigationSection, RouteOutcome,
    };
    use accident_route_backend::testing::fixtures::expected_route_outcome;

    const VERTICES: usize = 15_000;
    const SECTIONS: u32 = 14_000;

    /// 6候補×15,000頂点、選定2本にナビ区間を持つ合成結果。
    pub fn large_outcome() -> RouteOutcome {
        let mut outcome = expected_route_outcome("twoSelected");
        let template = outcome.candidates[0].clone();
        outcome.candidates = (0..6)
            .map(|i| {
                let mut c = template.clone();
                c.path.coordinates = (0..VERTICES)
                    .map(|k| {
                        LonLat::new(
                            139.0 + i as f64 * 1e-3 + k as f64 * 1.234_567_1e-5,
                            35.0 + k as f64 * 9.876_543_2e-6,
                        )
                    })
                    .collect();
                c.high_risk_total = u64::from(SECTIONS);
                c.high_risk_sections = (0..100)
                    .map(|j| HighRiskSegment {
                        segment_index: j * 2,
                        source_vertex_start_index: j,
                        start: c.path.coordinates[j as usize],
                        end: c.path.coordinates[j as usize + 1],
                        severity_weight: 3,
                    })
                    .collect();
                c.navigation = (i < 2).then(|| {
                    let distances: Vec<f64> =
                        (0..VERTICES).map(|k| k as f64 * 7.123_456_789).collect();
                    let length = *distances.last().unwrap();
                    NavigationData {
                        projection_lat_deg: 35.0,
                        length_m: length,
                        vertex_distances_m: distances,
                        sections: (0..SECTIONS)
                            .map(|j| NavigationSection {
                                start_segment_index: j * 2,
                                end_segment_index: j * 2,
                                start_m: f64::from(j) * 7.5,
                                end_m: f64::from(j) * 7.5 + 3.75,
                                severity_weight: 3,
                            })
                            .collect(),
                    }
                });
                c
            })
            .collect();
        outcome.fastest = 0;
        outcome.safest = Some(1);
        outcome
    }
}
