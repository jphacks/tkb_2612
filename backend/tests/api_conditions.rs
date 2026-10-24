//! S10：`GET /api/conditions` の入力検証・時間帯と曜日区分・天候の全分岐・気象キャッシュ・WMO変換表。
//!
//! 現在時刻は固定（2026-10-24T09:00+09:00）。上流は偽の固定応答（実通信しない）。

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use accident_route_backend::api::weather_cache::{MAX_ENTRIES, WeatherCache};
use accident_route_backend::api::weather_code::weather_from_wmo;
use accident_route_backend::contract::{
    Calendar, CancelToken, DayClassification, DayTypeReason, Weather,
};
use accident_route_backend::providers::gate::{Gate, GatePolicy, LocalGateStore};
use accident_route_backend::providers::http::UpstreamClient;
use accident_route_backend::providers::open_meteo::{
    OpenMeteoProvider, RoundedPoint, WeatherError, WeatherReport,
};
use axum::http::StatusCode;
use chrono::{NaiveDate, NaiveDateTime};
use serde_json::{Value, json};

use common::{FakeUpstream, FakeWeather, ManualClock, Reply, TestApp, TestAppBuilder};

const AT: &str = "lat=35.681&lon=139.767";

fn local(text: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M").unwrap()
}

/// 現在時刻に取得した気象。毎時予報は10/24 0時から8日分、コードは時刻ごとに決める。
fn report_with(code_at: impl Fn(NaiveDateTime) -> i64) -> WeatherReport {
    let start = local("2026-10-24T00:00");
    WeatherReport {
        current_time: local("2026-10-24T09:00"),
        current_code: 3,
        hourly: (0..24 * 8)
            .map(|h| {
                let t = start + chrono::Duration::hours(h);
                (t, code_at(t))
            })
            .collect(),
    }
}

fn report() -> WeatherReport {
    report_with(|_| 61)
}

fn app_with_weather(weather: Arc<FakeWeather>) -> TestApp {
    TestAppBuilder {
        weather,
        ..Default::default()
    }
    .build()
}

fn query(datetime: &str, rest: &str) -> String {
    let datetime: String = form_urlencoded::byte_serialize(datetime.as_bytes()).collect();
    if rest.is_empty() {
        format!("/api/conditions?datetime={datetime}")
    } else {
        format!("/api/conditions?datetime={datetime}&{rest}")
    }
}

async fn conditions(app: &TestApp, datetime: &str, rest: &str) -> Value {
    let response = app.get(&query(datetime, rest)).await;
    assert_eq!(response.status, StatusCode::OK, "{datetime} {rest}");
    common::assert_api_headers(&response);
    response.json()
}

/// 渡された暦日を記録する曜日区分。
struct RecordingCalendar {
    day: DayClassification,
    dates: Mutex<Vec<NaiveDate>>,
}

impl Calendar for RecordingCalendar {
    fn classify_date(&self, date: NaiveDate) -> DayClassification {
        self.dates.lock().unwrap().push(date);
        self.day.clone()
    }
}

// ---- 入力検証 ---------------------------------------------------------------------

#[tokio::test]
async fn invalid_inputs_are_400_and_checked_before_the_upstream() {
    let datetime_msg = "datetimeはオフセット付きのISO 8601形式（例：2026-11-03T14:30+09:00）で、2000-01-01T00:00+09:00〜2100-12-31T23:59+09:00の実在する日時を1個指定してください。";
    let pair_msg = "latとlonは両方を指定するか、両方を省略してください。";
    let latlon_msg = "latは20〜46、lonは122〜154の数値で、それぞれ1個指定してください。";
    let cases = [
        ("/api/conditions", datetime_msg),
        ("/api/conditions?lat=35&lon=139", datetime_msg),
        ("/api/conditions?datetime=2026-10-24T09:00", datetime_msg),
        (
            "/api/conditions?datetime=2026-02-29T09:00%2B09:00",
            datetime_msg,
        ),
        (
            "/api/conditions?datetime=1999-12-31T23:59%2B09:00",
            datetime_msg,
        ),
        (
            "/api/conditions?datetime=2101-01-01T00:00%2B09:00",
            datetime_msg,
        ),
        (
            "/api/conditions?datetime=2026-10-24T09:00Z&datetime=2026-10-24T09:00Z",
            datetime_msg,
        ),
        // 検証順はdatetime→lat/lon
        (
            "/api/conditions?datetime=bad&lat=999&lon=139.7",
            datetime_msg,
        ),
        (
            "/api/conditions?datetime=2026-10-24T09:00Z&lat=35.6",
            pair_msg,
        ),
        (
            "/api/conditions?datetime=2026-10-24T09:00Z&lon=139.7",
            pair_msg,
        ),
        (
            "/api/conditions?datetime=2026-10-24T09:00Z&lat=19.99&lon=139.7",
            latlon_msg,
        ),
        (
            "/api/conditions?datetime=2026-10-24T09:00Z&lat=35.6&lon=154.01",
            latlon_msg,
        ),
        (
            "/api/conditions?datetime=2026-10-24T09:00Z&lat=abc&lon=139.7",
            latlon_msg,
        ),
        (
            "/api/conditions?datetime=2026-10-24T09:00Z&lat=&lon=139.7",
            latlon_msg,
        ),
        (
            "/api/conditions?datetime=2026-10-24T09:00Z&lat=35.6&lat=35.6&lon=139.7",
            latlon_msg,
        ),
        (
            "/api/conditions?datetime=2026-10-24T09:00Z&lat=35.6&lon=139.7&lon=139.7",
            latlon_msg,
        ),
    ];
    for (target, message) in cases {
        let app = app_with_weather(FakeWeather::new([]));
        let response = app.get(target).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{target}");
        assert_eq!(response.error_code(), "invalid_request");
        assert_eq!(response.json()["error"]["message"], message, "{target}");
    }
}

#[tokio::test]
async fn boundaries_of_the_accepted_range_and_unknown_keys() {
    let app = app_with_weather(FakeWeather::new([]));
    for (datetime, expected) in [
        ("2000-01-01T00:00+09:00", "2000-01-01T00:00:00+09:00"),
        ("1999-12-31T15:00Z", "2000-01-01T00:00:00+09:00"),
        ("2100-12-31T23:59:59+09:00", "2100-12-31T23:59:00+09:00"),
        ("2026-10-24T00:00:00-05:30", "2026-10-24T14:30:00+09:00"),
    ] {
        assert_eq!(conditions(&app, datetime, "").await["datetime"], expected);
    }
    // 未知のキーは無視し、境界の緯度経度は受け付ける
    let body = conditions(&app, "2026-10-20T09:00+09:00", "lat=20&lon=154&unknown=1&x").await;
    assert_eq!(body["weather_status"], "past");
}

// ---- 時間帯・曜日区分 -----------------------------------------------------------------

#[tokio::test]
async fn period_boundaries_follow_the_calendar_contract() {
    let app = app_with_weather(FakeWeather::new([]));
    for (time, period) in [
        ("00:00", "深夜"),
        ("03:59", "深夜"),
        ("04:00", "早朝"),
        ("08:59", "早朝"),
        ("09:00", "昼"),
        ("15:59", "昼"),
        ("16:00", "夕"),
        ("18:59", "夕"),
        ("19:00", "夜"),
        ("23:59", "夜"),
    ] {
        let body = conditions(&app, &format!("2026-11-03T{time}+09:00"), "").await;
        assert_eq!(body["period"], period, "{time}");
    }
}

#[tokio::test]
async fn day_classification_is_returned_as_the_calendar_decides() {
    let reasons = [
        (DayTypeReason::Weekday, "平日", "平日"),
        (DayTypeReason::Saturday, "休日", "土曜"),
        (DayTypeReason::Sunday, "休日", "日曜"),
        (
            DayTypeReason::NationalHoliday {
                name: "文化の日".to_owned(),
            },
            "休日",
            "祝日：文化の日",
        ),
        (DayTypeReason::SubstituteHoliday, "休日", "振替休日"),
        (DayTypeReason::CitizensHoliday, "休日", "国民の休日"),
        (DayTypeReason::YearEnd, "休日", "年末年始"),
    ];
    for covered in [true, false] {
        for (reason, day_type, label) in &reasons {
            let calendar = Arc::new(RecordingCalendar {
                day: DayClassification {
                    reason: reason.clone(),
                    holiday_table_covered: covered,
                },
                dates: Mutex::new(Vec::new()),
            });
            let app = TestAppBuilder {
                custom_calendar: Some(calendar.clone()),
                ..Default::default()
            }
            .build();
            // UTCの15:30はAsia/Tokyoの翌日0:30。暦日はAsia/Tokyoで渡す（深夜も前日扱いにしない）
            let body = conditions(&app, "2026-11-02T15:30Z", "").await;
            assert_eq!(body["day_type"], *day_type);
            assert_eq!(body["day_type_reason"], *label);
            assert_eq!(body["holiday_table_covered"], covered);
            assert_eq!(body["period"], "深夜");
            assert_eq!(
                *calendar.dates.lock().unwrap(),
                [NaiveDate::from_ymd_opt(2026, 11, 3).unwrap()]
            );
        }
    }
}

// ---- 天候の全分岐 --------------------------------------------------------------------

/// (基準時刻, 位置あり, 期待status, kind, target_time, 上流呼出し回数)
type WindowCase = (
    &'static str,
    bool,
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    usize,
);

#[tokio::test]
async fn weather_windows_and_upstream_calls() {
    let cases: [WindowCase; 9] = [
        (
            "2026-10-24T09:00+09:00",
            false,
            "no_location",
            None,
            None,
            0,
        ),
        ("2026-10-24T08:44+09:00", true, "past", None, None, 0),
        (
            "2026-10-24T08:45+09:00",
            true,
            "ok",
            Some("current"),
            Some("2026-10-24T09:00:00+09:00"),
            1,
        ),
        (
            "2026-10-24T09:15+09:00",
            true,
            "ok",
            Some("current"),
            Some("2026-10-24T09:00:00+09:00"),
            1,
        ),
        (
            "2026-10-24T09:16+09:00",
            true,
            "ok",
            Some("forecast"),
            Some("2026-10-24T09:00:00+09:00"),
            1,
        ),
        (
            "2026-10-26T14:59+09:00",
            true,
            "ok",
            Some("forecast"),
            Some("2026-10-26T14:00:00+09:00"),
            1,
        ),
        (
            "2026-10-31T09:00+09:00",
            true,
            "ok",
            Some("forecast"),
            Some("2026-10-31T09:00:00+09:00"),
            1,
        ),
        // 日数超過は通信前に判定する
        (
            "2026-10-31T09:01+09:00",
            true,
            "out_of_range",
            None,
            None,
            0,
        ),
        (
            "2030-01-01T00:00+09:00",
            true,
            "out_of_range",
            None,
            None,
            0,
        ),
    ];
    for (datetime, located, status, kind, target, calls) in cases {
        let weather = FakeWeather::new(if calls > 0 {
            vec![Ok(report())]
        } else {
            vec![]
        });
        let app = app_with_weather(weather.clone());
        let body = conditions(&app, datetime, if located { AT } else { "" }).await;
        assert_eq!(body["weather_status"], status, "{datetime}");
        assert_eq!(weather.calls().len(), calls, "{datetime}");
        match (kind, target) {
            (Some(kind), Some(target)) => {
                assert_eq!(
                    body["weather"],
                    if kind == "current" { "曇" } else { "雨" },
                    "{datetime}"
                );
                assert_eq!(
                    body["weather_source"],
                    json!({"provider": "Open-Meteo", "target_time": target, "kind": kind})
                );
            }
            _ => {
                assert_eq!(body["weather"], Value::Null);
                assert_eq!(body["weather_source"], Value::Null);
            }
        }
    }
}

/// 予報範囲内でもhourly行が無ければ、取得した応答を見てから out_of_range にする。
#[tokio::test]
async fn missing_hourly_row_is_out_of_range_after_the_upstream_call() {
    let mut report = report();
    report
        .hourly
        .retain(|(t, _)| *t != local("2026-10-26T14:00"));
    let weather = FakeWeather::new([Ok(report)]);
    let app = app_with_weather(weather.clone());
    let body = conditions(&app, "2026-10-26T14:30+09:00", AT).await;
    assert_eq!(body["weather_status"], "out_of_range");
    assert_eq!(weather.calls().len(), 1);
}

#[tokio::test]
async fn failures_are_unavailable_with_200() {
    for result in [
        Err(WeatherError::Failed),
        Err(WeatherError::Busy),
        Err(WeatherError::Interrupted(
            accident_route_backend::contract::Interrupt::DeadlineExceeded,
        )),
        // 対応表にないコード
        Ok(report_with(|_| 4)),
    ] {
        for datetime in ["2026-10-24T09:00+09:00", "2026-10-25T12:00+09:00"] {
            let mut result = result.clone();
            if let Ok(report) = &mut result {
                report.current_code = 4;
            }
            let app = app_with_weather(FakeWeather::new([result]));
            let body = conditions(&app, datetime, AT).await;
            assert_eq!(body["weather_status"], "unavailable", "{datetime}");
            assert_eq!(body["weather"], Value::Null);
            assert_eq!(body["weather_source"], Value::Null);
        }
    }
}

/// 全体期限（`CONDITIONS_TIMEOUT_MS` を縮めて検証）内に天候が得られなければ天候だけunavailableの200。
#[tokio::test]
async fn conditions_deadline_gives_unavailable() {
    let mut builder = TestAppBuilder {
        weather: FakeWeather::stalling(),
        ..Default::default()
    };
    builder.settings.conditions_timeout = Duration::from_millis(150);
    let app = builder.build();
    let started = std::time::Instant::now();
    let body = conditions(&app, "2026-10-24T09:00+09:00", AT).await;
    assert_eq!(body["weather_status"], "unavailable");
    assert_eq!(body["period"], "昼");
    assert!(started.elapsed() < Duration::from_secs(1));
}

/// 事故データが未readyでも応答する。
#[tokio::test]
async fn conditions_do_not_wait_for_accident_data() {
    let app = TestAppBuilder {
        engine: None,
        ..Default::default()
    }
    .build();
    let body = conditions(&app, "2026-10-24T09:00+09:00", "").await;
    assert_eq!(body["weather_status"], "no_location");
}

#[tokio::test]
async fn post_is_405() {
    let app = app_with_weather(FakeWeather::new([]));
    let response = app.post_json("/api/conditions", "{}").await;
    assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.header("allow"), Some("GET"));
}

// ---- WMO変換表 -------------------------------------------------------------------

#[test]
fn wmo_table_matches_behavior_chapter_9() {
    let clear = [0, 1, 2];
    let cloudy = [3];
    let fog = [45, 48];
    let rain = [
        51, 53, 55, 56, 57, 61, 63, 65, 66, 67, 80, 81, 82, 95, 96, 99,
    ];
    let snow = [71, 73, 75, 77, 85, 86];
    for code in -5..=300 {
        let expected = if clear.contains(&code) {
            Some(Weather::Clear)
        } else if cloudy.contains(&code) {
            Some(Weather::Cloudy)
        } else if fog.contains(&code) {
            Some(Weather::Fog)
        } else if rain.contains(&code) {
            Some(Weather::Rain)
        } else if snow.contains(&code) {
            Some(Weather::Snow)
        } else {
            None
        };
        assert_eq!(weather_from_wmo(code), expected, "WMO {code}");
    }
}

#[tokio::test]
async fn each_weather_label_is_returned_through_the_handler() {
    for (code, label) in [(0, "晴"), (3, "曇"), (45, "霧"), (95, "雨"), (86, "雪")] {
        let mut report = report();
        report.current_code = code;
        let app = app_with_weather(FakeWeather::new([Ok(report)]));
        let body = conditions(&app, "2026-10-24T09:00+09:00", AT).await;
        assert_eq!(body["weather"], label, "WMO {code}");
    }
}

// ---- キャッシュ ----------------------------------------------------------------------

/// 丸めはキャッシュキーと上流URLの両方に適用される（実アダプタ＋偽Open-Meteo）。
#[tokio::test]
async fn rounding_applies_to_both_the_cache_key_and_the_upstream_url() {
    let upstream = FakeUpstream::fixed(Reply::json(&json!({
        "utc_offset_seconds": 32400,
        "current": {"time": "2026-10-24T09:00", "weather_code": 0},
        "hourly": {"time": ["2026-10-24T09:00"], "weather_code": [0]},
    })))
    .await;
    let clock = ManualClock::new(common::fixed_now());
    let provider = OpenMeteoProvider::new(
        upstream.base_url(),
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
        weather: Arc::new(provider),
        clock,
        ..Default::default()
    }
    .build();
    for at in ["lat=35.678&lon=139.7649", "lat=35.6849&lon=139.7551"] {
        let body = conditions(&app, "2026-10-24T09:00+09:00", at).await;
        assert_eq!(body["weather"], "晴");
    }
    let requests = upstream.requests();
    assert_eq!(requests.len(), 1, "同じ丸め値はキャッシュから返す");
    let query = requests[0].query.as_deref().unwrap();
    assert!(
        query.starts_with("latitude=35.68&longitude=139.76&"),
        "{query}"
    );
}

#[tokio::test]
async fn ttl_boundary_and_failures_are_not_cached() {
    let clock = ManualClock::new(common::fixed_now());
    let weather = FakeWeather::new([Err(WeatherError::Failed), Ok(report()), Ok(report())]);
    let cache = WeatherCache::new(weather.clone(), clock.clone(), Duration::from_secs(600));
    let point = RoundedPoint::round(35.68, 139.76);
    let cancel = CancelToken::unbounded();
    assert_eq!(cache.get(point, &cancel).await, Err(WeatherError::Failed));
    assert!(cache.is_empty(), "失敗は保存しない");
    cache.get(point, &cancel).await.unwrap();
    assert_eq!(weather.calls().len(), 2);
    clock.advance(Duration::from_millis(599_999));
    cache.get(point, &cancel).await.unwrap();
    assert_eq!(weather.calls().len(), 2, "TTL未満はキャッシュ");
    clock.advance(Duration::from_millis(1));
    cache.get(point, &cancel).await.unwrap();
    assert_eq!(weather.calls().len(), 3, "TTLちょうどで取り直す");
}

/// 再アクセスが無くても、保存からTTLで座標キーと応答を消す（architecture.md 8章「最大10分保持」）。
#[tokio::test(start_paused = true)]
async fn entries_are_removed_after_the_ttl_without_access() {
    let clock = ManualClock::new(common::fixed_now());
    let weather = FakeWeather::new([Ok(report()), Ok(report()), Ok(report())]);
    let ttl = Duration::from_secs(600);
    let cache = WeatherCache::new(weather.clone(), clock.clone(), ttl);
    let point = RoundedPoint::round(35.68, 139.76);
    let cancel = CancelToken::unbounded();
    cache.get(point, &cancel).await.unwrap();
    assert_eq!(cache.len(), 1);
    tokio::time::sleep(ttl - Duration::from_millis(1)).await;
    assert_eq!(cache.len(), 1, "TTL未満は残す");
    tokio::time::sleep(Duration::from_millis(2)).await;
    assert!(cache.is_empty(), "TTLで消す");

    // 保存し直した応答は、前の保存の期限では消さない
    cache.get(point, &cancel).await.unwrap();
    tokio::time::sleep(ttl / 2).await;
    clock.advance(ttl);
    cache.get(point, &cancel).await.unwrap();
    assert_eq!(weather.calls().len(), 3);
    tokio::time::sleep(ttl / 2 + Duration::from_millis(1)).await;
    assert_eq!(cache.len(), 1, "前の保存の期限では消さない");
    tokio::time::sleep(ttl / 2).await;
    assert!(cache.is_empty());
}

/// 期限切れのキーは、取り直しに失敗しても残さない。
#[tokio::test]
async fn expired_entry_is_removed_even_if_refetching_fails() {
    let clock = ManualClock::new(common::fixed_now());
    let weather = FakeWeather::new([Ok(report()), Err(WeatherError::Failed)]);
    let cache = WeatherCache::new(weather.clone(), clock.clone(), Duration::from_secs(600));
    let point = RoundedPoint::round(35.68, 139.76);
    let cancel = CancelToken::unbounded();
    cache.get(point, &cancel).await.unwrap();
    clock.advance(Duration::from_secs(600));
    assert_eq!(cache.get(point, &cancel).await, Err(WeatherError::Failed));
    assert!(cache.is_empty());
}

/// 削除のタイマーが動かなくても（要求外にCPUが無い環境）、次の取得で他の地点の期限切れも消す。
#[tokio::test]
async fn any_access_removes_every_expired_entry() {
    let clock = ManualClock::new(common::fixed_now());
    let weather = FakeWeather::new([Ok(report()), Ok(report()), Ok(report())]);
    let cache = WeatherCache::new(weather.clone(), clock.clone(), Duration::from_secs(600));
    let cancel = CancelToken::unbounded();
    let (a, b, c) = (
        RoundedPoint::round(35.68, 139.76),
        RoundedPoint::round(35.70, 139.70),
        RoundedPoint::round(34.70, 135.50),
    );
    cache.get(a, &cancel).await.unwrap();
    clock.advance(Duration::from_secs(300));
    cache.get(b, &cancel).await.unwrap();
    clock.advance(Duration::from_secs(300));
    cache.get(c, &cancel).await.unwrap();
    assert_eq!(cache.len(), 2, "aだけが期限切れ");
    clock.advance(Duration::from_secs(300));
    cache.get(c, &cancel).await.unwrap();
    assert_eq!(cache.len(), 1, "bも期限切れ");
    assert_eq!(weather.calls().len(), 3);
}

/// TTLが0なら保存しない。
#[tokio::test]
async fn zero_ttl_does_not_store() {
    let clock = ManualClock::new(common::fixed_now());
    let weather = FakeWeather::new([Ok(report()), Ok(report())]);
    let cache = WeatherCache::new(weather.clone(), clock, Duration::ZERO);
    let point = RoundedPoint::round(35.68, 139.76);
    let cancel = CancelToken::unbounded();
    cache.get(point, &cancel).await.unwrap();
    assert!(cache.is_empty());
    cache.get(point, &cancel).await.unwrap();
    assert_eq!(weather.calls().len(), 2);
}

#[tokio::test]
async fn oldest_key_is_evicted_beyond_1024_keys() {
    assert_eq!(MAX_ENTRIES, 1024);
    let clock = ManualClock::new(common::fixed_now());
    let weather = FakeWeather::new((0..MAX_ENTRIES + 2).map(|_| Ok(report())));
    let cache = WeatherCache::new(weather.clone(), clock, Duration::from_secs(600));
    let cancel = CancelToken::unbounded();
    let point = |i: usize| RoundedPoint {
        lat_centi: 3000 + (i / 100) as i32,
        lon_centi: 13000 + (i % 100) as i32,
    };
    for i in 0..=MAX_ENTRIES {
        cache.get(point(i), &cancel).await.unwrap();
    }
    assert_eq!(cache.len(), MAX_ENTRIES);
    assert_eq!(weather.calls().len(), MAX_ENTRIES + 1);
    // 2番目に古いキーは残っている
    cache.get(point(1), &cancel).await.unwrap();
    assert_eq!(weather.calls().len(), MAX_ENTRIES + 1);
    // 最古のキーは除かれたので取り直す
    cache.get(point(0), &cancel).await.unwrap();
    assert_eq!(weather.calls().len(), MAX_ENTRIES + 2);
    assert_eq!(cache.len(), MAX_ENTRIES);
}
