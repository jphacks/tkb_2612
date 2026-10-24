//! S9：Geoapifyアダプタ。ループバックの偽Geoapifyだけを相手にする（実通信しない）。

mod common;

use std::sync::Arc;
use std::time::Duration;

use accident_route_backend::contract::{CancelToken, Interrupt};
use accident_route_backend::providers::gate::{Gate, GatePolicy, LocalGateStore};
use accident_route_backend::providers::geoapify::{GeoapifyGeocoder, GeocodeError, Geocoder};
use accident_route_backend::providers::http::UpstreamClient;
use serde_json::{Value, json};

use common::{FakeUpstream, ManualClock, Reply};

const UA: &str = "accident-route-test/0.1 (+https://example.invalid/contact)";
const KEY: &str = "test-secret-key-0123";

fn geocoder_with(
    upstream: &FakeUpstream,
    timeout: Duration,
    queue_capacity: usize,
) -> GeoapifyGeocoder {
    GeoapifyGeocoder::new(
        upstream.base_url(),
        KEY.to_owned(),
        UpstreamClient::new(UA).unwrap(),
        Gate::new(
            "geoapify",
            GatePolicy {
                min_interval: Duration::ZERO,
                max_concurrent: 1,
                queue_capacity,
                lease_ttl: Duration::from_secs(10),
                quotas: Vec::new(),
            },
            Arc::new(LocalGateStore::new()),
            ManualClock::new(common::fixed_now()),
        ),
        timeout,
        1_000_000,
    )
}

fn geocoder(upstream: &FakeUpstream) -> GeoapifyGeocoder {
    geocoder_with(upstream, Duration::from_secs(8), 16)
}

fn result(lat: Value, lon: Value, formatted: Value) -> Value {
    json!({"lat": lat, "lon": lon, "formatted": formatted, "result_type": "amenity"})
}

fn results(n: usize) -> Value {
    json!({"results": (0..n)
        .map(|i| result(json!(35.0 + i as f64 * 0.01), json!(139.5), json!(format!("候補{i}"))))
        .collect::<Vec<_>>()})
}

fn pairs(query: &str) -> Vec<(String, String)> {
    form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

#[tokio::test]
async fn request_url_parameters_and_headers_follow_the_spec() {
    let upstream = FakeUpstream::fixed(Reply::json(&results(1))).await;
    let geocoder = geocoder(&upstream);
    geocoder
        .search("東京都千代田区丸の内 1-9-1&x=y", &CancelToken::unbounded())
        .await
        .unwrap();
    let requests = upstream.requests();
    assert_eq!(requests.len(), 1);
    let r = &requests[0];
    assert_eq!(r.method, "GET");
    assert_eq!(r.path, "/v1/geocode/search");
    assert_eq!(
        pairs(r.query.as_deref().unwrap()),
        [
            ("text", "東京都千代田区丸の内 1-9-1&x=y"),
            ("lang", "ja"),
            ("filter", "countrycode:jp"),
            ("limit", "5"),
            ("format", "json"),
            ("apiKey", KEY),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
    );
    assert_eq!(r.headers["user-agent"], UA);
    assert_eq!(r.headers["accept"], "application/json");
}

/// キャッシュしない：同じqでも毎回通信する。
#[tokio::test]
async fn same_query_is_sent_every_time() {
    let upstream = FakeUpstream::fixed(Reply::json(&results(2))).await;
    let geocoder = geocoder(&upstream);
    for _ in 0..2 {
        geocoder
            .search("東京駅", &CancelToken::unbounded())
            .await
            .unwrap();
    }
    assert_eq!(upstream.requests().len(), 2);
}

#[tokio::test]
async fn results_are_mapped_in_provider_order_up_to_five() {
    for (n, expected) in [(0, 0), (1, 1), (5, 5), (7, 5)] {
        let upstream = FakeUpstream::fixed(Reply::json(&results(n))).await;
        let places = geocoder(&upstream)
            .search("東京駅", &CancelToken::unbounded())
            .await
            .unwrap();
        assert_eq!(places.len(), expected);
        for (i, place) in places.iter().enumerate() {
            assert_eq!(place.label, format!("候補{i}"));
            assert_eq!(place.position.lat, 35.0 + i as f64 * 0.01);
            assert_eq!(place.position.lon, 139.5);
        }
    }
}

#[tokio::test]
async fn invalid_or_out_of_japan_results_are_external_api_errors() {
    let bad_results = [
        result(json!(10.0), json!(139.5), json!("矩形外")),
        result(json!(35.0), json!(160.0), json!("矩形外")),
        result(json!("35.0"), json!(139.5), json!("型不正")),
        result(json!(35.0), json!(139.5), json!(null)),
        json!({"lat": 35.0, "formatted": "lon欠落"}),
    ];
    for bad in bad_results {
        let body = json!({"results": [result(json!(35.0), json!(139.5), json!("正常")), bad]});
        let upstream = FakeUpstream::fixed(Reply::json(&body)).await;
        assert_eq!(
            geocoder(&upstream)
                .search("東京駅", &CancelToken::unbounded())
                .await,
            Err(GeocodeError::ExternalApi),
            "{body}"
        );
    }
    // 6件目以降は見ない
    let mut body = results(5);
    body["results"]
        .as_array_mut()
        .unwrap()
        .push(result(json!(0), json!(0), json!("6件目")));
    let upstream = FakeUpstream::fixed(Reply::json(&body)).await;
    assert_eq!(
        geocoder(&upstream)
            .search("東京駅", &CancelToken::unbounded())
            .await
            .unwrap()
            .len(),
        5
    );
}

#[tokio::test]
async fn http_and_format_failures_are_external_api_errors() {
    for reply in [
        Reply::bytes(429, b"{}".to_vec()),
        Reply::bytes(401, b"{}".to_vec()),
        Reply::bytes(500, b"{}".to_vec()),
        Reply::bytes(200, b"<html>".to_vec()),
        Reply::json(&json!({"features": []})),
        Reply::bytes(200, vec![b' '; 1_000_001]),
    ] {
        let upstream = FakeUpstream::fixed(reply).await;
        let error = geocoder(&upstream)
            .search("東京駅", &CancelToken::unbounded())
            .await
            .unwrap_err();
        assert_eq!(error, GeocodeError::ExternalApi);
        assert_eq!(upstream.requests().len(), 1, "再試行しない");
    }
}

#[tokio::test]
async fn timeouts_and_gate_failures() {
    let slow =
        FakeUpstream::fixed(Reply::json(&results(1)).delayed(Duration::from_millis(800))).await;
    assert_eq!(
        geocoder_with(&slow, Duration::from_millis(150), 16)
            .search("東京駅", &CancelToken::unbounded())
            .await,
        Err(GeocodeError::ExternalApi)
    );
    assert_eq!(
        geocoder(&slow)
            .search(
                "東京駅",
                &CancelToken::with_timeout(Duration::from_millis(150))
            )
            .await,
        Err(GeocodeError::Interrupted(Interrupt::DeadlineExceeded))
    );
    let upstream = FakeUpstream::fixed(Reply::json(&results(1))).await;
    assert_eq!(
        geocoder_with(&upstream, Duration::from_secs(8), 0)
            .search("東京駅", &CancelToken::unbounded())
            .await,
        Err(GeocodeError::Busy)
    );
    assert!(upstream.requests().is_empty());
}

/// 鍵をDebug表示・エラーに出さない。
#[tokio::test]
async fn api_key_is_not_exposed() {
    let upstream = FakeUpstream::fixed(Reply::bytes(500, Vec::new())).await;
    let geocoder = geocoder(&upstream);
    assert!(!format!("{geocoder:?}").contains(KEY));
    let error = geocoder
        .search("東京駅", &CancelToken::unbounded())
        .await
        .unwrap_err();
    assert!(!format!("{error:?}").contains(KEY));
}
