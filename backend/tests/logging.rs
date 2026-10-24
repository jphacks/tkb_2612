//! S14：ログ制限。ログ全体をメモリに捕捉し、特徴的な検索語・座標・鍵・上流URLを含む
//! geocode・conditions・route要求を、成功・失敗（上流失敗、400、期限切れ）とも実サーバへ送って、
//! どれもログに出ないことを確かめる。
//!
//! ログの購読はプロセスで1回しか設定できないので、このファイルのテストは1つにまとめる。

mod common;

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use accident_route_backend::contract::RouteEngine;
use accident_route_backend::providers::gate::{Gate, GatePolicy, LocalGateStore};
use accident_route_backend::providers::geoapify::GeoapifyGeocoder;
use accident_route_backend::providers::http::UpstreamClient;
use accident_route_backend::providers::open_meteo::OpenMeteoProvider;
use accident_route_backend::providers::osrm::{OsrmEndpoints, OsrmRouter};
use accident_route_backend::testing::fixtures::route_example;
use serde_json::json;

use common::{FakeUpstream, LiveServer, ManualClock, Recorded, Reply, TestAppBuilder};

const QUERY: &str = "秘密の検索語ZQX町";
const KEY: &str = "KEY-SECRET-0042";
const ORIGIN: (f64, f64) = (35.654_321_9, 139.765_432_1);
const DEST: (f64, f64) = (35.661_111_1, 139.772_222_2);

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

impl Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Capture {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

fn gate(key: &str) -> Gate {
    Gate::new(
        key,
        GatePolicy {
            min_interval: Duration::ZERO,
            max_concurrent: 1,
            queue_capacity: 16,
            lease_ttl: Duration::from_secs(10),
            quotas: Vec::new(),
        },
        Arc::new(LocalGateStore::new()),
        ManualClock::new(common::fixed_now()),
    )
}

/// 呼出し順に「成功→500→遅延」を繰り返す偽上流。
async fn cycling_upstream(ok: serde_json::Value) -> FakeUpstream {
    let calls = AtomicUsize::new(0);
    FakeUpstream::start(
        move |_: &Recorded| match calls.fetch_add(1, Ordering::SeqCst) % 3 {
            0 => Reply::json(&ok),
            1 => Reply::bytes(500, b"upstream failure".to_vec()),
            _ => Reply::json(&ok).delayed(Duration::from_secs(2)),
        },
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logs_never_contain_queries_coordinates_keys_or_upstream_urls() {
    let capture = Capture::default();
    let sink = capture.0.clone();
    accident_route_backend::logging::init_with_writer("trace", move || CaptureWriter(sink.clone()))
        .unwrap();

    let routes = &route_example("singleRoute").routes;
    let osrm = cycling_upstream(json!({
        "code": "Ok",
        "routes": routes.iter().map(|r| json!({
            "duration": r.duration_s,
            "distance": r.distance_m,
            "geometry": {"type": "LineString", "coordinates": r.coordinates.iter().map(|p| json!([p.lon, p.lat])).collect::<Vec<_>>()},
        })).collect::<Vec<_>>(),
    }))
    .await;
    let geoapify = cycling_upstream(json!({"results": [
        {"lat": ORIGIN.0, "lon": ORIGIN.1, "formatted": QUERY},
    ]}))
    .await;
    let open_meteo = cycling_upstream(json!({
        "utc_offset_seconds": 32400,
        "current": {"time": "2026-10-24T09:00", "weather_code": 3},
        "hourly": {"time": ["2026-10-24T09:00"], "weather_code": [3]},
    }))
    .await;

    let client = || UpstreamClient::new("accident-route-test/0.1").unwrap();
    let mut builder = TestAppBuilder {
        engine: Some(common::stub_engine("singleRoute") as Arc<dyn RouteEngine>),
        router: Arc::new(OsrmRouter::new(
            OsrmEndpoints {
                walk: osrm.base_url(),
                bike: osrm.base_url(),
                car: osrm.base_url(),
            },
            client(),
            gate("osrm"),
            Duration::from_secs(8),
            12_000_000,
        )),
        geocoder: Arc::new(GeoapifyGeocoder::new(
            geoapify.base_url(),
            KEY.to_owned(),
            client(),
            gate("geoapify"),
            Duration::from_secs(8),
            1_000_000,
        )),
        weather: Arc::new(OpenMeteoProvider::new(
            open_meteo.base_url(),
            Some(KEY.to_owned()),
            7,
            client(),
            gate("open-meteo"),
            Duration::from_secs(5),
            1_000_000,
        )),
        // 同じ地点でもキャッシュせず毎回上流へ行かせる
        weather_cache_ttl: Duration::ZERO,
        ..Default::default()
    };
    builder.settings.request_timeout = Duration::from_millis(500);
    builder.settings.conditions_timeout = Duration::from_millis(500);
    let app = builder.build();
    let server = LiveServer::start(app.router.clone()).await;
    let http = reqwest::Client::builder().no_proxy().build().unwrap();

    let encoded_q: String = form_urlencoded::byte_serialize(QUERY.as_bytes()).collect();
    let route_body = json!({
        "origin": {"lat": ORIGIN.0, "lon": ORIGIN.1},
        "dest": {"lat": DEST.0, "lon": DEST.1},
    });
    let mut statuses = Vec::new();
    // 上流は「成功→500→期限切れ」の順に応答する
    for _ in 0..3 {
        let r = http
            .get(server.url(&format!("/api/geocode?q={encoded_q}")))
            .send()
            .await
            .unwrap();
        statuses.push(("geocode", r.status().as_u16()));
        let r = http
            .get(server.url(&format!(
                "/api/conditions?datetime=2026-10-24T09:00%2B09:00&lat={}&lon={}",
                ORIGIN.0, ORIGIN.1
            )))
            .send()
            .await
            .unwrap();
        statuses.push(("conditions", r.status().as_u16()));
        let r = http
            .post(server.url("/api/route"))
            .header("content-type", "application/json")
            .header("referer", format!("https://example.invalid/?q={encoded_q}"))
            .body(route_body.to_string())
            .send()
            .await
            .unwrap();
        statuses.push(("route", r.status().as_u16()));
    }
    // 400と未知パス
    let bad_route = json!({"origin": {"lat": ORIGIN.0, "lon": ORIGIN.1}, "dest": {"lat": DEST.0, "lon": DEST.1}, "mode": QUERY});
    for request in [
        http.get(server.url(&format!("/api/geocode?q={encoded_q}&q={encoded_q}"))),
        http.get(server.url(&format!(
            "/api/conditions?datetime=2026-10-24T09:00Z&lat={}",
            ORIGIN.0
        ))),
        http.post(server.url("/api/route"))
            .header("content-type", "application/json")
            .body(bad_route.to_string()),
        http.get(server.url(&format!("/{encoded_q}/{}", ORIGIN.0))),
        http.get(server.url(&format!("/api/{encoded_q}"))),
    ] {
        let r = request.send().await.unwrap();
        statuses.push(("other", r.status().as_u16()));
    }
    assert_eq!(
        statuses,
        [
            ("geocode", 200),
            ("conditions", 200),
            ("route", 200),
            ("geocode", 502),
            ("conditions", 200),
            ("route", 502),
            ("geocode", 503),
            ("conditions", 200),
            ("route", 503),
            ("other", 400),
            ("other", 400),
            ("other", 400),
            ("other", 404),
            ("other", 404),
        ]
    );
    server.stop().await;
    // 期限切れ後の上流側の後始末もログに出ないことを含めて確かめる
    tokio::time::sleep(Duration::from_millis(100)).await;

    let logs = capture.text();
    // 捕捉できていること（アクセスログはルートのテンプレート・status・codeだけ）
    assert!(logs.contains("route=/api/geocode"), "{logs}");
    assert!(logs.contains("route=/api/route"), "{logs}");
    assert!(logs.contains("route=(unmatched)"), "{logs}");
    assert!(logs.contains("code=\"external_api_error\""), "{logs}");
    assert!(logs.contains("code=\"service_busy\""), "{logs}");
    assert!(logs.contains("code=\"invalid_request\""), "{logs}");
    let ports: Vec<String> = [&osrm, &geoapify, &open_meteo]
        .iter()
        .map(|u| u.addr.port().to_string())
        .collect();
    let mut forbidden: Vec<String> = vec![
        QUERY.to_owned(),
        "ZQX".to_owned(),
        encoded_q.clone(),
        "%E7%A7%98".to_owned(),
        KEY.to_owned(),
        "apiKey".to_owned(),
        "apikey".to_owned(),
        "35.65".to_owned(),
        "35.66".to_owned(),
        "139.76".to_owned(),
        "139.77".to_owned(),
        "6543219".to_owned(),
        "7654321".to_owned(),
        "route/v1".to_owned(),
        "geocode/search".to_owned(),
        "v1/forecast".to_owned(),
        "example.invalid".to_owned(),
        "upstream failure".to_owned(),
        "http://".to_owned(),
    ];
    forbidden.extend(ports.iter().map(|p| format!(":{p}")));
    for needle in &forbidden {
        assert!(
            !logs.contains(needle.as_str()),
            "ログに {needle:?} が出ている:\n{logs}"
        );
    }
    // 依存ライブラリ（HTTPサーバ・クライアント）のログは出さない
    for target in ["hyper", "reqwest", "h2", "rustls", "tower"] {
        assert!(
            !logs
                .lines()
                .any(|l| l.contains(&format!(" {target}::")) || l.contains(&format!(" {target}:"))),
            "{target} のログが出ている:\n{logs}"
        );
    }
}
