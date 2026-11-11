//! 安全危険情報の共有（Issue #23）：`/api/reports`。
//!
//! - `GET /api/reports?bbox=最小経度,最小緯度,最大経度,最大緯度`：範囲内の地点（新しい順に最大500件）。
//!   範囲は緯度・経度とも1度以内（超えたら400 invalid_request）
//! - `POST /api/reports`：地点を作り、作成者の1票（と任意の口コミ）を入れる。201
//! - `POST /api/reports/{id}/votes`：既存の地点に1票（と任意の口コミ）を入れる
//!
//! 投票者は `X-Voter-Id` ヘッダーの匿名ID（ブラウザーごとの乱数）で区別し、同じ地点への
//! 2回目の投票は409 already_voted。GETでも送れば、その投票者の票（`my_vote`）を返す。
//! 保存先の失敗・期限切れは503 service_busy。

use axum::body::Body;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, SecondsFormat};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::api::AppState;
use crate::api::error::{ApiError, json_response};
use crate::api::query::QueryParams;
use crate::api::route_request::{
    CONTENT_TYPE_MESSAGE, NOT_OBJECT, is_json_content_type, parse_strict_json,
};
use crate::providers::community::{
    Ballot, Bounds, CommunityError, MAX_TEXT_CHARS, SafetyKind, StoredReport, VoterId,
    is_report_id, new_report,
};

/// 投票者IDのヘッダー。
pub const VOTER_HEADER: &str = "x-voter-id";
/// 投稿・投票の本文の上限。
pub const MAX_BODY_BYTES: usize = 8_192;

pub const BODY_SIZE: &str = "リクエスト本文は1〜8192バイトにしてください。";
pub const INVALID_BBOX: &str =
    "bboxは「最小経度,最小緯度,最大経度,最大緯度」の数値で1個だけ指定してください。";
pub const BBOX_TOO_WIDE: &str =
    "表示範囲が広すぎます。緯度・経度とも1度以内の範囲で指定してください。";
pub const INVALID_VOTER: &str =
    "投票者IDを確認できません。ページを再読み込みして再試行してください。";
pub const INVALID_POSITION: &str =
    "地点は日本国内（北緯20〜46度、東経122〜154度）の数値で指定してください。";
pub const INVALID_KIND: &str = "評価はsafe（安全）またはdanger（危険）を指定してください。";
pub const INVALID_TEXT: &str = "口コミは500文字以内の文字列で指定してください。";
pub const UNKNOWN_FIELD: &str = "リクエスト本文に未対応の項目があります。";

/// 一覧で指定できる範囲の幅の上限（緯度・経度とも、十進度）。広い範囲で保存先を
/// 読みすぎないようにする。地図を引いて超えたら、フロントは拡大を促して読まない。
pub const MAX_BBOX_SPAN_DEG: f64 = 1.0;

/// 投稿できる地点の範囲（OpenAPIの `Latitude`・`Longitude`）。
const LAT_RANGE: std::ops::RangeInclusive<f64> = 20.0..=46.0;
const LON_RANGE: std::ops::RangeInclusive<f64> = 122.0..=154.0;

#[derive(Serialize)]
struct ListDto<'a> {
    reports: Vec<ReportDto<'a>>,
}

#[derive(Serialize)]
struct OneDto<'a> {
    report: ReportDto<'a>,
}

#[derive(Serialize)]
struct ReportDto<'a> {
    id: &'a str,
    lat: f64,
    lon: f64,
    created_at: String,
    safe_count: usize,
    danger_count: usize,
    my_vote: Option<&'static str>,
    comments: Vec<CommentDto<'a>>,
}

#[derive(Serialize)]
struct CommentDto<'a> {
    id: &'a str,
    kind: &'static str,
    text: &'a str,
    created_at: String,
}

/// UNIXミリ秒をUTCのISO 8601（ミリ秒まで）にする。
fn timestamp(ms: i64) -> String {
    DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn dto<'a>(report: &'a StoredReport, voter: Option<&VoterId>) -> ReportDto<'a> {
    ReportDto {
        id: &report.id,
        lat: report.lat,
        lon: report.lon,
        created_at: timestamp(report.created_at_ms),
        safe_count: report.count(SafetyKind::Safe),
        danger_count: report.count(SafetyKind::Danger),
        my_vote: voter
            .and_then(|v| report.vote_of(v))
            .map(SafetyKind::as_str),
        comments: report
            .comments
            .iter()
            .map(|c| CommentDto {
                id: &c.id,
                kind: c.kind.as_str(),
                text: &c.text,
                created_at: timestamp(c.at_ms),
            })
            .collect(),
    }
}

fn body_response<T: Serialize>(status: StatusCode, value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => json_response(status, body),
        Err(_) => ApiError::Internal.into_response(),
    }
}

impl From<CommunityError> for ApiError {
    fn from(value: CommunityError) -> Self {
        match value {
            CommunityError::NotFound => ApiError::ReportNotFound,
            CommunityError::AlreadyVoted => ApiError::AlreadyVoted,
            CommunityError::Full => ApiError::VoteLimit,
            CommunityError::Unavailable => ApiError::ServiceBusy,
        }
    }
}

/// `bbox` を解釈する。経度・緯度は世界の範囲で、最小≦最大、幅は [`MAX_BBOX_SPAN_DEG`] 以内。
pub fn parse_bbox(query: Option<&str>) -> Result<Bounds, ApiError> {
    let invalid = || ApiError::InvalidRequest(INVALID_BBOX);
    let params = QueryParams::parse(query, &["bbox"]);
    let Ok(Some(bbox)) = params.single("bbox") else {
        return Err(invalid());
    };
    let values: Vec<f64> = bbox
        .split(',')
        .map(|v| v.trim().parse::<f64>().ok().filter(|f| f.is_finite()))
        .collect::<Option<_>>()
        .ok_or_else(invalid)?;
    let [min_lon, min_lat, max_lon, max_lat] = values[..] else {
        return Err(invalid());
    };
    let lon_ok = |v: f64| (-180.0..=180.0).contains(&v);
    let lat_ok = |v: f64| (-90.0..=90.0).contains(&v);
    if !(lon_ok(min_lon) && lon_ok(max_lon) && lat_ok(min_lat) && lat_ok(max_lat))
        || min_lon > max_lon
        || min_lat > max_lat
    {
        return Err(invalid());
    }
    // 十進数の引き算の誤差（140.7 - 139.7 > 1.0 等）でちょうど1度を拒否しない
    let too_wide = |span: f64| span > MAX_BBOX_SPAN_DEG + 1e-9;
    if too_wide(max_lon - min_lon) || too_wide(max_lat - min_lat) {
        return Err(ApiError::InvalidRequest(BBOX_TOO_WIDE));
    }
    Ok(Bounds {
        min_lat,
        min_lon,
        max_lat,
        max_lon,
    })
}

/// 投票者ID。無い・形式違いは `None`。
fn voter(headers: &HeaderMap) -> Option<VoterId> {
    let mut values = headers.get_all(VOTER_HEADER).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return None;
    };
    VoterId::from_client(value.to_str().ok()?)
}

/// 本文のJSONオブジェクト（`allowed` 以外のキーは400）。
fn parse_object(bytes: &[u8], allowed: &[&str]) -> Result<Map<String, Value>, ApiError> {
    if bytes.is_empty() || bytes.len() > MAX_BODY_BYTES {
        return Err(ApiError::InvalidRequest(BODY_SIZE));
    }
    let Value::Object(object) = parse_strict_json(bytes).map_err(ApiError::InvalidRequest)? else {
        return Err(ApiError::InvalidRequest(NOT_OBJECT));
    };
    if object.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(ApiError::InvalidRequest(UNKNOWN_FIELD));
    }
    Ok(object)
}

/// `kind` と任意の `text`（文字列かnull。trim後に空ならなし）。
fn parse_ballot(object: &Map<String, Value>) -> Result<Ballot, ApiError> {
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .and_then(SafetyKind::parse)
        .ok_or(ApiError::InvalidRequest(INVALID_KIND))?;
    let text = match object.get("text") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => {
            let text = text.trim();
            if text.chars().count() > MAX_TEXT_CHARS {
                return Err(ApiError::InvalidRequest(INVALID_TEXT));
            }
            (!text.is_empty()).then(|| text.to_owned())
        }
        Some(_) => return Err(ApiError::InvalidRequest(INVALID_TEXT)),
    };
    Ok(Ballot { kind, text })
}

fn coordinate(
    object: &Map<String, Value>,
    key: &str,
    range: &std::ops::RangeInclusive<f64>,
) -> Result<f64, ApiError> {
    match object.get(key) {
        Some(Value::Number(n)) => n
            .as_f64()
            .filter(|v| v.is_finite() && range.contains(v))
            .ok_or(ApiError::InvalidRequest(INVALID_POSITION)),
        _ => Err(ApiError::InvalidRequest(INVALID_POSITION)),
    }
}

/// 投稿・投票の共通の前処理：Content-Type・投票者・本文の受信。
async fn receive(
    state: &AppState,
    headers: &HeaderMap,
    body: Body,
) -> Result<(VoterId, bytes::Bytes), ApiError> {
    if !is_json_content_type(headers) {
        return Err(ApiError::InvalidRequest(CONTENT_TYPE_MESSAGE));
    }
    let voter = voter(headers).ok_or(ApiError::InvalidRequest(INVALID_VOTER))?;
    let bytes = tokio::time::timeout(
        state.settings().request_timeout,
        axum::body::to_bytes(body, MAX_BODY_BYTES),
    )
    .await
    .map_err(|_| ApiError::ServiceBusy)?
    .map_err(|_| ApiError::InvalidRequest(BODY_SIZE))?;
    Ok((voter, bytes))
}

/// 保存先の呼出しを要求全体の期限に収める。
async fn within_deadline<T>(
    state: &AppState,
    work: impl Future<Output = Result<T, CommunityError>>,
) -> Result<T, ApiError> {
    tokio::time::timeout(state.settings().request_timeout, work)
        .await
        .map_err(|_| ApiError::ServiceBusy)?
        .map_err(ApiError::from)
}

pub async fn list(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    let result = async {
        let bounds = parse_bbox(query.as_deref())?;
        let viewer = voter(&headers);
        let reports = within_deadline(&state, state.community().list(bounds)).await?;
        Ok::<_, ApiError>(body_response(
            StatusCode::OK,
            &ListDto {
                reports: reports.iter().map(|r| dto(r, viewer.as_ref())).collect(),
            },
        ))
    };
    result.await.unwrap_or_else(IntoResponse::into_response)
}

pub async fn create(State(state): State<AppState>, headers: HeaderMap, body: Body) -> Response {
    let result = async {
        let (voter, bytes) = receive(&state, &headers, body).await?;
        let object = parse_object(&bytes, &["lat", "lon", "kind", "text"])?;
        let lat = coordinate(&object, "lat", &LAT_RANGE)?;
        let lon = coordinate(&object, "lon", &LON_RANGE)?;
        let ballot = parse_ballot(&object)?;
        let report = new_report(lat, lon, &voter, ballot, state.clock().now_ms());
        within_deadline(&state, state.community().create(report.clone())).await?;
        Ok::<_, ApiError>(body_response(
            StatusCode::CREATED,
            &OneDto {
                report: dto(&report, Some(&voter)),
            },
        ))
    };
    result.await.unwrap_or_else(IntoResponse::into_response)
}

pub async fn vote(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let result = async {
        if !is_report_id(&id) {
            return Err(ApiError::ReportNotFound);
        }
        let (voter, bytes) = receive(&state, &headers, body).await?;
        let ballot = parse_ballot(&parse_object(&bytes, &["kind", "text"])?)?;
        let report = within_deadline(
            &state,
            state
                .community()
                .vote(&id, &voter, ballot, state.clock().now_ms()),
        )
        .await?;
        Ok::<_, ApiError>(body_response(
            StatusCode::OK,
            &OneDto {
                report: dto(&report, Some(&voter)),
            },
        ))
    };
    result.await.unwrap_or_else(IntoResponse::into_response)
}
