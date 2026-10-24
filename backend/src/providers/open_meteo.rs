//! 気象のアダプタ（Open-Meteo Forecast API、behavior.md 9章）。
//!
//! 丸めた緯度経度を受け、現況のWMO天気コードと時刻、毎時予報の時刻とコードの列を返す。
//! WMOコードから天候への変換はアダプタの外（`api`）で行い、提供元を替えても共通にする。

use std::time::Duration;

use chrono::NaiveDateTime;
use serde_json::Value;

use crate::contract::{BoxFuture, CancelToken, Interrupt};
use crate::providers::gate::{Gate, GateError};
use crate::providers::http::{FetchError, UpstreamClient};

/// 応答に要求するUTCオフセット（Asia/Tokyo）。
pub const TOKYO_OFFSET_SECONDS: i64 = 32_400;

/// 小数2桁に丸めた地点。キャッシュキーと上流送信に同じ値を使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RoundedPoint {
    /// 緯度×100（整数）。
    pub lat_centi: i32,
    /// 経度×100（整数）。
    pub lon_centi: i32,
}

impl RoundedPoint {
    /// 小数2桁に丸める（Rust標準の四捨五入）。
    pub fn round(lat: f64, lon: f64) -> Self {
        Self {
            lat_centi: (lat * 100.0).round() as i32,
            lon_centi: (lon * 100.0).round() as i32,
        }
    }

    pub fn lat_text(&self) -> String {
        centi_text(self.lat_centi)
    }

    pub fn lon_text(&self) -> String {
        centi_text(self.lon_centi)
    }
}

fn centi_text(centi: i32) -> String {
    let sign = if centi < 0 { "-" } else { "" };
    let abs = centi.unsigned_abs();
    format!("{sign}{}.{:02}", abs / 100, abs % 100)
}

/// 1地点の気象。時刻はAsia/Tokyoのローカル時刻、コードはWMO天気コード。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeatherReport {
    pub current_time: NaiveDateTime,
    pub current_code: i64,
    /// 毎時予報（時刻, コード）。提供元の順。
    pub hourly: Vec<(NaiveDateTime, i64)>,
}

/// 気象取得の失敗。conditionsではどれも天候 `unavailable` になる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WeatherError {
    /// 通信・HTTP・1通信の期限・サイズ・形式の失敗。
    Failed,
    /// 待ち行列の満杯・共有上流ゲートの失敗・集計期間の上限。
    Busy,
    Interrupted(Interrupt),
}

impl From<Interrupt> for WeatherError {
    fn from(value: Interrupt) -> Self {
        WeatherError::Interrupted(value)
    }
}

/// 気象の提供元。
pub trait WeatherProvider: Send + Sync {
    fn fetch<'a>(
        &'a self,
        point: RoundedPoint,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<WeatherReport, WeatherError>>;
}

#[derive(Clone)]
pub struct OpenMeteoProvider {
    base: reqwest::Url,
    api_key: Option<String>,
    forecast_days: u8,
    client: UpstreamClient,
    gate: Gate,
    timeout: Duration,
    max_bytes: usize,
}

impl std::fmt::Debug for OpenMeteoProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenMeteoProvider")
            .field("forecast_days", &self.forecast_days)
            .field("gate", &self.gate)
            .finish_non_exhaustive()
    }
}

impl OpenMeteoProvider {
    pub fn new(
        base: reqwest::Url,
        api_key: Option<String>,
        forecast_days: u8,
        client: UpstreamClient,
        gate: Gate,
        timeout: Duration,
        max_bytes: usize,
    ) -> Self {
        Self {
            base,
            api_key,
            forecast_days,
            client,
            gate,
            timeout,
            max_bytes,
        }
    }
}

pub fn forecast_url(
    base: &reqwest::Url,
    point: RoundedPoint,
    forecast_days: u8,
    api_key: Option<&str>,
) -> reqwest::Url {
    let mut url = base.clone();
    url.path_segments_mut()
        .expect("接続先は設定の検査でhttp(s)のURLに限っている")
        .pop_if_empty()
        .extend(["v1", "forecast"]);
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("latitude", &point.lat_text())
            .append_pair("longitude", &point.lon_text())
            .append_pair("current", "weather_code")
            .append_pair("hourly", "weather_code")
            .append_pair("timezone", "Asia/Tokyo")
            .append_pair("forecast_days", &forecast_days.to_string());
        if let Some(key) = api_key {
            query.append_pair("apikey", key);
        }
    }
    url
}

impl WeatherProvider for OpenMeteoProvider {
    fn fetch<'a>(
        &'a self,
        point: RoundedPoint,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<WeatherReport, WeatherError>> {
        Box::pin(async move {
            let lease = self.gate.acquire(cancel).await.map_err(|e| match e {
                GateError::Busy => WeatherError::Busy,
                GateError::Interrupted(i) => WeatherError::Interrupted(i),
            })?;
            cancel.check()?;
            lease.ensure_valid().map_err(|_| WeatherError::Busy)?;
            let url = forecast_url(
                &self.base,
                point,
                self.forecast_days,
                self.api_key.as_deref(),
            );
            let fetched = self
                .client
                .get(
                    url,
                    self.timeout.min(lease.remaining()),
                    self.max_bytes,
                    cancel,
                )
                .await;
            lease.release().await;
            let body = fetched.map_err(|e| match e {
                FetchError::Failed => WeatherError::Failed,
                FetchError::Interrupted(i) => WeatherError::Interrupted(i),
            })?;
            parse_response(&body).ok_or(WeatherError::Failed)
        })
    }
}

/// 応答を検証して解釈する。`utc_offset_seconds=32400`、`current.time`・`current.weather_code`、
/// 同じ長さの `hourly.time`・`hourly.weather_code` を要求する。
pub fn parse_response(body: &[u8]) -> Option<WeatherReport> {
    let value: Value = serde_json::from_slice(body).ok()?;
    if value.get("utc_offset_seconds")?.as_i64()? != TOKYO_OFFSET_SECONDS {
        return None;
    }
    let current = value.get("current")?;
    let current_time = parse_time(current.get("time")?)?;
    let current_code = parse_code(current.get("weather_code")?)?;
    let hourly = value.get("hourly")?;
    let times = hourly.get("time")?.as_array()?;
    let codes = hourly.get("weather_code")?.as_array()?;
    if times.len() != codes.len() {
        return None;
    }
    let hourly = times
        .iter()
        .zip(codes)
        .map(|(t, c)| Some((parse_time(t)?, parse_code(c)?)))
        .collect::<Option<Vec<_>>>()?;
    Some(WeatherReport {
        current_time,
        current_code,
        hourly,
    })
}

/// `YYYY-MM-DDThh:mm`（秒付きも可）のローカル時刻。
fn parse_time(v: &Value) -> Option<NaiveDateTime> {
    let s = v.as_str()?;
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S"))
        .ok()
}

/// WMOコードは非負の整数（`3.0` 等の整数値の小数表記も可）。
fn parse_code(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        return (n >= 0).then_some(n);
    }
    let f = v.as_f64()?;
    (f.is_finite() && f.fract() == 0.0 && (0.0..=1.0e6).contains(&f)).then_some(f as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_applies_to_the_url() {
        let base = reqwest::Url::parse("http://127.0.0.1:9/").unwrap();
        let point = RoundedPoint::round(35.678, 139.7649);
        assert_eq!(point, RoundedPoint::round(35.68, 139.76));
        assert_eq!(
            forecast_url(&base, point, 7, None).as_str(),
            "http://127.0.0.1:9/v1/forecast?latitude=35.68&longitude=139.76&current=weather_code\
             &hourly=weather_code&timezone=Asia%2FTokyo&forecast_days=7"
        );
        assert_eq!(RoundedPoint::round(-0.05, 139.0).lat_text(), "-0.05");
    }

    #[test]
    fn response_shape_is_checked() {
        let ok = br#"{"utc_offset_seconds":32400,
            "current":{"time":"2026-10-24T09:00","weather_code":3},
            "hourly":{"time":["2026-10-24T09:00","2026-10-24T10:00"],"weather_code":[3,61]}}"#;
        let report = parse_response(ok).unwrap();
        assert_eq!(report.current_code, 3);
        assert_eq!(report.hourly.len(), 2);
        let wrong_offset = br#"{"utc_offset_seconds":0,
            "current":{"time":"2026-10-24T09:00","weather_code":3},
            "hourly":{"time":[],"weather_code":[]}}"#;
        assert_eq!(parse_response(wrong_offset), None);
        let length_mismatch = br#"{"utc_offset_seconds":32400,
            "current":{"time":"2026-10-24T09:00","weather_code":3},
            "hourly":{"time":["2026-10-24T09:00"],"weather_code":[]}}"#;
        assert_eq!(parse_response(length_mismatch), None);
    }
}
