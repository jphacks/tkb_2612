//! 住所検索のアダプタ（Geoapify Forward Geocoding、behavior.md 10章）。
//!
//! `{GEOAPIFY_BASE_URL}/v1/geocode/search?text=&lang=ja&filter=countrycode:jp&limit=5&format=json&apiKey=`
//! を上流ゲートを通して送る。キャッシュしない（同じqでも毎回通信する）。

use std::time::Duration;

use serde_json::Value;

use crate::contract::{BoxFuture, CancelToken, Interrupt, LatLon};
use crate::providers::gate::{Gate, GateError};
use crate::providers::http::{FetchError, UpstreamClient};

/// 返す候補の上限。
pub const MAX_PLACES: usize = 5;

/// 住所候補1件。
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub position: LatLon,
    pub label: String,
}

/// 住所検索の失敗。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GeocodeError {
    /// 通信・HTTP（429を含む）・期限・サイズ・形式の失敗、日本矩形外の候補。502。
    ExternalApi,
    /// 待ち行列の満杯・共有上流ゲートの失敗。503。
    Busy,
    Interrupted(Interrupt),
}

impl From<Interrupt> for GeocodeError {
    fn from(value: Interrupt) -> Self {
        GeocodeError::Interrupted(value)
    }
}

/// 住所検索の提供元。
pub trait Geocoder: Send + Sync {
    /// trim済みの `q` で検索し、提供元順の先頭5件までを返す。
    fn search<'a>(
        &'a self,
        q: &'a str,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<Vec<Place>, GeocodeError>>;
}

#[derive(Clone)]
pub struct GeoapifyGeocoder {
    base: reqwest::Url,
    api_key: String,
    client: UpstreamClient,
    gate: Gate,
    timeout: Duration,
    max_bytes: usize,
}

impl std::fmt::Debug for GeoapifyGeocoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 鍵を表示しない
        f.debug_struct("GeoapifyGeocoder")
            .field("gate", &self.gate)
            .finish_non_exhaustive()
    }
}

impl GeoapifyGeocoder {
    pub fn new(
        base: reqwest::Url,
        api_key: String,
        client: UpstreamClient,
        gate: Gate,
        timeout: Duration,
        max_bytes: usize,
    ) -> Self {
        Self {
            base,
            api_key,
            client,
            gate,
            timeout,
            max_bytes,
        }
    }

    fn url(&self, q: &str) -> reqwest::Url {
        search_url(&self.base, q, &self.api_key)
    }
}

pub fn search_url(base: &reqwest::Url, q: &str, api_key: &str) -> reqwest::Url {
    let mut url = base.clone();
    url.path_segments_mut()
        .expect("接続先は設定の検査でhttp(s)のURLに限っている")
        .pop_if_empty()
        .extend(["v1", "geocode", "search"]);
    url.query_pairs_mut()
        .append_pair("text", q)
        .append_pair("lang", "ja")
        .append_pair("filter", "countrycode:jp")
        .append_pair("limit", "5")
        .append_pair("format", "json")
        .append_pair("apiKey", api_key);
    url
}

impl Geocoder for GeoapifyGeocoder {
    fn search<'a>(
        &'a self,
        q: &'a str,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<Vec<Place>, GeocodeError>> {
        Box::pin(async move {
            let lease = self.gate.acquire(cancel).await.map_err(gate_error)?;
            cancel.check()?;
            lease.ensure_valid().map_err(gate_error)?;
            let fetched = self
                .client
                .get(self.url(q), self.timeout, self.max_bytes, cancel)
                .await;
            lease.release().await;
            let body = fetched.map_err(|e| match e {
                FetchError::Failed => GeocodeError::ExternalApi,
                FetchError::Interrupted(i) => GeocodeError::Interrupted(i),
            })?;
            parse_response(&body)
        })
    }
}

fn gate_error(e: GateError) -> GeocodeError {
    match e {
        GateError::Busy => GeocodeError::Busy,
        GateError::Interrupted(i) => GeocodeError::Interrupted(i),
    }
}

/// `format=json` の応答 `{"results":[{"lat","lon","formatted"},…]}` を解釈する。
/// 先頭5件のどれかが欠落・型不正・日本矩形外なら全体を失敗にする。
pub fn parse_response(body: &[u8]) -> Result<Vec<Place>, GeocodeError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| GeocodeError::ExternalApi)?;
    let results = value
        .get("results")
        .and_then(Value::as_array)
        .ok_or(GeocodeError::ExternalApi)?;
    results
        .iter()
        .take(MAX_PLACES)
        .map(|r| {
            let lat = r.get("lat").and_then(Value::as_f64);
            let lon = r.get("lon").and_then(Value::as_f64);
            let label = r.get("formatted").and_then(Value::as_str);
            match (lat, lon, label) {
                (Some(lat), Some(lon), Some(label)) => {
                    let position = LatLon::new(lat, lon);
                    if !position.is_in_japan() {
                        return Err(GeocodeError::ExternalApi);
                    }
                    Ok(Place {
                        position,
                        label: label.to_owned(),
                    })
                }
                _ => Err(GeocodeError::ExternalApi),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_is_percent_encoded() {
        let base = reqwest::Url::parse("http://127.0.0.1:9").unwrap();
        assert_eq!(
            search_url(&base, "東京駅 丸の内", "k&y").as_str(),
            "http://127.0.0.1:9/v1/geocode/search?text=%E6%9D%B1%E4%BA%AC%E9%A7%85+%E4%B8%B8%E3%81%AE%E5%86%85\
             &lang=ja&filter=countrycode%3Ajp&limit=5&format=json&apiKey=k%26y"
        );
    }

    #[test]
    fn results_are_limited_and_validated() {
        let body = br#"{"results":[
            {"lat":35.1,"lon":139.1,"formatted":"a"},{"lat":35.2,"lon":139.2,"formatted":"b"},
            {"lat":35.3,"lon":139.3,"formatted":"c"},{"lat":35.4,"lon":139.4,"formatted":"d"},
            {"lat":35.5,"lon":139.5,"formatted":"e"},{"lat":0,"lon":0,"formatted":"f"}]}"#;
        let places = parse_response(body).unwrap();
        assert_eq!(places.len(), 5);
        assert_eq!(places[4].label, "e");
        assert_eq!(
            parse_response(br#"{"results":[{"lat":0,"lon":0,"formatted":"x"}]}"#),
            Err(GeocodeError::ExternalApi)
        );
        assert_eq!(
            parse_response(br#"{"results":[{"lat":"35","lon":139,"formatted":"x"}]}"#),
            Err(GeocodeError::ExternalApi)
        );
        assert_eq!(parse_response(br#"{"results":[]}"#), Ok(vec![]));
        assert_eq!(parse_response(b"{}"), Err(GeocodeError::ExternalApi));
    }
}
