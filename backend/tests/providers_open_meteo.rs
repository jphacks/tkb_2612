//! S10：Open-Meteoアダプタ。ループバックの偽Open-Meteoだけを相手にする（実通信しない）。

mod common;

use std::sync::Arc;
use std::time::Duration;

use accident_route_backend::contract::{CancelToken, Interrupt};
use accident_route_backend::providers::gate::{Gate, GatePolicy, LocalGateStore};
use accident_route_backend::providers::http::UpstreamClient;
use accident_route_backend::providers::open_meteo::{
    OpenMeteoProvider, RoundedPoint, WeatherError, WeatherProvider,
};
use chrono::NaiveDateTime;
use serde_json::{Value, json};

use common::{FakeUpstream, ManualClock, Reply};

const UA: &str = "accident-route-test/0.1 (+https://example.invalid/contact)";

fn provider_with(
    upstream: &FakeUpstream,
    api_key: Option<&str>,
    timeout: Duration,
    queue_capacity: usize,
) -> OpenMeteoProvider {
    OpenMeteoProvider::new(
        upstream.base_url(),
        api_key.map(str::to_owned),
        7,
        UpstreamClient::new(UA).unwrap(),
        Gate::new(
            "open-meteo",
            GatePolicy {
                min_interval: Duration::from_millis(200),
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

fn provider(upstream: &FakeUpstream) -> OpenMeteoProvider {
    provider_with(upstream, None, Duration::from_secs(5), 16)
}

fn body() -> Value {
    json!({
        "latitude": 35.68,
        "longitude": 139.76,
        "utc_offset_seconds": 32400,
        "timezone": "Asia/Tokyo",
        "current_units": {"time": "iso8601", "weather_code": "wmo code"},
        "current": {"time": "2026-10-24T09:00", "interval": 900, "weather_code": 3},
        "hourly": {
            "time": ["2026-10-24T09:00", "2026-10-24T10:00", "2026-10-24T11:00"],
            "weather_code": [3, 61, 71.0],
        },
    })
}

fn local(text: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M").unwrap()
}

fn point() -> RoundedPoint {
    RoundedPoint::round(35.678, 139.7649)
}

#[tokio::test]
async fn request_url_uses_rounded_point_and_fixed_parameters() {
    let upstream = FakeUpstream::fixed(Reply::json(&body())).await;
    provider(&upstream)
        .fetch(point(), &CancelToken::unbounded())
        .await
        .unwrap();
    provider_with(&upstream, Some("paid&key"), Duration::from_secs(5), 16)
        .fetch(
            RoundedPoint::round(-0.004, 122.0),
            &CancelToken::unbounded(),
        )
        .await
        .unwrap();
    let requests = upstream.requests();
    assert_eq!(requests[0].path, "/v1/forecast");
    assert_eq!(
        requests[0].query.as_deref(),
        Some(
            "latitude=35.68&longitude=139.76&current=weather_code&hourly=weather_code\
             &timezone=Asia%2FTokyo&forecast_days=7"
        )
    );
    assert_eq!(
        requests[1].query.as_deref(),
        Some(
            "latitude=0.00&longitude=122.00&current=weather_code&hourly=weather_code\
             &timezone=Asia%2FTokyo&forecast_days=7&apikey=paid%26key"
        )
    );
    for r in &requests {
        assert_eq!(r.headers["user-agent"], UA);
        assert_eq!(r.headers["accept"], "application/json");
    }
}

#[tokio::test]
async fn valid_response_is_parsed() {
    let upstream = FakeUpstream::fixed(Reply::json(&body())).await;
    let report = provider(&upstream)
        .fetch(point(), &CancelToken::unbounded())
        .await
        .unwrap();
    assert_eq!(report.current_time, local("2026-10-24T09:00"));
    assert_eq!(report.current_code, 3);
    assert_eq!(
        report.hourly,
        [
            (local("2026-10-24T09:00"), 3),
            (local("2026-10-24T10:00"), 61),
            (local("2026-10-24T11:00"), 71),
        ]
    );
}

type Mutation = Box<dyn Fn(&mut Value)>;

#[tokio::test]
async fn malformed_responses_are_failures() {
    let mutate: Vec<Mutation> = vec![
        Box::new(|b| b["utc_offset_seconds"] = json!(0)),
        Box::new(|b| b["utc_offset_seconds"] = json!("32400")),
        Box::new(|b| {
            b.as_object_mut().unwrap().remove("utc_offset_seconds");
        }),
        Box::new(|b| {
            b.as_object_mut().unwrap().remove("current");
        }),
        Box::new(|b| b["current"]["time"] = json!("2026/10/24 09:00")),
        Box::new(|b| b["current"]["weather_code"] = json!(null)),
        Box::new(|b| b["current"]["weather_code"] = json!(-1)),
        Box::new(|b| b["current"]["weather_code"] = json!(2.5)),
        Box::new(|b| b["hourly"]["weather_code"] = json!([3, 61])),
        Box::new(|b| b["hourly"]["time"] = json!("2026-10-24T09:00")),
        Box::new(|b| b["hourly"]["weather_code"][1] = json!("61")),
        Box::new(|b| {
            b.as_object_mut().unwrap().remove("hourly");
        }),
    ];
    for (i, change) in mutate.iter().enumerate() {
        let mut b = body();
        change(&mut b);
        let upstream = FakeUpstream::fixed(Reply::json(&b)).await;
        assert_eq!(
            provider(&upstream)
                .fetch(point(), &CancelToken::unbounded())
                .await,
            Err(WeatherError::Failed),
            "変更{i}"
        );
    }
}

#[tokio::test]
async fn http_size_and_timeout_failures() {
    let mut big = body();
    big["padding"] = json!("x".repeat(1_000_000));
    for reply in [
        Reply::bytes(429, b"{}".to_vec()),
        Reply::bytes(500, b"{}".to_vec()),
        Reply::bytes(200, b"not json".to_vec()),
        Reply::json(&big),
        Reply::json(&big).chunked(),
    ] {
        let upstream = FakeUpstream::fixed(reply).await;
        assert_eq!(
            provider(&upstream)
                .fetch(point(), &CancelToken::unbounded())
                .await,
            Err(WeatherError::Failed)
        );
        assert_eq!(upstream.requests().len(), 1, "再試行しない");
    }
    let slow = FakeUpstream::fixed(Reply::json(&body()).delayed(Duration::from_millis(800))).await;
    assert_eq!(
        provider_with(&slow, None, Duration::from_millis(150), 16)
            .fetch(point(), &CancelToken::unbounded())
            .await,
        Err(WeatherError::Failed)
    );
    assert_eq!(
        provider(&slow)
            .fetch(
                point(),
                &CancelToken::with_timeout(Duration::from_millis(150))
            )
            .await,
        Err(WeatherError::Interrupted(Interrupt::DeadlineExceeded))
    );
}

#[tokio::test]
async fn full_queue_is_busy_and_not_sent() {
    let upstream = FakeUpstream::fixed(Reply::json(&body())).await;
    assert_eq!(
        provider_with(&upstream, None, Duration::from_secs(5), 0)
            .fetch(point(), &CancelToken::unbounded())
            .await,
        Err(WeatherError::Busy)
    );
    assert!(upstream.requests().is_empty());
}

/// 鍵をDebug表示に出さない。
#[tokio::test]
async fn api_key_is_not_exposed() {
    let upstream = FakeUpstream::fixed(Reply::json(&body())).await;
    let provider = provider_with(
        &upstream,
        Some("paid-secret-1234"),
        Duration::from_secs(5),
        16,
    );
    assert!(!format!("{provider:?}").contains("paid-secret-1234"));
}
