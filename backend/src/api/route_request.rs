//! routeの入力の検証・正規化・互換入力（behavior.md 4章、backend-spec.md 2章）。
//!
//! 検証順はJSONオブジェクト→origin/dest→直線距離→mode→max_detour_min→period→hour→
//! weather→day_typeで、最初の不正を返す。外部呼出しの前に行う。

use std::fmt;

use axum::http::HeaderMap;
use axum::http::header::CONTENT_TYPE;
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};

use crate::api::route_response::MAX_SAFE_INTEGER;
use crate::contract::engine::{DEFAULT_MAX_DETOUR_MIN, validate_endpoints, validate_max_detour};
use crate::contract::{
    ConditionFilter, DayType, LatLon, Mode, Period, QueryError, RouteQuery, Weather,
};

/// 本文の上限（バイト、両端を含む）。
pub const MAX_BODY_BYTES: usize = 16_384;

pub const BODY_SIZE: &str = "リクエスト本文は1〜16384バイトにしてください。";
pub const CONTENT_TYPE_MESSAGE: &str = "Content-Typeはapplication/jsonにしてください。";
pub const INVALID_JSON: &str = "リクエスト本文がUTF-8のJSONとして解釈できません。";
pub const DUPLICATE_KEY: &str = "JSONに重複したキーがあります。";
pub const NOT_OBJECT: &str = "リクエスト本文はJSONオブジェクトにしてください。";
pub const INVALID_MODE: &str = "移動手段はwalk・bike・carのいずれかを指定してください。";
pub const INVALID_PERIOD: &str =
    "時間帯は早朝・昼・夕・夜・深夜・allのいずれかを指定してください。";
pub const INVALID_HOUR: &str = "hourは0〜23の整数で指定してください。";
pub const HOUR_CONFLICT: &str = "hourと時間帯の指定が一致しません。";
pub const INVALID_WEATHER: &str = "天候は晴・曇・雨・霧・雪・allのいずれかを指定してください。";
pub const INVALID_DAY_TYPE: &str = "曜日区分は平日・休日・allのいずれかを指定してください。";

/// Content-Typeが `application/json`（charsetを付けるならUTF-8）か。
pub fn is_json_content_type(headers: &HeaderMap) -> bool {
    let mut values = headers.get_all(CONTENT_TYPE).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    let mut parts = value.split(';');
    let media = parts.next().unwrap_or_default().trim();
    if !media.eq_ignore_ascii_case("application/json") {
        return false;
    }
    parts.all(|param| {
        let Some((name, v)) = param.split_once('=') else {
            return false;
        };
        let v = v.trim().trim_matches('"');
        !name.trim().eq_ignore_ascii_case("charset")
            || v.eq_ignore_ascii_case("utf-8")
            || v.eq_ignore_ascii_case("utf8")
    })
}

/// 本文を解釈して `RouteQuery` を作る。エラーは400のmessage。
pub fn parse_route_request(body: &[u8]) -> Result<RouteQuery, &'static str> {
    if body.is_empty() || body.len() > MAX_BODY_BYTES {
        return Err(BODY_SIZE);
    }
    let value = parse_strict_json(body)?;
    let Value::Object(object) = value else {
        return Err(NOT_OBJECT);
    };
    let origin = lat_lon(object.get("origin")).ok_or(QueryError::InvalidOrigin.message())?;
    let dest = lat_lon(object.get("dest")).ok_or(QueryError::InvalidDest.message())?;
    validate_endpoints(origin, dest).map_err(QueryError::message)?;
    let mode = match object.get("mode") {
        None => Mode::Walk,
        Some(v) => v.as_str().and_then(Mode::parse).ok_or(INVALID_MODE)?,
    };
    let max_detour_min = match object.get("max_detour_min") {
        None => DEFAULT_MAX_DETOUR_MIN,
        Some(v) => number(v).ok_or(QueryError::MaxDetour.message())?,
    };
    validate_max_detour(max_detour_min).map_err(QueryError::message)?;
    // キー `period` が存在すれば `時間帯` より優先する（nullでも優先）
    let period = match object.get("period").or_else(|| object.get("時間帯")) {
        None => None,
        Some(v) => coded(v, Period::from_label, Period::from_code).ok_or(INVALID_PERIOD)?,
    };
    let period = match object.get("hour") {
        None | Some(Value::Null) => period,
        Some(v) => {
            let hour = integer(v)
                .filter(|h| (0..=23).contains(h))
                .ok_or(INVALID_HOUR)?;
            let from_hour = Period::from_hour(hour as u8).expect("0〜23");
            match period {
                None => Some(from_hour),
                Some(p) if p == from_hour => Some(p),
                Some(_) => return Err(HOUR_CONFLICT),
            }
        }
    };
    let weather = match object.get("weather") {
        None => None,
        Some(v) => coded(v, Weather::from_label, Weather::from_code).ok_or(INVALID_WEATHER)?,
    };
    let day_type = match object.get("day_type") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s == "all" => None,
        Some(Value::String(s)) => Some(DayType::from_label(s).ok_or(INVALID_DAY_TYPE)?),
        Some(_) => return Err(INVALID_DAY_TYPE),
    };
    RouteQuery::new(
        origin,
        dest,
        mode,
        max_detour_min,
        ConditionFilter {
            period,
            day_type,
            weather,
        },
    )
    .map_err(QueryError::message)
}

/// `{lat, lon}`（有限数値。数値文字列は不可。未知キーは無視）。
fn lat_lon(v: Option<&Value>) -> Option<LatLon> {
    let object = v?.as_object()?;
    Some(LatLon::new(
        number(object.get("lat")?)?,
        number(object.get("lon")?)?,
    ))
}

/// JSONの数値（boolや文字列は数値にしない）。
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64().filter(|f| f.is_finite()),
        _ => None,
    }
}

/// 数学的な整数（`12.0` を含む）。小数表記は、浮動小数点で整数を正確に表せる
/// JSONの安全整数の範囲（絶対値2^53−1以下）だけを受け付ける。範囲の検査は呼出し側で行う。
fn integer(v: &Value) -> Option<i64> {
    let Value::Number(n) = v else {
        return None;
    };
    if let Some(i) = n.as_i64() {
        return Some(i);
    }
    let f = n.as_f64()?;
    (f.is_finite() && f.fract() == 0.0 && f.abs() <= MAX_SAFE_INTEGER as f64).then_some(f as i64)
}

/// 日本語ラベル・`all`・整数コード・null。外側の `None` は不正、内側の `None` はall。
fn coded<T>(
    v: &Value,
    from_label: fn(&str) -> Option<T>,
    from_code: fn(u8) -> Option<T>,
) -> Option<Option<T>> {
    match v {
        Value::Null => Some(None),
        Value::String(s) if s == "all" => Some(None),
        Value::String(s) => from_label(s).map(Some),
        Value::Number(_) => {
            let code = u8::try_from(integer(v)?).ok()?;
            from_code(code).map(Some)
        }
        _ => None,
    }
}

/// 重複キーを拒否するJSONの解析。`serde_json::Value` は重複キーを後勝ちで受けるため。
pub fn parse_strict_json(body: &[u8]) -> Result<Value, &'static str> {
    match serde_json::from_slice::<StrictValue>(body) {
        Ok(StrictValue(v)) => Ok(v),
        Err(e) if e.to_string().starts_with(DUPLICATE_MARKER) => Err(DUPLICATE_KEY),
        Err(_) => Err(INVALID_JSON),
    }
}

const DUPLICATE_MARKER: &str = "duplicate JSON key";

struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(StrictVisitor).map(StrictValue)
    }
}

struct StrictVisitor;

impl<'de> Visitor<'de> for StrictVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JSON value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::Number(v.into()))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Value, E> {
        Number::from_f64(v)
            .map(Value::Number)
            .ok_or_else(|| E::custom("non-finite number"))
    }

    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }

    fn visit_string<E>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(StrictValue(v)) = seq.next_element()? {
            items.push(v);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut object = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(de::Error::custom(DUPLICATE_MARKER));
            }
            let StrictValue(v) = map.next_value()?;
            object.insert(key, v);
        }
        Ok(Value::Object(object))
    }
}
