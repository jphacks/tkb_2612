//! S9：`GET /api/geocode` の入力検証・応答・期限。

mod common;

use std::sync::Arc;
use std::time::Duration;

use accident_route_backend::contract::{BoxFuture, CancelToken, LatLon};
use accident_route_backend::providers::gate::{Gate, GatePolicy, LocalGateStore};
use accident_route_backend::providers::geoapify::{
    GeoapifyGeocoder, GeocodeError, Geocoder, Place,
};
use accident_route_backend::providers::http::UpstreamClient;
use axum::http::StatusCode;
use serde_json::json;

use common::{FakeGeocoder, FakeUpstream, ManualClock, Reply, TestAppBuilder};

fn encode(q: &str) -> String {
    form_urlencoded::byte_serialize(q.as_bytes()).collect()
}

fn app_with(geocoder: Arc<FakeGeocoder>) -> common::TestApp {
    TestAppBuilder {
        geocoder,
        engine: None,
        ..Default::default()
    }
    .build()
}

/// `test_geocode_cache_and_japanese_query` 相当：実アダプタと偽Geoapifyで、
/// trimした日本語クエリが送られ、キャッシュせずに毎回通信する。
#[tokio::test]
async fn japanese_query_is_trimmed_and_not_cached() {
    let upstream = FakeUpstream::fixed(Reply::json(&json!({"results": [
        {"lat": 35.681_236_8, "lon": 139.767_125, "formatted": "東京駅, 丸の内一丁目, 千代田区, 東京都, 日本"}
    ]})))
    .await;
    let geocoder = GeoapifyGeocoder::new(
        upstream.base_url(),
        "dummy-key".to_owned(),
        UpstreamClient::new("accident-route-test/0.1").unwrap(),
        Gate::new(
            "geoapify",
            GatePolicy {
                min_interval: Duration::ZERO,
                max_concurrent: 1,
                queue_capacity: 16,
                lease_ttl: Duration::from_secs(10),
                quotas: Vec::new(),
            },
            Arc::new(LocalGateStore::new()),
            ManualClock::new(common::fixed_now()),
        ),
        Duration::from_secs(8),
        1_000_000,
    );
    let app = TestAppBuilder {
        geocoder: Arc::new(geocoder),
        engine: None,
        ..Default::default()
    }
    .build();
    let target = format!("/api/geocode?q={}", encode("　 東京駅 \t"));
    for _ in 0..2 {
        let response = app.get(&target).await;
        assert_eq!(response.status, StatusCode::OK);
        common::assert_api_headers(&response);
        assert_eq!(
            response.json(),
            json!({
                "results": [{"lat": 35.681_236_8, "lon": 139.767_125, "label": "東京駅, 丸の内一丁目, 千代田区, 東京都, 日本"}],
                "attribution": [
                    {"text": "Powered by Geoapify", "url": "https://www.geoapify.com/"},
                    {"text": "© OpenStreetMap contributors", "url": "https://www.openstreetmap.org/copyright"},
                ],
            })
        );
    }
    let requests = upstream.requests();
    assert_eq!(requests.len(), 2);
    for r in requests {
        let text = form_urlencoded::parse(r.query.as_deref().unwrap().as_bytes())
            .find(|(k, _)| k == "text")
            .unwrap()
            .1
            .into_owned();
        assert_eq!(text, "東京駅");
    }
}

#[tokio::test]
async fn q_length_is_checked_in_unicode_characters_after_trim() {
    let ok = |q: String| (q, true);
    let ng = |q: String| (q, false);
    let cases = [
        ng("東".to_owned()),
        ok("東京".to_owned()),
        ok("駅".repeat(200)),
        ng("駅".repeat(201)),
        ok(format!("  {}  ", "a".repeat(200))),
        ng(" a ".to_owned()),
        ng("   ".to_owned()),
        ng(String::new()),
    ];
    for (q, valid) in cases {
        let geocoder = FakeGeocoder::new(if valid { vec![Ok(Vec::new())] } else { vec![] });
        let app = app_with(geocoder.clone());
        let response = app.get(&format!("/api/geocode?q={}", encode(&q))).await;
        if valid {
            assert_eq!(
                response.status,
                StatusCode::OK,
                "{} 文字",
                q.chars().count()
            );
            assert_eq!(geocoder.calls(), vec![q.trim().to_owned()]);
        } else {
            assert_eq!(response.status, StatusCode::BAD_REQUEST, "{q:?}");
            assert_eq!(response.error_code(), "invalid_request");
            assert!(geocoder.calls().is_empty(), "外部呼出し前に検証する");
            assert!(
                !response.body.windows(3).any(|w| w == "駅".as_bytes()),
                "入力を返さない"
            );
        }
    }
}

#[tokio::test]
async fn q_must_appear_exactly_once() {
    for target in [
        "/api/geocode",
        "/api/geocode?",
        "/api/geocode?query=%E6%9D%B1%E4%BA%AC",
        "/api/geocode?q=%E6%9D%B1%E4%BA%AC&q=%E6%9D%B1%E4%BA%AC",
        "/api/geocode?q",
    ] {
        let app = app_with(FakeGeocoder::new([]));
        let response = app.get(target).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{target}");
        assert_eq!(response.error_code(), "invalid_request");
    }
    // 未知のキーは無視する
    let geocoder = FakeGeocoder::new([Ok(Vec::new())]);
    let response = app_with(geocoder.clone())
        .get("/api/geocode?lang=en&q=%E6%9D%B1%E4%BA%AC")
        .await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(geocoder.calls(), ["東京"]);
}

#[tokio::test]
async fn results_keep_provider_order_and_empty_results_are_200() {
    let geocoder = FakeGeocoder::new([
        Ok(Vec::new()),
        Ok(vec![
            common::place(35.7, 139.7, "b"),
            common::place(35.1, 139.1, "a"),
        ]),
    ]);
    let app = app_with(geocoder);
    let empty = app.get("/api/geocode?q=%E3%81%AA%E3%81%97").await;
    assert_eq!(empty.status, StatusCode::OK);
    assert_eq!(empty.json()["results"], json!([]));
    assert_eq!(empty.json()["attribution"].as_array().unwrap().len(), 2);
    let two = app.get("/api/geocode?q=%E6%9D%B1%E4%BA%AC").await;
    let labels: Vec<_> = two.json()["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["label"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(labels, ["b", "a"]);
}

#[tokio::test]
async fn provider_failures_map_to_502_and_503() {
    for (error, status, code) in [
        (GeocodeError::ExternalApi, 502, "external_api_error"),
        (GeocodeError::Busy, 503, "service_busy"),
        (
            GeocodeError::Interrupted(
                accident_route_backend::contract::Interrupt::DeadlineExceeded,
            ),
            503,
            "service_busy",
        ),
    ] {
        let response = app_with(FakeGeocoder::new([Err(error)]))
            .get("/api/geocode?q=%E6%9D%B1%E4%BA%AC")
            .await;
        assert_eq!(response.status.as_u16(), status, "{error:?}");
        assert_eq!(response.error_code(), code);
        common::assert_api_headers(&response);
    }
}

/// 受付からの全体期限（`REQUEST_TIMEOUT_MS` を縮めて検証）で503。
#[tokio::test]
async fn request_deadline_is_503() {
    let mut builder = TestAppBuilder {
        geocoder: FakeGeocoder::stalling(),
        ..Default::default()
    };
    builder.settings.request_timeout = Duration::from_millis(150);
    let app = builder.build();
    let started = std::time::Instant::now();
    let response = app.get("/api/geocode?q=%E6%9D%B1%E4%BA%AC").await;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.error_code(), "service_busy");
    assert!(started.elapsed() < Duration::from_secs(1));
}

/// 事故データが未readyでもgeocodeは使える。
#[tokio::test]
async fn geocode_does_not_wait_for_accident_data() {
    let response = app_with(FakeGeocoder::new([Ok(Vec::new())]))
        .get("/api/geocode?q=%E6%9D%B1%E4%BA%AC")
        .await;
    assert_eq!(response.status, StatusCode::OK);
}

/// 期限の直前に検索結果を返す偽の住所検索。表示名は大きく、JSONの準備に時間がかかる。
struct ReturnsJustBeforeDeadline;

impl Geocoder for ReturnsJustBeforeDeadline {
    fn search<'a>(
        &'a self,
        _q: &'a str,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<Vec<Place>, GeocodeError>> {
        // 大きな結果は先に作り、期限の直前まで待ってから返す
        let label = "\"".repeat(10_000_000);
        let places: Vec<Place> = (0..5)
            .map(|_| Place {
                position: LatLon::new(35.681, 139.767),
                label: label.clone(),
            })
            .collect();
        Box::pin(async move {
            while cancel
                .remaining()
                .is_some_and(|r| r > Duration::from_millis(1))
            {
                tokio::time::sleep(Duration::from_micros(200)).await;
            }
            Ok(places)
        })
    }
}

/// JSONの準備までが期限（35秒）の対象。期限直前に結果を得ても、準備中に期限が過ぎたら503。
#[tokio::test]
async fn deadline_covers_json_preparation() {
    let mut builder = TestAppBuilder {
        geocoder: Arc::new(ReturnsJustBeforeDeadline),
        engine: None,
        ..Default::default()
    };
    builder.settings.request_timeout = Duration::from_millis(200);
    let response = builder.build().get("/api/geocode?q=abc").await;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.error_code(), "service_busy");
}
