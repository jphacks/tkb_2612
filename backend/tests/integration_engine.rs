//! C3：②の実エンジン（人工release）と③の実ハンドラの結合（Issue #9 S19）。
//!
//! 人工releaseを `data::load_calendar` / `data::load_engine` で読み、例ごとの固定経路
//! （`ScriptedRouter`）と実ハンドラを組んで `POST /api/route` を呼び、応答が
//! `route-responses.json` の例と一致することを確かめる。同じ起動で `/api/health` と
//! `/api/conditions`（祝日表が実装に差し替わる）も確かめる。上流はループバックの偽物だけ。
//!
//! 比較規則（共通前提4章 C3 と同じ）：
//! - 応答の `dataset_id` は64桁の小文字hexで `engine.dataset().dataset_id` と等しいことを確かめてから、
//!   fixtureの人工値（0×64）に置き換える。`/api/health` では `release_id` も同様。
//! - `aggregation_ms` は存在し有限の非負数であることを確かめてから、値の比較から除く。
//! - 数値は数値として比べる（`0` と `0.0` を同一視）。
//! - 座標（経路・事故点・高リスク区間の補間座標・ナビの `projection_lat_deg`）は絶対誤差1e-10度以内、
//!   距離（ナビの `length_m`・`vertex_distances_m`・区間の `start_m`/`end_m`）は1e-6m以内。
//!   それ以外（丸め済みの値・件数・順序・ID・選定・文言）は完全一致。
//!   許容差は差の判定にだけ使い、応答の値を期待値へ書き換えない（許容差を超える差は必ず報告する）。
//!
//! 例の名前は固定せず、fixtureの全例を対象にする。

mod common;

use std::sync::Arc;
use std::time::Duration;

use accident_route_backend::contract::{Calendar, RouteEngine};
use accident_route_backend::data;
use accident_route_backend::providers::gate::{Gate, GatePolicy, LocalGateStore};
use accident_route_backend::providers::http::UpstreamClient;
use accident_route_backend::providers::open_meteo::OpenMeteoProvider;
use accident_route_backend::testing::fixtures::{
    route_example, route_response_json, route_response_names,
};
use accident_route_backend::testing::{TestRelease, example_router};
use axum::http::StatusCode;
use serde_json::{Value, json};

use common::{
    FakeUpstream, ManualClock, Reply, TestApp, TestAppBuilder, assert_api_headers,
    canonical_request, check_and_strip_aggregation_ms, json_diff,
};

const COORDINATE_TOLERANCE_DEG: f64 = 1e-10;
const DISTANCE_TOLERANCE_M: f64 = 1e-6;

/// 許容差の対象になるキー。値（配列なら全要素）にだけ許容差を使う。
fn tolerance_for(key: &str) -> Option<f64> {
    match key {
        "coordinates" | "projection_lat_deg" => Some(COORDINATE_TOLERANCE_DEG),
        "length_m" | "vertex_distances_m" | "start_m" | "end_m" => Some(DISTANCE_TOLERANCE_M),
        _ => None,
    }
}

/// 期待値との最初の差のパス。許容差が決まっているキーの数値だけ絶対誤差で比べ、
/// それ以外は `common::json_diff` と同じ完全一致。
fn diff_with_tolerance(actual: &Value, expected: &Value) -> Option<String> {
    fn walk(path: &str, a: &Value, e: &Value, tolerance: Option<f64>) -> Option<String> {
        match (a, e) {
            (Value::Array(xs), Value::Array(ys)) => {
                if xs.len() != ys.len() {
                    return Some(format!("{path}: 長さ {} != {}", xs.len(), ys.len()));
                }
                xs.iter()
                    .zip(ys)
                    .enumerate()
                    .find_map(|(i, (x, y))| walk(&format!("{path}[{i}]"), x, y, tolerance))
            }
            (Value::Object(xs), Value::Object(ys)) => {
                let mut keys_a: Vec<_> = xs.keys().collect();
                let mut keys_e: Vec<_> = ys.keys().collect();
                keys_a.sort();
                keys_e.sort();
                if keys_a != keys_e {
                    return Some(format!("{path}: キー {keys_a:?} != {keys_e:?}"));
                }
                xs.iter()
                    .find_map(|(k, x)| walk(&format!("{path}.{k}"), x, &ys[k], tolerance_for(k)))
            }
            (Value::Number(x), Value::Number(y)) if tolerance.is_some() => {
                let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
                let ok = (x - y).abs() <= tolerance.unwrap();
                (!ok).then(|| {
                    format!(
                        "{path}: {x} と {y} の差が許容差 {} を超える",
                        tolerance.unwrap()
                    )
                })
            }
            _ => json_diff(a, e).map(|d| d.replacen('$', path, 1)),
        }
    }
    walk("$", actual, expected, None)
}

fn is_lower_hex_64(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `field` が64桁の小文字hexで `expected_id` と等しいことを確かめ、fixtureの人工値（0×64）に置き換える。
fn replace_id(body: &mut Value, field: &str, expected_id: &str) {
    let id = body[field]
        .as_str()
        .unwrap_or_else(|| panic!("{field}が文字列でない"))
        .to_owned();
    assert!(is_lower_hex_64(&id), "{field}は64桁の小文字hex");
    assert_eq!(id, expected_id, "{field}が読み込んだreleaseと異なる");
    body[field] = Value::String("0".repeat(64));
}

/// 人工releaseを読んだ実エンジン・実Calendar・実ハンドラのアプリ。気象は偽Open-Meteo。
struct Started {
    app: TestApp,
    engine: Arc<dyn RouteEngine>,
    router: Arc<accident_route_backend::testing::ScriptedRouter>,
    _release: TestRelease,
    _weather: FakeUpstream,
}

async fn start(name: &str) -> Started {
    let release = TestRelease::for_example(name);
    // ③のstartup.rsと同じ順（祝日表、事故データ）で読む
    let calendar: Arc<dyn Calendar> =
        data::load_calendar(release.path()).expect("人工releaseの祝日表");
    let engine: Arc<dyn RouteEngine> = data::load_engine(release.path()).expect("人工release");
    assert_eq!(engine.dataset().dataset_id, release.ids.dataset_id);
    assert_eq!(engine.dataset().release_id, release.ids.release_id);

    let router = Arc::new(example_router(&route_example(name)));
    let weather = FakeUpstream::fixed(Reply::json(&json!({
        "utc_offset_seconds": 32400,
        "current": {"time": "2026-10-24T09:00", "weather_code": 0},
        "hourly": {"time": ["2026-10-24T09:00"], "weather_code": [0]},
    })))
    .await;
    let clock = ManualClock::new(common::fixed_now());
    let provider = OpenMeteoProvider::new(
        weather.base_url(),
        None,
        7,
        UpstreamClient::new("accident-route-test/0.1").unwrap(),
        Gate::new(
            "open-meteo",
            GatePolicy {
                min_interval: Duration::ZERO,
                max_concurrent: 1,
                queue_capacity: 16,
                lease_ttl: Duration::from_secs(10),
                quotas: Vec::new(),
            },
            Arc::new(LocalGateStore::new()),
            clock.clone(),
        ),
        Duration::from_secs(5),
        1_000_000,
    );
    let app = TestAppBuilder {
        engine: Some(engine.clone()),
        custom_calendar: Some(calendar),
        router: router.clone(),
        weather: Arc::new(provider),
        clock,
        ..Default::default()
    }
    .build();
    Started {
        app,
        engine,
        router,
        _release: release,
        _weather: weather,
    }
}

/// 例1件の `POST /api/route` 応答を、置換規則を適用してfixtureと比べる。
async fn check_route_example(name: &str) {
    let started = start(name).await;
    let response = started
        .app
        .post_json("/api/route", canonical_request(name).to_string())
        .await;
    assert_eq!(response.status, StatusCode::OK, "{name}");
    assert_api_headers(&response);
    assert_eq!(
        started.router.remaining(),
        0,
        "{name}: 用意した経路応答を使い切る"
    );

    let mut actual = response.json();
    replace_id(
        &mut actual,
        "dataset_id",
        &started.engine.dataset().dataset_id,
    );
    check_and_strip_aggregation_ms(&mut actual);
    let mut expected = route_response_json(name);
    expected.as_object_mut().unwrap().remove("aggregation_ms");
    if let Some(diff) = diff_with_tolerance(&actual, &expected) {
        panic!("{name}: 応答が一致しない: {diff}");
    }
}

#[tokio::test]
async fn every_fixture_example_matches_through_the_real_handler() {
    let names = route_response_names();
    assert!(!names.is_empty(), "fixtureに例がない");
    let mut checked = 0;
    for name in &names {
        check_route_example(name).await;
        checked += 1;
    }
    assert_eq!(
        checked,
        names.len(),
        "検査した例の数がfixtureの例の数と違う"
    );
}

#[tokio::test]
async fn health_reports_the_loaded_release() {
    let names = route_response_names();
    assert!(!names.is_empty());
    let mut checked = 0;
    for name in &names {
        let started = start(name).await;
        let response = started.app.get("/api/health").await;
        assert_eq!(response.status, StatusCode::OK, "{name}");
        assert_api_headers(&response);
        let mut actual = response.json();
        let dataset = started.engine.dataset();
        replace_id(&mut actual, "release_id", &dataset.release_id);
        replace_id(&mut actual, "dataset_id", &dataset.dataset_id);

        // 人工releaseは例の事故IDの事故だけを持つ（2025年のみ）
        assert_eq!(
            actual["accidents"],
            route_example(name).accident_ids.len(),
            "{name}"
        );
        assert_eq!(actual["data_years"], json!([2025]), "{name}");
        // 読込秒数は計測値：有限の非負数であることを確かめて、値の比較から除く
        let seconds = actual["load_seconds"].as_f64().expect("load_secondsが数値");
        assert!(seconds.is_finite() && seconds >= 0.0, "{name}: {seconds}");
        actual["load_seconds"] = json!(0);
        // grid_cellsは索引方式を採ったときだけ付く任意項目（docs/api/openapi.yaml）。
        // 実エンジンは付けるので、事故のあるセルの数として妥当（事故件数以下）か確かめて除く。
        let cells = actual
            .as_object_mut()
            .unwrap()
            .remove("grid_cells")
            .and_then(|v| v.as_u64())
            .unwrap_or_else(|| panic!("{name}: grid_cellsが非負整数でない"));
        assert!(cells <= dataset.accidents, "{name}: grid_cells={cells}");

        // 既存のhealth fixtureと比べる（件数は人工releaseの事故数に置き換える）
        let mut expected = health_fixture();
        expected["accidents"] = json!(route_example(name).accident_ids.len());
        expected["load_seconds"] = json!(0);
        if let Some(diff) = diff_with_tolerance(&actual, &expected) {
            panic!("{name}: healthがfixtureと一致しない: {diff}");
        }
        checked += 1;
    }
    assert_eq!(checked, names.len());
}

/// `api-responses.json` の `health.ready` の本文。
fn health_fixture() -> Value {
    let text = std::fs::read_to_string(
        accident_route_backend::testing::fixtures::backend_fixture_path("api-responses.json"),
    )
    .unwrap();
    let doc: Value = serde_json::from_str(&text).unwrap();
    doc["responses"]["health.ready"]["body"].clone()
}

async fn conditions(app: &TestApp, datetime: &str, rest: &str) -> Value {
    let datetime: String = form_urlencoded::byte_serialize(datetime.as_bytes()).collect();
    let target = if rest.is_empty() {
        format!("/api/conditions?datetime={datetime}")
    } else {
        format!("/api/conditions?datetime={datetime}&{rest}")
    };
    let response = app.get(&target).await;
    assert_eq!(response.status, StatusCode::OK, "{target}");
    assert_api_headers(&response);
    response.json()
}

/// 人工の祝日表の収録範囲は2025-01-01〜2025-12-31（`testing::release` の文書コメント）。
#[tokio::test]
async fn conditions_use_the_real_calendar_of_the_artificial_release() {
    let started = start("singleRoute").await;
    let app = &started.app;

    // 範囲内：平日、祝日、振替休日、土曜、日曜
    for (datetime, day_type, reason) in [
        ("2025-03-04T12:00+09:00", "平日", "平日"),
        ("2025-11-03T12:00+09:00", "休日", "祝日：文化の日"),
        ("2025-11-24T12:00+09:00", "休日", "振替休日"),
        ("2025-11-22T12:00+09:00", "休日", "土曜"),
        ("2025-11-23T12:00+09:00", "休日", "祝日：勤労感謝の日"),
        ("2025-11-02T12:00+09:00", "休日", "日曜"),
    ] {
        let body = conditions(app, datetime, "").await;
        assert_eq!(body["day_type"], day_type, "{datetime}");
        assert_eq!(body["day_type_reason"], reason, "{datetime}");
        assert_eq!(body["holiday_table_covered"], true, "{datetime}");
    }

    // 範囲外（収録は2025年だけ）：土日と年末年始だけで判定し、covered=false
    for (datetime, day_type, reason) in [
        ("2026-01-05T12:00+09:00", "平日", "平日"),
        ("2026-11-03T12:00+09:00", "平日", "平日"),
        ("2026-01-03T12:00+09:00", "休日", "年末年始"),
        ("2024-12-28T12:00+09:00", "休日", "土曜"),
    ] {
        let body = conditions(app, datetime, "").await;
        assert_eq!(body["day_type"], day_type, "{datetime}");
        assert_eq!(body["day_type_reason"], reason, "{datetime}");
        assert_eq!(body["holiday_table_covered"], false, "{datetime}");
    }

    // 範囲の境目
    for (datetime, covered) in [
        ("2025-01-01T12:00+09:00", true),
        ("2025-12-31T12:00+09:00", true),
        ("2024-12-31T12:00+09:00", false),
        ("2026-01-01T12:00+09:00", false),
    ] {
        let body = conditions(app, datetime, "").await;
        assert_eq!(body["holiday_table_covered"], covered, "{datetime}");
    }

    // 気象は偽Open-Meteo（ループバック）から取る
    let body = conditions(app, "2026-10-24T09:00+09:00", "lat=35.681&lon=139.767").await;
    assert_eq!(body["weather"], "晴");
    assert_eq!(body["weather_status"], "ok");
    assert_eq!(body["day_type_reason"], "土曜");
    assert_eq!(body["holiday_table_covered"], false);
    assert_eq!(started._weather.requests().len(), 1);
}

#[test]
fn tolerance_compare_accepts_small_and_rejects_large_differences() {
    // 数値は数値として比べる
    assert_eq!(
        diff_with_tolerance(&json!({"a": 0}), &json!({"a": 0.0})),
        None
    );
    // 座標は1e-10度以内
    let expected = json!({"geometry": {"coordinates": [[139.1, 35.2]]}});
    let near = json!({"geometry": {"coordinates": [[139.1 + 5e-11, 35.2]]}});
    let far = json!({"geometry": {"coordinates": [[139.1 + 2e-10, 35.2]]}});
    assert_eq!(diff_with_tolerance(&near, &expected), None);
    assert!(diff_with_tolerance(&far, &expected).is_some());
    // 距離は1e-6m以内
    let expected = json!({"navigation": {"length_m": 100.0, "vertex_distances_m": [0.0, 100.0]}});
    let near =
        json!({"navigation": {"length_m": 100.0 + 5e-7, "vertex_distances_m": [0.0, 100.0]}});
    let far = json!({"navigation": {"length_m": 100.0, "vertex_distances_m": [0.0, 100.0 + 2e-6]}});
    assert_eq!(diff_with_tolerance(&near, &expected), None);
    assert!(diff_with_tolerance(&far, &expected).is_some());
    // 許容差の対象でない値は完全一致
    assert!(
        diff_with_tolerance(
            &json!({"weighted_risk": 12.000001}),
            &json!({"weighted_risk": 12.0})
        )
        .is_some()
    );
    assert!(diff_with_tolerance(&json!({"distance": 1001}), &json!({"distance": 1000})).is_some());
}
