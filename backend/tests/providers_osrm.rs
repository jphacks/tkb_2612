//! S8：OSRMアダプタ。ループバックの偽OSRMだけを相手にする（実通信しない）。

mod common;

use std::sync::Arc;
use std::time::Duration;

use accident_route_backend::contract::{
    CancelToken, Interrupt, LatLon, MalformedRoute, Mode, RoutedPath, Router, RoutingError,
    RoutingRequest,
};
use accident_route_backend::providers::gate::{Gate, GatePolicy, LocalGateStore};
use accident_route_backend::providers::http::UpstreamClient;
use accident_route_backend::providers::osrm::{OsrmEndpoints, OsrmRouter};
use accident_route_backend::testing::fixtures::{route_example, route_examples};
use accident_route_backend::testing::valid_routes;
use serde_json::{Value, json};

use common::{FakeUpstream, ManualClock, Reply};

const UA: &str = "accident-route-test/0.1 (+https://example.invalid/contact)";
const MAX_BYTES: usize = 12_000_000;

fn open_policy(queue_capacity: usize) -> GatePolicy {
    GatePolicy {
        min_interval: Duration::ZERO,
        max_concurrent: 1,
        queue_capacity,
        lease_ttl: Duration::from_secs(10),
        quotas: Vec::new(),
    }
}

fn router_with(
    upstream: &FakeUpstream,
    timeout: Duration,
    max_bytes: usize,
    policy: GatePolicy,
) -> OsrmRouter {
    OsrmRouter::new(
        OsrmEndpoints {
            walk: upstream.url("/foot/"),
            bike: upstream.url("/bike"),
            car: upstream.url("/car/"),
        },
        UpstreamClient::new(UA).unwrap(),
        Gate::new(
            "osrm",
            policy,
            Arc::new(LocalGateStore::new()),
            ManualClock::new(common::fixed_now()),
        ),
        timeout,
        max_bytes,
    )
}

fn router(upstream: &FakeUpstream) -> OsrmRouter {
    router_with(upstream, Duration::from_secs(8), MAX_BYTES, open_policy(16))
}

fn request(mode: Mode, via: Option<LatLon>) -> RoutingRequest {
    RoutingRequest {
        origin: LatLon::new(35.681_236_8, 139.767_125),
        dest: LatLon::new(35.713_8, 139.777_3),
        mode,
        via,
    }
}

fn osrm_body(routes: &[RoutedPath]) -> Value {
    json!({
        "code": "Ok",
        "routes": routes.iter().map(|r| json!({
            "duration": r.duration_s,
            "distance": r.distance_m,
            "weight": r.duration_s,
            "legs": [],
            "geometry": {
                "type": "LineString",
                "coordinates": r.coordinates.iter().map(|p| json!([p.lon, p.lat])).collect::<Vec<_>>(),
            },
        })).collect::<Vec<_>>(),
        "waypoints": [],
    })
}

fn ok_body() -> Value {
    osrm_body(&route_example("singleRoute").routes)
}

async fn route(router: &OsrmRouter, cancel: &CancelToken) -> Result<usize, RoutingError> {
    router
        .route(&request(Mode::Walk, None), cancel)
        .await
        .map(|routes| routes.len())
}

#[tokio::test]
async fn url_profile_coordinates_query_and_headers_follow_the_spec() {
    let upstream = FakeUpstream::fixed(Reply::json(&ok_body())).await;
    let router = router(&upstream);
    let via = Some(LatLon::new(35.7, 139.77));
    for (mode, via) in [(Mode::Walk, None), (Mode::Bike, via), (Mode::Car, None)] {
        router
            .route(&request(mode, via), &CancelToken::unbounded())
            .await
            .unwrap();
    }
    let requests = upstream.requests();
    let paths: Vec<_> = requests.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "/foot/route/v1/walking/139.7671250,35.6812368;139.7773000,35.7138000",
            "/bike/route/v1/cycling/139.7671250,35.6812368;139.7700000,35.7000000;139.7773000,35.7138000",
            "/car/route/v1/driving/139.7671250,35.6812368;139.7773000,35.7138000",
        ]
    );
    for r in &requests {
        assert_eq!(r.method, "GET");
        assert_eq!(
            r.query.as_deref(),
            Some("alternatives=true&geometries=geojson&overview=full&steps=false")
        );
        assert_eq!(r.headers["user-agent"], UA);
        assert_eq!(r.headers["accept"], "application/json");
    }
}

/// fixtureの全例の固定経路をOSRM形式にして往復させると、同じ経路に戻る。
#[tokio::test]
async fn fixture_routes_round_trip_through_the_osrm_format() {
    for example in route_examples() {
        let upstream = FakeUpstream::fixed(Reply::json(&osrm_body(&example.routes))).await;
        let routes = router(&upstream)
            .route(
                &request(example.query.mode(), None),
                &CancelToken::unbounded(),
            )
            .await;
        assert_eq!(routes, valid_routes(&example.routes), "{}", example.name);
    }
}

#[tokio::test]
async fn only_the_first_six_routes_are_returned_in_provider_order() {
    let base = &route_example("singleRoute").routes[0];
    let routes: Vec<RoutedPath> = (0..8)
        .map(|i| RoutedPath {
            duration_s: 100.0 + f64::from(i),
            ..base.clone()
        })
        .collect();
    let upstream = FakeUpstream::fixed(Reply::json(&osrm_body(&routes))).await;
    let parsed = router(&upstream)
        .route(&request(Mode::Walk, None), &CancelToken::unbounded())
        .await
        .unwrap();
    assert_eq!(parsed, valid_routes(&routes[..6]).unwrap());
}

/// 値の範囲はアダプタで弾かない（②が検証する）。形の不正はその位置に置く。
#[tokio::test]
async fn malformed_routes_are_placed_at_their_position_and_values_are_not_checked() {
    let body = json!({"code": "Ok", "routes": [
        {"duration": -5, "distance": 1e9, "geometry": {"type": "LineString", "coordinates": [[200, 95]]}},
        {"duration": 1, "distance": 2, "geometry": {"type": "LineString", "coordinates": [[139, 35, 10], [139.1, 35]]}},
    ]});
    let upstream = FakeUpstream::fixed(Reply::json(&body)).await;
    let parsed = router(&upstream)
        .route(&request(Mode::Walk, None), &CancelToken::unbounded())
        .await
        .unwrap();
    assert_eq!(parsed.len(), 2);
    let first = parsed[0].as_ref().unwrap();
    assert_eq!(first.duration_s, -5.0);
    assert_eq!(first.coordinates.len(), 1);
    assert_eq!(parsed[1], Err(MalformedRoute));

    for malformed in [
        json!({"duration": 1, "distance": 2, "geometry": {"type": "Point", "coordinates": [139, 35]}}),
        json!({"duration": 1, "distance": 2, "geometry": {"type": "LineString", "coordinates": [["139", 35]]}}),
        json!({"duration": "1", "distance": 2, "geometry": {"type": "LineString", "coordinates": []}}),
        json!({"duration": 1, "geometry": {"type": "LineString", "coordinates": []}}),
        json!("route"),
    ] {
        let upstream =
            FakeUpstream::fixed(Reply::json(&json!({"code": "Ok", "routes": [malformed]}))).await;
        let parsed = router(&upstream)
            .route(&request(Mode::Walk, None), &CancelToken::unbounded())
            .await
            .unwrap();
        assert_eq!(parsed, vec![Err(MalformedRoute)]);
    }
}

#[tokio::test]
async fn provider_failures_map_to_routing_errors() {
    let cases: Vec<(Reply, RoutingError)> = vec![
        (Reply::bytes(429, b"{}".to_vec()), RoutingError::ExternalApi),
        (Reply::bytes(500, b"{}".to_vec()), RoutingError::ExternalApi),
        (Reply::bytes(302, Vec::new()), RoutingError::ExternalApi),
        (
            Reply::bytes(200, b"{not json".to_vec()),
            RoutingError::ExternalApi,
        ),
        (
            Reply::json(&json!({"code": "Ok", "routes": {}})),
            RoutingError::ExternalApi,
        ),
        (
            Reply::json(&json!({"code": "Ok"})),
            RoutingError::ExternalApi,
        ),
        (
            Reply::json(&json!({"code": "NoRoute", "routes": []})),
            RoutingError::NoRoute,
        ),
        (
            Reply::json(&json!({"code": "Ok", "routes": []})),
            RoutingError::NoRoute,
        ),
        (Reply::json(&json!([1, 2])), RoutingError::NoRoute),
    ];
    for (reply, expected) in cases {
        let status = reply.status;
        let upstream = FakeUpstream::fixed(reply).await;
        let error = route(&router(&upstream), &CancelToken::unbounded())
            .await
            .unwrap_err();
        assert_eq!(error, expected, "status {status}");
        // 再試行しない
        assert_eq!(upstream.requests().len(), 1);
        // エラーにURL・座標を含めない
        assert!(!format!("{error:?}").contains("139."));
    }
}

#[tokio::test]
async fn oversized_responses_are_external_api_errors() {
    let body = serde_json::to_vec(&ok_body()).unwrap();
    for reply in [
        Reply::bytes(200, body.clone()),
        Reply::bytes(200, body.clone()).chunked(),
    ] {
        let upstream = FakeUpstream::fixed(reply).await;
        let router = router_with(&upstream, Duration::from_secs(8), 100, open_policy(16));
        assert_eq!(
            route(&router, &CancelToken::unbounded()).await,
            Err(RoutingError::ExternalApi)
        );
    }
    // ちょうど上限は読める
    let upstream = FakeUpstream::fixed(Reply::bytes(200, body.clone())).await;
    let router = router_with(
        &upstream,
        Duration::from_secs(8),
        body.len(),
        open_policy(16),
    );
    assert_eq!(route(&router, &CancelToken::unbounded()).await, Ok(1));
}

/// 1通信の期限（`UPSTREAM_TIMEOUT_MS` を縮めて検証）は、遅い分割応答を含めて全体で打ち切る。
#[tokio::test]
async fn upstream_timeout_bounds_the_whole_exchange() {
    let slow_head =
        FakeUpstream::fixed(Reply::json(&ok_body()).delayed(Duration::from_millis(800))).await;
    let router = router_with(
        &slow_head,
        Duration::from_millis(200),
        MAX_BYTES,
        open_policy(16),
    );
    let started = std::time::Instant::now();
    assert_eq!(
        route(&router, &CancelToken::unbounded()).await,
        Err(RoutingError::ExternalApi)
    );
    assert!(started.elapsed() < Duration::from_millis(700));

    let trickle =
        FakeUpstream::fixed(Reply::json(&ok_body()).trickle(Duration::from_millis(50))).await;
    let router = router_with(
        &trickle,
        Duration::from_millis(300),
        MAX_BYTES,
        open_policy(16),
    );
    assert_eq!(
        route(&router, &CancelToken::unbounded()).await,
        Err(RoutingError::ExternalApi)
    );
}

/// 要求全体の残りが短いときはそちらで打ち切り、`ExternalApi` ではなく中断として返す。
#[tokio::test]
async fn request_deadline_is_reported_as_interrupted() {
    let upstream =
        FakeUpstream::fixed(Reply::json(&ok_body()).delayed(Duration::from_millis(800))).await;
    let router = router(&upstream);
    let started = std::time::Instant::now();
    assert_eq!(
        route(
            &router,
            &CancelToken::with_timeout(Duration::from_millis(150))
        )
        .await,
        Err(RoutingError::Interrupted(Interrupt::DeadlineExceeded))
    );
    assert!(started.elapsed() < Duration::from_millis(600));
}

#[tokio::test]
async fn cancelled_or_busy_requests_are_not_sent() {
    let upstream = FakeUpstream::fixed(Reply::json(&ok_body())).await;
    let cancel = CancelToken::unbounded();
    cancel.cancel();
    assert_eq!(
        route(&router(&upstream), &cancel).await,
        Err(RoutingError::Interrupted(Interrupt::Cancelled))
    );
    // 待ち行列0（満杯）はゲートのBusy
    let busy = router_with(&upstream, Duration::from_secs(8), MAX_BYTES, open_policy(0));
    assert_eq!(
        route(&busy, &CancelToken::unbounded()).await,
        Err(RoutingError::Busy)
    );
    assert!(upstream.requests().is_empty());
}

/// 取消しで通信中の待ちも止まる。
#[tokio::test]
async fn cancellation_during_the_exchange_stops_waiting() {
    let upstream =
        FakeUpstream::fixed(Reply::json(&ok_body()).delayed(Duration::from_secs(5))).await;
    let router = Arc::new(router(&upstream));
    let cancel = CancelToken::unbounded();
    let task = {
        let router = router.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move { route(&router, &cancel).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = std::time::Instant::now();
    cancel.cancel();
    assert_eq!(
        task.await.unwrap(),
        Err(RoutingError::Interrupted(Interrupt::Cancelled))
    );
    assert!(started.elapsed() < Duration::from_millis(200));
}
