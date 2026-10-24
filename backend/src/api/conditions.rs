//! `GET /api/conditions`（behavior.md 9章）。
//!
//! 時間帯・曜日区分は事故記録の前処理と同じ `Calendar` で判定し、天候はOpen-Meteoの
//! 現況・毎時予報から決める。入力が正しければ常に200で、天候が取れないことは
//! `weather_status` で示す。事故データを参照しないので未readyでも応答する。

use axum::extract::{RawQuery, State};
use axum::response::{IntoResponse, Response};
use chrono::{
    DateTime, Duration as ChronoDuration, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Utc,
};
use serde::Serialize;

use crate::api::AppState;
use crate::api::error::{ApiError, json_ok};
use crate::api::query::QueryParams;
use crate::api::weather_code::weather_from_wmo;
use crate::contract::geo::{LAT_RANGE, LON_RANGE};
use crate::contract::{CancelToken, Interrupt, TokyoMinute, Weather};
use crate::providers::open_meteo::{RoundedPoint, TOKYO_OFFSET_SECONDS, WeatherError};

pub const INVALID_DATETIME: &str = "datetimeはオフセット付きのISO 8601形式（例：2026-11-03T14:30+09:00）で、2000-01-01T00:00+09:00〜2100-12-31T23:59+09:00の実在する日時を1個指定してください。";
pub const LAT_LON_PAIR: &str = "latとlonは両方を指定するか、両方を省略してください。";
pub const INVALID_LAT_LON: &str =
    "latは20〜46、lonは122〜154の数値で、それぞれ1個指定してください。";

/// 現況として扱う基準時刻の幅（分）。
const CURRENT_WINDOW_MIN: i64 = 15;
/// `datetime` を受け付ける最初と最後の年（Asia/Tokyo、各年の1月1日0時〜12月31日23時59分）。
const DATETIME_YEARS: (i32, i32) = (2000, 2100);

/// 検証済みの入力。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConditionsInput {
    /// Asia/Tokyoの分単位の基準時刻。
    pub at: NaiveDateTime,
    pub point: Option<RoundedPoint>,
}

/// クエリを検証する。検証順はdatetime→lat/lon。
pub fn parse_input(query: Option<&str>) -> Result<ConditionsInput, ApiError> {
    let params = QueryParams::parse(query, &["datetime", "lat", "lon"]);
    let at = match params.single("datetime") {
        Ok(Some(text)) => parse_datetime(text),
        _ => None,
    }
    .ok_or(ApiError::InvalidRequest(INVALID_DATETIME))?;
    let (Ok(lat), Ok(lon)) = (params.single("lat"), params.single("lon")) else {
        return Err(ApiError::InvalidRequest(INVALID_LAT_LON));
    };
    let point = match (lat, lon) {
        (None, None) => None,
        (Some(lat), Some(lon)) => {
            let lat = parse_decimal(lat).filter(|v| LAT_RANGE.contains(v));
            let lon = parse_decimal(lon).filter(|v| LON_RANGE.contains(v));
            let (Some(lat), Some(lon)) = (lat, lon) else {
                return Err(ApiError::InvalidRequest(INVALID_LAT_LON));
            };
            Some(RoundedPoint::round(lat, lon))
        }
        _ => return Err(ApiError::InvalidRequest(LAT_LON_PAIR)),
    };
    Ok(ConditionsInput { at, point })
}

/// `YYYY-MM-DDThh:mm[:ss](Z|±hh:mm)` を、Asia/Tokyoの分単位の日時にする。範囲外・実在しない日時は `None`。
pub fn parse_datetime(text: &str) -> Option<NaiveDateTime> {
    let b = text.as_bytes();
    let digits = |range: std::ops::Range<usize>| -> Option<u32> {
        let part = b.get(range)?;
        part.iter().all(u8::is_ascii_digit).then(|| {
            part.iter()
                .fold(0u32, |acc, d| acc * 10 + u32::from(d - b'0'))
        })
    };
    let sep = |i: usize, c: u8| b.get(i) == Some(&c);
    if !(sep(4, b'-') && sep(7, b'-') && sep(10, b'T') && sep(13, b':')) {
        return None;
    }
    let date = NaiveDate::from_ymd_opt(
        i32::try_from(digits(0..4)?).ok()?,
        digits(5..7)?,
        digits(8..10)?,
    )?;
    let (hour, minute) = (digits(11..13)?, digits(14..16)?);
    let (second, rest) = if sep(16, b':') {
        (digits(17..19)?, b.get(19..)?)
    } else {
        (0, b.get(16..)?)
    };
    // chronoは秒=60（うるう秒）を受け付けるので、ここで59までに絞る
    if second > 59 {
        return None;
    }
    let time = NaiveTime::from_hms_opt(hour, minute, second)?;
    let offset_seconds = match rest {
        b"Z" => 0,
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] => {
            let two = |a: u8, b: u8| -> Option<i64> {
                (a.is_ascii_digit() && b.is_ascii_digit())
                    .then(|| i64::from(a - b'0') * 10 + i64::from(b - b'0'))
            };
            let (h, m) = (two(*h1, *h2)?, two(*m1, *m2)?);
            if h > 23 || m > 59 {
                return None;
            }
            let seconds = h * 3600 + m * 60;
            if *sign == b'-' { -seconds } else { seconds }
        }
        _ => return None,
    };
    let local = NaiveDateTime::new(date, time);
    let tokyo = local
        .checked_sub_signed(ChronoDuration::seconds(offset_seconds))?
        .checked_add_signed(ChronoDuration::seconds(TOKYO_OFFSET_SECONDS))?;
    let tokyo = tokyo.with_second(0)?;
    // 扱う範囲（backend-spec.md：2000-01-01T00:00〜2100-12-31T23:59、Asia/Tokyo）
    let (first_year, last_year) = DATETIME_YEARS;
    let min = NaiveDate::from_ymd_opt(first_year, 1, 1)?.and_hms_opt(0, 0, 0)?;
    let max = NaiveDate::from_ymd_opt(last_year, 12, 31)?.and_hms_opt(23, 59, 0)?;
    (min..=max).contains(&tokyo).then_some(tokyo)
}

/// 10進表記の数値（JSONの数値と同じ文法）。有限値だけ。
pub fn parse_decimal(text: &str) -> Option<f64> {
    let b = text.as_bytes();
    let mut i = 0;
    if b.get(i) == Some(&b'-') {
        i += 1;
    }
    let int_start = i;
    while b.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    let int_len = i - int_start;
    if int_len == 0 || (int_len > 1 && b[int_start] == b'0') {
        return None;
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        let frac_start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == frac_start {
            return None;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let exp_start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == exp_start {
            return None;
        }
    }
    if i != b.len() {
        return None;
    }
    text.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// 天候の判定結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherOutcome {
    Ok {
        weather: Weather,
        /// Asia/Tokyoのローカル時刻。
        target_time: NaiveDateTime,
        kind: SourceKind,
    },
    NoLocation,
    OutOfRange,
    Past,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Current,
    Forecast,
}

/// 上流を呼ぶ前に決まる区分。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Window {
    Past,
    Current,
    Forecast,
    OutOfRange,
}

fn window(at: NaiveDateTime, now: DateTime<Utc>, forecast_days: u8) -> Window {
    let at_utc = at - ChronoDuration::seconds(TOKYO_OFFSET_SECONDS);
    let diff = at_utc - now.naive_utc();
    let current = ChronoDuration::minutes(CURRENT_WINDOW_MIN);
    if diff < -current {
        Window::Past
    } else if diff <= current {
        Window::Current
    } else if diff <= ChronoDuration::days(i64::from(forecast_days)) {
        Window::Forecast
    } else {
        Window::OutOfRange
    }
}

async fn decide_weather(
    state: &AppState,
    input: &ConditionsInput,
    cancel: &CancelToken,
) -> WeatherOutcome {
    let Some(point) = input.point else {
        return WeatherOutcome::NoLocation;
    };
    let window = window(
        input.at,
        state.clock().now(),
        state.settings().weather_forecast_days,
    );
    let kind = match window {
        Window::Past => return WeatherOutcome::Past,
        Window::OutOfRange => return WeatherOutcome::OutOfRange,
        Window::Current => SourceKind::Current,
        Window::Forecast => SourceKind::Forecast,
    };
    let deadline =
        tokio::time::Instant::from_std(cancel.deadline().expect("要求ごとのトークンは期限付き"));
    let report = tokio::time::timeout_at(deadline, state.weather().get(point, cancel))
        .await
        .unwrap_or(Err(WeatherError::Interrupted(Interrupt::DeadlineExceeded)));
    let Ok(report) = cancel.settle(report) else {
        return WeatherOutcome::Unavailable;
    };
    let (target_time, code) = match kind {
        SourceKind::Current => (report.current_time, report.current_code),
        SourceKind::Forecast => {
            let hour = input.at.with_minute(0).expect("0分は常に有効");
            match report.hourly.iter().find(|(time, _)| *time == hour) {
                Some(&(time, code)) => (time, code),
                None => return WeatherOutcome::OutOfRange,
            }
        }
    };
    match weather_from_wmo(code) {
        Some(weather) => WeatherOutcome::Ok {
            weather,
            target_time,
            kind,
        },
        None => WeatherOutcome::Unavailable,
    }
}

#[derive(Serialize)]
struct ConditionsDto {
    api_version: &'static str,
    datetime: String,
    period: &'static str,
    day_type: &'static str,
    day_type_reason: String,
    holiday_table_covered: bool,
    weather: Option<&'static str>,
    weather_status: &'static str,
    weather_source: Option<WeatherSourceDto>,
}

#[derive(Serialize)]
struct WeatherSourceDto {
    provider: &'static str,
    target_time: String,
    kind: &'static str,
}

/// Asia/Tokyoのローカル時刻を `+09:00` 付きのRFC 3339にする。
fn tokyo_text(local: NaiveDateTime) -> String {
    format!("{}+09:00", local.format("%Y-%m-%dT%H:%M:%S"))
}

pub async fn handle(State(state): State<AppState>, RawQuery(query): RawQuery) -> Response {
    let input = match parse_input(query.as_deref()) {
        Ok(input) => input,
        Err(error) => return error.into_response(),
    };
    let cancel = CancelToken::with_timeout(state.settings().conditions_timeout);
    let _guard = cancel.cancel_on_drop();
    let class = state.calendar().classify(TokyoMinute::from_local(input.at));
    let weather = decide_weather(&state, &input, &cancel).await;
    let (weather_value, status, source) = match weather {
        WeatherOutcome::Ok {
            weather,
            target_time,
            kind,
        } => (
            Some(weather.label()),
            "ok",
            Some(WeatherSourceDto {
                provider: "Open-Meteo",
                target_time: tokyo_text(target_time),
                kind: match kind {
                    SourceKind::Current => "current",
                    SourceKind::Forecast => "forecast",
                },
            }),
        ),
        WeatherOutcome::NoLocation => (None, "no_location", None),
        WeatherOutcome::OutOfRange => (None, "out_of_range", None),
        WeatherOutcome::Past => (None, "past", None),
        WeatherOutcome::Unavailable => (None, "unavailable", None),
    };
    json_ok(&ConditionsDto {
        api_version: "2.0",
        datetime: tokyo_text(input.at),
        period: class.period.label(),
        day_type: class.day.day_type().label(),
        day_type_reason: class.day.reason.label(),
        holiday_table_covered: class.day.holiday_table_covered,
        weather: weather_value,
        weather_status: status,
        weather_source: source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(text: &str) -> Option<String> {
        parse_datetime(text).map(tokyo_text)
    }

    #[test]
    fn datetime_is_converted_to_tokyo_and_truncated() {
        assert_eq!(
            dt("2026-11-03T14:30+09:00").unwrap(),
            "2026-11-03T14:30:00+09:00"
        );
        assert_eq!(
            dt("2026-11-03T05:30:59Z").unwrap(),
            "2026-11-03T14:30:00+09:00"
        );
        assert_eq!(
            dt("2026-11-02T23:59:59-15:00").unwrap(),
            "2026-11-03T23:59:00+09:00"
        );
        assert_eq!(
            dt("1999-12-31T15:00Z").unwrap(),
            "2000-01-01T00:00:00+09:00"
        );
        assert_eq!(
            dt("2100-12-31T23:59:59+09:00").unwrap(),
            "2100-12-31T23:59:00+09:00"
        );
    }

    #[test]
    fn invalid_datetimes_are_rejected() {
        for text in [
            "2026-11-03T14:30",
            "2026-11-03",
            "2026-02-30T00:00Z",
            "2026-11-03T24:00Z",
            "2026-11-03T14:60Z",
            "2026-11-03T14:30:60Z",
            "2026-11-03T14:30+24:00",
            "2026-11-03T14:30+09:60",
            "2026-11-03T14:30+0900",
            "2026-11-03t14:30Z",
            "2026-11-03T14:30z",
            "2026-11-03T14:30:5Z",
            "2026-11-03T14:30:00.5Z",
            "2026-11-03 14:30Z",
            "1999-12-31T14:59Z",
            "2100-12-31T15:00Z",
            "+2026-11-03T14:30Z",
            "２０２６-11-03T14:30Z",
        ] {
            assert_eq!(dt(text), None, "{text}");
        }
    }

    #[test]
    fn decimals_follow_json_number_syntax() {
        assert_eq!(parse_decimal("35.7"), Some(35.7));
        assert_eq!(parse_decimal("-0.5"), Some(-0.5));
        assert_eq!(parse_decimal("1e2"), Some(100.0));
        for text in [
            "", "+1", "01", "1.", ".5", "1e", "NaN", "inf", " 1", "1 ", "0x10", "1e999",
        ] {
            assert_eq!(parse_decimal(text), None, "{text}");
        }
    }

    #[test]
    fn windows_follow_the_spec_boundaries() {
        let now = DateTime::parse_from_rfc3339("2026-11-03T05:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let at = |text: &str| parse_datetime(text).unwrap();
        assert_eq!(window(at("2026-11-03T13:44+09:00"), now, 7), Window::Past);
        assert_eq!(
            window(at("2026-11-03T13:45+09:00"), now, 7),
            Window::Current
        );
        assert_eq!(
            window(at("2026-11-03T14:15+09:00"), now, 7),
            Window::Current
        );
        assert_eq!(
            window(at("2026-11-03T14:16+09:00"), now, 7),
            Window::Forecast
        );
        assert_eq!(
            window(at("2026-11-10T14:00+09:00"), now, 7),
            Window::Forecast
        );
        assert_eq!(
            window(at("2026-11-10T14:01+09:00"), now, 7),
            Window::OutOfRange
        );
    }
}
