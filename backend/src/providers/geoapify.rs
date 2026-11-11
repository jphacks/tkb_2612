//! 住所検索のアダプタ（Geoapify Forward Geocoding、behavior.md 10章）。
//!
//! `{GEOAPIFY_BASE_URL}/v1/geocode/search?text=&lang=ja&filter=countrycode:jp&limit=5&format=json&apiKey=`
//! を上流ゲートを通して送る。キャッシュしない（同じqでも毎回通信する）。
//! 候補は施設名などの `label` と、都道府県・市区町村・地区を連結した短い `detail` に分ける。

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
    pub detail: Option<String>,
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
        .append_pair("limit", &MAX_PLACES.to_string())
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
                .get(
                    self.url(q),
                    self.timeout.min(lease.remaining()),
                    self.max_bytes,
                    cancel,
                )
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

/// 結果から `key` の文字列をtrimして取り出す。無い・文字列でない・空ならNone。
fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// 施設名・地点名。名前が無ければ住所全体を使う。
fn place_label(result: &Value, formatted: &str) -> String {
    let name = text(result, "name");
    let name = match name {
        Some(name) => result
            .get("other_names")
            .and_then(|other| text(other, "name"))
            .filter(|other| other.starts_with(name))
            .unwrap_or(name),
        None => {
            return text(result, "address_line1")
                .unwrap_or(formatted)
                .to_owned();
        }
    };
    name.replace(';', " ")
}

/// 都道府県・市区町村・地区からなる短い所在地。無ければNone。
fn place_detail(result: &Value, label: &str) -> Option<String> {
    let prefecture = text(result, "county")
        .filter(|county| matches!(county.chars().last(), Some('都' | '道' | '府' | '県')));
    let district = text(result, "district").or_else(|| text(result, "suburb"));
    let parts = [prefecture, text(result, "city"), district]
        .into_iter()
        .flatten()
        .filter(|part| *part != label);
    let joined: String = parts.collect();
    (!joined.is_empty()).then_some(joined)
}

/// `format=json` の応答 `{"results":[{"lat","lon","formatted",…},…]}` を解釈する。
/// label は name（other_names.name が name で始まればその値）の ";" を空白にしたもの、
/// 無ければ address_line1、それも無ければ formatted。detail は都道府県・市区町村・地区のうち
/// label と異なる値を区切りなしで連結する。
/// 先頭5件のどれかで lat・lon・formatted が欠落・型不正・日本矩形外なら全体を失敗にする。
/// それ以外のフィールドの型不正は無視する。
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
            let formatted = r.get("formatted").and_then(Value::as_str);
            match (lat, lon, formatted) {
                (Some(lat), Some(lon), Some(formatted)) => {
                    let position = LatLon::new(lat, lon);
                    if !position.is_in_japan() {
                        return Err(GeocodeError::ExternalApi);
                    }
                    let label = place_label(r, formatted);
                    Ok(Place {
                        position,
                        detail: place_detail(r, &label),
                        label,
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

    /// lat、lon、formatted を補って1件の結果を解釈する（各例ではこれらを省略して書く）。
    fn parse_one(result: serde_json::Value) -> Place {
        let mut result = result;
        let object = result.as_object_mut().expect("結果はobject");
        object
            .entry("lat")
            .or_insert_with(|| serde_json::json!(35.0));
        object
            .entry("lon")
            .or_insert_with(|| serde_json::json!(139.0));
        object
            .entry("formatted")
            .or_insert_with(|| serde_json::json!("提供元の住所"));
        let body = serde_json::json!({"results": [result]});
        let bytes = serde_json::to_vec(&body).unwrap();
        let mut places = parse_response(&bytes).unwrap();
        places.pop().unwrap()
    }

    #[test]
    fn label_and_detail_are_split() {
        let cases = [
            (
                serde_json::json!({
                    "name": "柏",
                    "other_names": {"name": "柏駅"},
                    "county": "千葉県",
                    "city": "柏市",
                    "district": "旭町一丁目",
                }),
                "柏駅",
                Some("千葉県柏市旭町一丁目"),
            ),
            (
                serde_json::json!({
                    "name": "東京駅;総武地下4番線",
                    "city": "千代田区",
                    "district": "丸の内一丁目",
                    "suburb": "丸の内",
                }),
                "東京駅 総武地下4番線",
                Some("千代田区丸の内一丁目"),
            ),
            (
                serde_json::json!({
                    "name": "新幹線試験車両951-1",
                    "county": "国立市",
                    "city": "国分寺市",
                    "suburb": "光町",
                }),
                "新幹線試験車両951-1",
                Some("国分寺市光町"),
            ),
            (
                serde_json::json!({
                    "name": "柏市",
                    "county": "千葉県",
                    "city": "柏市",
                }),
                "柏市",
                Some("千葉県"),
            ),
            (
                serde_json::json!({
                    "address_line1": "Gion 1-1",
                    "county": "福岡市",
                    "city": "福岡",
                }),
                "Gion 1-1",
                Some("福岡"),
            ),
            (
                serde_json::json!({
                    "formatted": "東京都千代田区丸の内1-1",
                }),
                "東京都千代田区丸の内1-1",
                None,
            ),
            (
                serde_json::json!({
                    "name": "柏",
                    "other_names": {"name": "別の駅"},
                }),
                "柏",
                None,
            ),
        ];
        for (result, expected_label, expected_detail) in cases {
            let place = parse_one(result);
            assert_eq!(place.label, expected_label);
            assert_eq!(place.detail.as_deref(), expected_detail);
        }
    }

    #[test]
    fn malformed_optional_fields_fall_back() {
        // 名前が空白のみでも、他の型不正と合わせてフォールバックする
        let place = parse_one(serde_json::json!({
            "name": "   ",
            "other_names": "柏駅",
            "city": 123,
            "formatted": "東京都千代田区丸の内1-1",
        }));
        assert_eq!(place.label, "東京都千代田区丸の内1-1");
        assert_eq!(place.detail, None);

        // 名前が数値でも address_line1 にフォールバックする
        let place = parse_one(serde_json::json!({
            "name": 42,
            "other_names": "柏駅",
            "city": 123,
            "address_line1": "Gion 1-1",
        }));
        assert_eq!(place.label, "Gion 1-1");
        assert_eq!(place.detail, None);

        // other_names が文字列でも name をそのまま使う
        let place = parse_one(serde_json::json!({
            "name": "柏",
            "other_names": "柏駅",
            "city": 123,
        }));
        assert_eq!(place.label, "柏");
        assert_eq!(place.detail, None);
    }
}
