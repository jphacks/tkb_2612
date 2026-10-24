//! 実Axumハンドラの出力と公開API契約のfixtureとの整合（共通前提4章 C1・C2）。
//!
//! - C1：`/api/route` の実応答（gzip有無とも展開後）が、全例で `route_response_json(name)` と一致する。
//! - C2：fixtureにない応答（エラー各code・conditionsの各status・health・geocode）を実ハンドラから
//!   生成した `tests/fixtures/api-responses.json` と、実ハンドラの出力が一致する。
//!   再生成は `UPDATE_API_FIXTURES=1 cargo test --manifest-path backend/Cargo.toml --test api_contract`。

mod common;

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Arc;

use accident_route_backend::api::route_request::parse_route_request;
use accident_route_backend::contract::{
    DayClassification, DayTypeReason, EngineError, RouteEngine, RoutingRequest,
};
use accident_route_backend::providers::geoapify::GeocodeError;
use accident_route_backend::providers::open_meteo::{WeatherError, WeatherReport};
use accident_route_backend::testing::fixtures::{
    backend_fixture_path, route_example, route_response_json, route_response_names,
};
use accident_route_backend::testing::{ScriptedRouter, valid_routes};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use chrono::NaiveDateTime;
use serde_json::{Map, Value, json};

use common::{
    ErrorEngine, FailAt, FakeGeocoder, FakeWeather, RecordingEngine, TestAppBuilder, TestResponse,
};

// ---- C1 -----------------------------------------------------------------------

fn route_post(body: &Value, accept_encoding: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("/api/route")
        .header("content-type", "application/json");
    if let Some(value) = accept_encoding {
        builder = builder.header("accept-encoding", value);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn decoded_json(response: &TestResponse) -> Value {
    let bytes = match response.header("content-encoding") {
        Some("gzip") => {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(&response.body[..])
                .read_to_end(&mut out)
                .unwrap();
            out
        }
        None => response.body.to_vec(),
        Some(other) => panic!("想定外のContent-Encoding: {other}"),
    };
    serde_json::from_slice(&bytes).unwrap()
}

/// 例の名前を固定せず、`route-responses.json` の全例を回す。
#[tokio::test]
async fn c1_real_route_handler_matches_every_fixture_example() {
    let names = route_response_names();
    assert!(!names.is_empty());
    for name in &names {
        let example = route_example(name);
        let request_body = common::canonical_request(name);
        // 正規リクエストの解釈がfixtureのクエリと一致する（ハンドラが同じ値をエンジンへ渡す前提）
        assert_eq!(
            parse_route_request(request_body.to_string().as_bytes()).unwrap(),
            example.query,
            "{name}"
        );
        for accept in [None, Some("identity"), Some("gzip")] {
            let engine = RecordingEngine::new(common::stub_engine(name));
            let router = Arc::new(ScriptedRouter::new([valid_routes(&example.routes)]));
            let app = TestAppBuilder {
                engine: Some(engine.clone() as Arc<dyn RouteEngine>),
                router: router.clone(),
                ..Default::default()
            }
            .build();
            let response = app.send(route_post(&request_body, accept)).await;
            assert_eq!(response.status, StatusCode::OK, "{name} {accept:?}");
            common::assert_api_headers(&response);
            assert_eq!(
                response.header("content-encoding"),
                (accept == Some("gzip")).then_some("gzip"),
                "{name} {accept:?}"
            );

            let mut actual = decoded_json(&response);
            common::check_and_strip_aggregation_ms(&mut actual);
            let mut expected = route_response_json(name);
            expected.as_object_mut().unwrap().remove("aggregation_ms");
            if let Some(diff) = common::json_diff(&actual, &expected) {
                panic!("{name} {accept:?}: 実ハンドラの応答がfixtureと一致しない: {diff}");
            }

            assert_eq!(engine.queries(), vec![example.query.clone(); 2], "{name}");
            assert_eq!(
                router.calls(),
                vec![RoutingRequest {
                    origin: example.query.origin(),
                    dest: example.query.dest(),
                    mode: example.query.mode(),
                    via: None,
                }],
                "{name}"
            );
        }
    }
}

// ---- C2 -----------------------------------------------------------------------

const UPDATE_ENV: &str = "UPDATE_API_FIXTURES";
const REGENERATE: &str =
    "UPDATE_API_FIXTURES=1 cargo test --manifest-path backend/Cargo.toml --test api_contract";
const FIXED_NOW_JST: &str = "2026-10-24T09:00:00+09:00";

/// C2の1件：フロントの検証関数名、要求、組み立てるアプリ。
struct Case {
    name: &'static str,
    validator: &'static str,
    method: Method,
    target: &'static str,
    body: Option<Value>,
    app: TestAppBuilder,
}

fn get(name: &'static str, validator: &'static str, target: &'static str) -> Case {
    Case {
        name,
        validator,
        method: Method::GET,
        target,
        body: None,
        app: TestAppBuilder::default(),
    }
}

fn post_route(name: &'static str, body: Value) -> Case {
    Case {
        name,
        validator: "validateError",
        method: Method::POST,
        target: "/api/route",
        body: Some(body),
        app: TestAppBuilder::default(),
    }
}

impl Case {
    fn with(mut self, f: impl FnOnce(&mut TestAppBuilder)) -> Self {
        f(&mut self.app);
        self
    }
}

fn local(text: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M").unwrap()
}

/// 固定の現在時刻（2026-10-24 09:00 JST）に取得した気象。毎時予報は10/24 0時から8日分。
fn weather_report() -> WeatherReport {
    let start = local("2026-10-24T00:00");
    WeatherReport {
        current_time: local("2026-10-24T09:00"),
        current_code: 3,
        hourly: (0..24 * 8)
            .map(|h| {
                let time = start + chrono::Duration::hours(h);
                let code = if time == local("2026-10-26T14:00") {
                    61
                } else {
                    1
                };
                (time, code)
            })
            .collect(),
    }
}

fn day(reason: DayTypeReason, covered: bool) -> DayClassification {
    DayClassification {
        reason,
        holiday_table_covered: covered,
    }
}

fn error_engine(error: EngineError) -> Option<Arc<dyn RouteEngine>> {
    Some(ErrorEngine::new(error, FailAt::Collect))
}

/// C2の全件。名前の集合がC2の契約になる（①のテストが増減を検出する）。
fn cases() -> Vec<Case> {
    let saturday = || day(DayTypeReason::Saturday, true);
    vec![
        // ---- エラー各code ----
        post_route("error.invalidRequest.route", json!({})),
        get(
            "error.invalidRequest.geocode",
            "validateError",
            "/api/geocode?q=%20a%20",
        ),
        get(
            "error.invalidRequest.conditions",
            "validateError",
            "/api/conditions?datetime=2026-10-24T09:00",
        ),
        get("error.notFound.api", "validateError", "/api/unknown"),
        get(
            "error.methodNotAllowed.route",
            "validateError",
            "/api/route",
        ),
        Case {
            method: Method::POST,
            ..get(
                "error.methodNotAllowed.health",
                "validateError",
                "/api/health",
            )
        },
        post_route("error.routeTooLong.route", common::valid_route_body())
            .with(|b| b.engine = error_engine(EngineError::RouteTooLong)),
        post_route("error.noRoute.route", common::valid_route_body())
            .with(|b| b.engine = error_engine(EngineError::NoRoute)),
        post_route("error.externalApi.route", common::valid_route_body())
            .with(|b| b.engine = error_engine(EngineError::ExternalApi)),
        get(
            "error.externalApi.geocode",
            "validateError",
            "/api/geocode?q=%E6%9D%B1%E4%BA%AC%E9%A7%85",
        )
        .with(|b| b.geocoder = FakeGeocoder::new([Err(GeocodeError::ExternalApi)])),
        get(
            "error.dataUnavailable.health",
            "validateError",
            "/api/health",
        )
        .with(|b| b.engine = None),
        post_route("error.dataUnavailable.route", common::valid_route_body())
            .with(|b| b.engine = None),
        post_route("error.serviceBusy.route", common::valid_route_body())
            .with(|b| b.engine = error_engine(EngineError::Busy)),
        get(
            "error.serviceBusy.geocode",
            "validateError",
            "/api/geocode?q=%E6%9D%B1%E4%BA%AC%E9%A7%85",
        )
        .with(|b| b.geocoder = FakeGeocoder::new([Err(GeocodeError::Busy)])),
        post_route("error.internal.route", common::valid_route_body()).with(|b| {
            b.engine = error_engine(EngineError::ContractViolation("相関".into()));
        }),
        // ---- conditionsの各status ----
        get(
            "conditions.currentOk",
            "validateConditions",
            "/api/conditions?datetime=2026-10-24T09:05%2B09:00&lat=35.681&lon=139.767",
        )
        .with(|b| {
            b.calendar = saturday();
            b.weather = FakeWeather::new([Ok(weather_report())]);
        }),
        get(
            "conditions.forecastOk",
            "validateConditions",
            "/api/conditions?datetime=2026-10-26T05:30Z&lat=35.681&lon=139.767",
        )
        .with(|b| b.weather = FakeWeather::new([Ok(weather_report())])),
        get(
            "conditions.noLocation",
            "validateConditions",
            "/api/conditions?datetime=2026-10-24T18:00%2B09:00",
        )
        .with(|b| b.calendar = saturday()),
        get(
            "conditions.past",
            "validateConditions",
            "/api/conditions?datetime=2026-10-20T08:00%2B09:00&lat=35.681&lon=139.767",
        ),
        get(
            "conditions.outOfRange",
            "validateConditions",
            "/api/conditions?datetime=2026-11-03T14:30%2B09:00&lat=35.7&lon=139.77",
        )
        .with(|b| {
            b.calendar = day(
                DayTypeReason::NationalHoliday {
                    name: "文化の日".to_owned(),
                },
                true,
            );
        }),
        get(
            "conditions.unavailable",
            "validateConditions",
            "/api/conditions?datetime=2026-10-24T12:00%2B09:00&lat=35.681&lon=139.767",
        )
        .with(|b| {
            b.calendar = saturday();
            b.weather = FakeWeather::new([Err(WeatherError::Failed)]);
        }),
        get(
            "conditions.holidayTableNotCovered",
            "validateConditions",
            "/api/conditions?datetime=2099-11-03T12:00%2B09:00",
        )
        .with(|b| b.calendar = day(DayTypeReason::Weekday, false)),
        // ---- health ----
        get("health.ready", "validateHealth", "/api/health"),
        get("health.status", "validateHealth", "/api/status"),
        // ---- geocode ----
        get(
            "geocode.empty",
            "validatePlaces",
            "/api/geocode?q=%E5%AD%98%E5%9C%A8%E3%81%97%E3%81%AA%E3%81%84%E5%9C%B0%E5%90%8D",
        )
        .with(|b| b.geocoder = FakeGeocoder::new([Ok(Vec::new())])),
        get(
            "geocode.multiple",
            "validatePlaces",
            "/api/geocode?q=%E6%9D%B1%E4%BA%AC%E9%A7%85",
        )
        .with(|b| {
            b.geocoder = FakeGeocoder::new([Ok(vec![
                common::place(
                    35.681_236_8,
                    139.767_125_1,
                    "東京駅, 丸の内一丁目, 千代田区, 東京都, 日本",
                ),
                common::place(
                    35.680_0,
                    139.768_9,
                    "東京駅八重洲口, 八重洲一丁目, 中央区, 東京都, 日本",
                ),
                common::place(34.985_8, 135.758_8, "東京駅前バス停, 京都市, 京都府, 日本"),
            ])]);
        }),
    ]
}

async fn run_case(case: Case) -> (String, Value) {
    let app = case.app.build();
    let body = case
        .body
        .as_ref()
        .map_or_else(Body::empty, |b| Body::from(b.to_string()));
    let mut builder = Request::builder()
        .method(case.method.clone())
        .uri(case.target);
    if case.body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let response = app.send(builder.body(body).unwrap()).await;
    // content-lengthは本文から決まるので記録しない
    let headers: BTreeMap<String, String> = response
        .headers
        .iter()
        .filter(|(k, _)| k.as_str() != "content-length")
        .map(|(k, v)| (k.as_str().to_owned(), v.to_str().unwrap().to_owned()))
        .collect();
    let mut request = Map::new();
    request.insert("method".into(), json!(case.method.as_str()));
    request.insert("target".into(), json!(case.target));
    if let Some(body) = case.body {
        request.insert("body".into(), body);
    }
    (
        case.name.to_owned(),
        json!({
            "validator": case.validator,
            "request": request,
            "status": response.status.as_u16(),
            "headers": headers,
            "body": response.json(),
        }),
    )
}

async fn generate() -> Value {
    let mut responses = Map::new();
    for case in cases() {
        let (name, entry) = run_case(case).await;
        assert!(
            responses.insert(name.clone(), entry).is_none(),
            "C2の名前が重複: {name}"
        );
    }
    json!({
        "description": format!(
            "実Axumハンドラから生成（共通前提4章 C2）。手編集禁止。再生成: {REGENERATE}。\
             各responsesのvalidatorはフロントの検証関数名、requestのbodyはPOSTの要求本文（JSON）、\
             headersはcontent-lengthを除く応答ヘッダー（小文字名）、bodyは応答JSON本文。"
        ),
        "fixed_now": FIXED_NOW_JST,
        "responses": responses,
    })
}

fn render(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).unwrap();
    text.push('\n');
    text
}

#[tokio::test]
async fn c2_generated_responses_match_the_saved_fixture() {
    let generated = generate().await;
    let text = render(&generated);
    let path = backend_fixture_path("api-responses.json");
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::write(&path, &text).unwrap();
        return;
    }
    let saved = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} が読めない（{e}）。再生成: {REGENERATE}", path.display()));
    if saved != text {
        let detail = serde_json::from_str::<Value>(&saved)
            .ok()
            .and_then(|saved| common::json_diff(&generated, &saved))
            .unwrap_or_else(|| "書式の差（JSONとしての差は無い、または解析できない）".to_owned());
        panic!("api-responses.json が実ハンドラの出力と一致しない: {detail}\n再生成: {REGENERATE}");
    }
}

/// 生成内容がC2の約束（エラー9種類・conditionsの全status・health・geocode）を網羅する。
#[tokio::test]
async fn c2_covers_every_error_code_and_weather_status() {
    let generated = generate().await;
    let responses = generated["responses"].as_object().unwrap();
    let mut codes = Vec::new();
    let mut statuses = Vec::new();
    for (name, entry) in responses {
        let validator = entry["validator"].as_str().unwrap();
        assert!(
            [
                "validateConditions",
                "validateHealth",
                "validatePlaces",
                "validateError"
            ]
            .contains(&validator),
            "{name}"
        );
        let status = entry["status"].as_u64().unwrap();
        assert_eq!(
            entry["headers"]["content-type"], "application/json",
            "{name}"
        );
        assert_eq!(entry["headers"]["cache-control"], "no-store", "{name}");
        assert_eq!(
            entry["headers"]["x-content-type-options"], "nosniff",
            "{name}"
        );
        match validator {
            "validateError" => {
                assert!(status >= 400, "{name}");
                codes.push(entry["body"]["error"]["code"].as_str().unwrap().to_owned());
            }
            "validateConditions" => {
                assert_eq!(status, 200, "{name}");
                statuses.push(entry["body"]["weather_status"].as_str().unwrap().to_owned());
            }
            _ => assert_eq!(status, 200, "{name}"),
        }
    }
    codes.sort();
    codes.dedup();
    assert_eq!(
        codes,
        [
            "data_unavailable",
            "external_api_error",
            "internal_error",
            "invalid_request",
            "method_not_allowed",
            "no_route",
            "not_found",
            "route_too_long",
            "service_busy",
        ]
    );
    statuses.sort();
    statuses.dedup();
    assert_eq!(
        statuses,
        ["no_location", "ok", "out_of_range", "past", "unavailable"]
    );
    let kinds: Vec<_> = ["conditions.currentOk", "conditions.forecastOk"]
        .iter()
        .map(|n| responses[*n]["body"]["weather_source"]["kind"].clone())
        .collect();
    assert_eq!(kinds, [json!("current"), json!("forecast")]);
    assert_eq!(
        responses["conditions.holidayTableNotCovered"]["body"]["holiday_table_covered"],
        false
    );
    assert_eq!(responses["geocode.empty"]["body"]["results"], json!([]));
    assert_eq!(
        responses["geocode.multiple"]["body"]["results"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}
