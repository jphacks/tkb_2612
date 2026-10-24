//! S4：route入力の検証・正規化・互換入力（behavior.md 4章、11章 追加試験10）。
//!
//! 不正な入力はすべて外部呼出しの前に400になることを、偽ルーターが呼ばれないことで確かめる。

mod common;

use std::sync::Arc;

use accident_route_backend::api::route_request::parse_route_request;
use accident_route_backend::contract::geo::METERS_PER_DEGREE;
use accident_route_backend::contract::{
    ConditionFilter, DayType, LatLon, Mode, Period, RouteQuery, Weather,
};
use accident_route_backend::testing::fixtures::route_example;
use accident_route_backend::testing::{ScriptedRouter, valid_routes};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};

use common::{TestAppBuilder, TestResponse};

/// 1回分の正常な経路を用意したアプリに送る。戻り値は応答とルーターの残り応答数。
async fn send(content_type: Option<&str>, body: impl Into<Body>) -> (TestResponse, usize) {
    let router = Arc::new(ScriptedRouter::new([valid_routes(
        &route_example("singleRoute").routes,
    )]));
    let app = TestAppBuilder {
        router: router.clone(),
        ..Default::default()
    }
    .build();
    let mut builder = Request::builder().method(Method::POST).uri("/api/route");
    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    let response = app.send(builder.body(body.into()).unwrap()).await;
    (response, router.remaining())
}

async fn send_json(body: &str) -> (TestResponse, usize) {
    send(Some("application/json"), body.to_owned()).await
}

/// 400で、ルーターを呼んでいないこと。messageを返す。
async fn assert_rejected(body: &str) -> String {
    let (response, remaining) = send_json(body).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(response.error_code(), "invalid_request", "{body}");
    assert_eq!(remaining, 1, "不正な入力でルーターを呼んだ: {body}");
    response.json()["error"]["message"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn assert_accepted(body: &str) {
    let (response, remaining) = send_json(body).await;
    assert_eq!(
        response.status,
        StatusCode::OK,
        "{body}: {}",
        String::from_utf8_lossy(&response.body)
    );
    assert_eq!(remaining, 0);
}

/// 有効な要求に `extra` のキーを足す（nullのキーは削除）。
fn with(extra: Value) -> String {
    let mut body = common::valid_route_body();
    for (k, v) in extra.as_object().unwrap() {
        if v.is_null() && k.starts_with('-') {
            body.as_object_mut().unwrap().remove(&k[1..]);
        } else {
            body[k] = v.clone();
        }
    }
    body.to_string()
}

#[tokio::test]
async fn non_object_bodies_and_broken_json_are_rejected() {
    for body in [
        "[]",
        "null",
        "1",
        "\"x\"",
        "{",
        "{\"origin\":}",
        "\u{feff}{}",
    ] {
        assert_rejected(body).await;
    }
    // UTF-8でない本文
    let (response, remaining) = send(Some("application/json"), vec![0xff, 0xfe]).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(remaining, 1);
}

#[tokio::test]
async fn origin_and_dest_must_be_named_finite_numbers_in_japan() {
    for (origin, dest) in [
        (json!(null), json!({"lat": 35.69, "lon": 139.77})),
        (json!({"lat": 35.681}), json!({"lat": 35.69, "lon": 139.77})),
        (
            json!({"lat": "35.681", "lon": 139.767}),
            json!({"lat": 35.69, "lon": 139.77}),
        ),
        (
            json!({"lat": true, "lon": 139.767}),
            json!({"lat": 35.69, "lon": 139.77}),
        ),
        (
            json!([139.767, 35.681]),
            json!({"lat": 35.69, "lon": 139.77}),
        ),
        (json!({"lat": 35.681, "lon": 139.767}), json!(null)),
        (
            json!({"lat": 19.9999, "lon": 139.767}),
            json!({"lat": 35.69, "lon": 139.77}),
        ),
        (
            json!({"lat": 35.681, "lon": 154.0001}),
            json!({"lat": 35.69, "lon": 139.77}),
        ),
    ] {
        assert_rejected(&with(json!({"origin": origin, "dest": dest}))).await;
    }
    // 同一点
    let same = json!({"lat": 35.681, "lon": 139.767});
    assert_rejected(&with(json!({"origin": same, "dest": same}))).await;
}

#[tokio::test]
async fn japan_rectangle_edges_are_inclusive() {
    // 端の点そのものは受理する（距離は20m〜30kmに収める）
    for (origin, dest) in [
        (
            json!({"lat": 20.0, "lon": 136.0}),
            json!({"lat": 20.01, "lon": 136.0}),
        ),
        (
            json!({"lat": 46.0, "lon": 142.0}),
            json!({"lat": 45.99, "lon": 142.0}),
        ),
        (
            json!({"lat": 30.0, "lon": 122.0}),
            json!({"lat": 30.0, "lon": 122.01}),
        ),
        (
            json!({"lat": 30.0, "lon": 154.0}),
            json!({"lat": 30.0, "lon": 153.99}),
        ),
    ] {
        let body = json!({"origin": origin, "dest": dest}).to_string();
        let query = parse_route_request(body.as_bytes());
        assert!(query.is_ok(), "{body}: {query:?}");
    }
}

#[tokio::test]
async fn straight_distance_bounds() {
    let lat = 35.0;
    let point = |meters: f64| json!({"lat": lat + meters / METERS_PER_DEGREE, "lon": 139.0});
    let origin = json!({"lat": lat, "lon": 139.0});
    for meters in [19.9, 30_001.0] {
        let message =
            assert_rejected(&json!({"origin": origin, "dest": point(meters)}).to_string()).await;
        assert_eq!(
            message,
            "出発地と目的地の直線距離は20m〜30kmにしてください。"
        );
    }
    for meters in [20.1, 29_999.0] {
        let body = json!({"origin": origin, "dest": point(meters)}).to_string();
        assert!(parse_route_request(body.as_bytes()).is_ok(), "{meters}");
    }
}

#[tokio::test]
async fn mode_and_max_detour() {
    for extra in [
        json!({"mode": null}),
        json!({"mode": "train"}),
        json!({"mode": "Walk"}),
        json!({"mode": 1}),
        json!({"max_detour_min": null}),
        json!({"max_detour_min": -0.1}),
        json!({"max_detour_min": 30.0001}),
        json!({"max_detour_min": 31}),
        json!({"max_detour_min": "10"}),
        json!({"max_detour_min": true}),
    ] {
        assert_rejected(&with(extra)).await;
    }
    // NaNはJSONに書けないので、JSONとして不正な本文になる
    assert_rejected(&with(json!({})).replace('}', ",\"max_detour_min\":NaN}")).await;
    for value in [0.0, 30.0, 12.5] {
        let body = with(json!({"max_detour_min": value}));
        assert_eq!(
            parse_route_request(body.as_bytes())
                .unwrap()
                .max_detour_min(),
            value
        );
    }
}

fn conditions(body: &str) -> ConditionFilter {
    parse_route_request(body.as_bytes())
        .unwrap_or_else(|e| panic!("{body}: {e}"))
        .conditions()
}

#[tokio::test]
async fn defaults_are_walk_all_and_ten_minutes() {
    let query = parse_route_request(common::valid_route_body().to_string().as_bytes()).unwrap();
    assert_eq!(
        query,
        RouteQuery::new(
            LatLon::new(35.681, 139.767),
            LatLon::new(35.69, 139.77),
            Mode::Walk,
            10.0,
            ConditionFilter::default(),
        )
        .unwrap()
    );
}

#[tokio::test]
async fn period_labels_codes_aliases_and_hour() {
    let expect = |body: &str, period: Option<Period>| {
        assert_eq!(conditions(body).period, period, "{body}");
    };
    // 日本語・整数コード（新しい内部コード）
    for (label, code, period) in [
        ("早朝", 0, Period::EarlyMorning),
        ("昼", 1, Period::Daytime),
        ("夕", 2, Period::Evening),
        ("夜", 3, Period::Night),
        ("深夜", 4, Period::LateNight),
    ] {
        expect(&with(json!({"period": label})), Some(period));
        expect(&with(json!({"period": code})), Some(period));
        expect(&with(json!({"period": f64::from(code)})), Some(period));
        expect(&with(json!({"時間帯": label})), Some(period));
    }
    expect(&with(json!({"period": "all"})), None);
    expect(&with(json!({"period": null})), None);
    // 旧クライアントの3は19〜23時（0〜3時を含めない）
    expect(&with(json!({"period": 3, "hour": 21})), Some(Period::Night));
    assert_rejected(&with(json!({"period": 3, "hour": 2}))).await;
    // `period` キーが存在すれば `時間帯` より優先（nullでも優先）
    expect(
        &with(json!({"period": "昼", "時間帯": "夜"})),
        Some(Period::Daytime),
    );
    expect(&with(json!({"period": null, "時間帯": "夜"})), None);
    // hourで具体化
    for (hour, period) in [
        (0, Period::LateNight),
        (3, Period::LateNight),
        (4, Period::EarlyMorning),
        (12, Period::Daytime),
        (18, Period::Evening),
        (19, Period::Night),
        (23, Period::Night),
    ] {
        expect(&with(json!({"hour": hour})), Some(period));
        expect(&with(json!({"period": "all", "hour": hour})), Some(period));
        expect(&with(json!({"period": null, "hour": hour})), Some(period));
    }
    expect(&with(json!({"hour": 12.0})), Some(Period::Daytime));
    expect(
        &with(json!({"period": "昼", "hour": null})),
        Some(Period::Daytime),
    );
    for extra in [
        json!({"period": "朝"}),
        json!({"period": 5}),
        json!({"period": -1}),
        json!({"period": 1.5}),
        json!({"period": true}),
        json!({"period": "1"}),
        json!({"時間帯": "朝"}),
        json!({"hour": 24}),
        json!({"hour": -1}),
        json!({"hour": 12.1}),
        json!({"hour": true}),
        json!({"hour": "12"}),
        json!({"period": "夜", "hour": 2}),
        // hourがあっても先に不正periodを検査する
        json!({"period": "朝", "hour": 2}),
    ] {
        assert_rejected(&with(extra)).await;
    }
    let message = assert_rejected(&with(json!({"period": "朝", "hour": "x"}))).await;
    assert_eq!(
        message,
        "時間帯は早朝・昼・夕・夜・深夜・allのいずれかを指定してください。"
    );
}

#[tokio::test]
async fn weather_and_day_type() {
    for (label, code, weather) in [
        ("晴", 1, Weather::Clear),
        ("曇", 2, Weather::Cloudy),
        ("雨", 3, Weather::Rain),
        ("霧", 4, Weather::Fog),
        ("雪", 5, Weather::Snow),
    ] {
        assert_eq!(
            conditions(&with(json!({"weather": label}))).weather,
            Some(weather)
        );
        assert_eq!(
            conditions(&with(json!({"weather": code}))).weather,
            Some(weather)
        );
    }
    assert_eq!(conditions(&with(json!({"weather": "all"}))).weather, None);
    assert_eq!(conditions(&with(json!({"weather": null}))).weather, None);
    assert_eq!(
        conditions(&with(json!({"day_type": "平日"}))).day_type,
        Some(DayType::Weekday)
    );
    assert_eq!(
        conditions(&with(json!({"day_type": "休日"}))).day_type,
        Some(DayType::Holiday)
    );
    assert_eq!(conditions(&with(json!({"day_type": "all"}))).day_type, None);
    assert_eq!(conditions(&with(json!({"day_type": null}))).day_type, None);
    for extra in [
        json!({"weather": 0}),
        json!({"weather": 6}),
        json!({"weather": "晴れ"}),
        json!({"weather": false}),
        json!({"day_type": 0}),
        json!({"day_type": 1}),
        json!({"day_type": "土曜"}),
        json!({"day_type": true}),
    ] {
        assert_rejected(&with(extra)).await;
    }
}

#[tokio::test]
async fn validation_order_reports_the_first_problem() {
    // origin→dest→直線距離→mode→max_detour_min→period→hour→weather→day_type
    let message = assert_rejected(
        r#"{"origin":{"lat":0,"lon":0},"dest":{"lat":0,"lon":0},"mode":"x","period":"x"}"#,
    )
    .await;
    assert_eq!(
        message,
        "出発地には日本国内の緯度・経度を指定してください。"
    );
    let message = assert_rejected(&with(json!({"mode": "x", "max_detour_min": 99}))).await;
    assert_eq!(
        message,
        "移動手段はwalk・bike・carのいずれかを指定してください。"
    );
    let message = assert_rejected(&with(json!({"max_detour_min": 99, "period": "x"}))).await;
    assert_eq!(message, "追加を許容する時間は0〜30分で指定してください。");
    let message = assert_rejected(&with(json!({"hour": 99, "weather": "x"}))).await;
    assert_eq!(message, "hourは0〜23の整数で指定してください。");
    let message = assert_rejected(&with(json!({"weather": "x", "day_type": "x"}))).await;
    assert_eq!(
        message,
        "天候は晴・曇・雨・霧・雪・allのいずれかを指定してください。"
    );
}

#[tokio::test]
async fn compatible_inputs_normalize_to_the_same_query() {
    let canonical =
        with(json!({"mode": "bike", "period": "夜", "weather": "雨", "day_type": "休日"}));
    let compatible = with(
        json!({"mode": "bike", "時間帯": 3, "weather": 3.0, "day_type": "休日", "unknown": [1, 2]}),
    );
    let by_hour = with(json!({"mode": "bike", "hour": 22, "weather": 3, "day_type": "休日"}));
    let expected = parse_route_request(canonical.as_bytes()).unwrap();
    assert_eq!(
        parse_route_request(compatible.as_bytes()).unwrap(),
        expected
    );
    assert_eq!(parse_route_request(by_hour.as_bytes()).unwrap(), expected);
}

#[tokio::test]
async fn duplicate_keys_are_rejected_at_any_depth() {
    let message = assert_rejected(
        r#"{"origin":{"lat":35.681,"lon":139.767},"dest":{"lat":35.69,"lon":139.77},"mode":"walk","mode":"car"}"#,
    )
    .await;
    assert_eq!(message, "JSONに重複したキーがあります。");
    assert_rejected(
        r#"{"origin":{"lat":35.681,"lat":35.682,"lon":139.767},"dest":{"lat":35.69,"lon":139.77}}"#,
    )
    .await;
    assert_rejected(
        r#"{"origin":{"lat":35.681,"lon":139.767},"dest":{"lat":35.69,"lon":139.77},"x":[{"a":1,"a":2}]}"#,
    )
    .await;
}

#[tokio::test]
async fn body_size_limits() {
    let base = common::valid_route_body().to_string();
    let padded = |total: usize| format!("{base}{}", " ".repeat(total - base.len()));
    assert_accepted(&padded(16_384)).await;
    let message = assert_rejected(&padded(16_385)).await;
    assert_eq!(message, "リクエスト本文は1〜16384バイトにしてください。");
    let message = assert_rejected("").await;
    assert_eq!(message, "リクエスト本文は1〜16384バイトにしてください。");
}

#[tokio::test]
async fn content_type_must_be_application_json() {
    let body = common::valid_route_body().to_string();
    for content_type in [
        "application/json",
        "application/json; charset=utf-8",
        "Application/JSON;charset=UTF-8",
    ] {
        let (response, _) = send(Some(content_type), body.clone()).await;
        assert_eq!(response.status, StatusCode::OK, "{content_type}");
    }
    for content_type in [
        None,
        Some("text/plain"),
        Some("application/json; charset=latin1"),
        Some("application/jsonx"),
        Some("application/x-www-form-urlencoded"),
    ] {
        let (response, remaining) = send(content_type, body.clone()).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{content_type:?}");
        assert_eq!(remaining, 1);
        assert_eq!(
            response.json()["error"]["message"],
            "Content-Typeはapplication/jsonにしてください。"
        );
    }
}
