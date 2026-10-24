//! `GET /api/geocode`（behavior.md 10章）。
//!
//! qは1個必須、trim後のUnicode文字数2〜200。受付からJSON準備まで要求全体の期限
//! （`REQUEST_TIMEOUT_MS`）に収め、期限切れは503 service_busy。キャッシュしない。

use axum::extract::{RawQuery, State};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::api::AppState;
use crate::api::error::{ApiError, GEOCODING_UNAVAILABLE, json_ok};
use crate::api::query::QueryParams;
use crate::config::Attribution;
use crate::contract::{CancelToken, Interrupt};
use crate::providers::geoapify::{GeocodeError, Place};

pub const INVALID_Q: &str = "検索語qは1個だけ指定し、前後の空白を除いて2〜200文字にしてください。";

const MIN_CHARS: usize = 2;
const MAX_CHARS: usize = 200;

#[derive(Serialize)]
struct GeocodeDto<'a> {
    results: Vec<ResultDto<'a>>,
    attribution: Vec<AttributionDto<'a>>,
}

#[derive(Serialize)]
struct ResultDto<'a> {
    lat: f64,
    lon: f64,
    label: &'a str,
}

#[derive(Serialize)]
struct AttributionDto<'a> {
    text: &'a str,
    url: &'a str,
}

/// trim済みのqを取り出す。
pub fn parse_q(query: Option<&str>) -> Result<String, ApiError> {
    let params = QueryParams::parse(query, &["q"]);
    let Ok(Some(q)) = params.single("q") else {
        return Err(ApiError::InvalidRequest(INVALID_Q));
    };
    let q = q.trim();
    let chars = q.chars().count();
    if !(MIN_CHARS..=MAX_CHARS).contains(&chars) {
        return Err(ApiError::InvalidRequest(INVALID_Q));
    }
    Ok(q.to_owned())
}

pub async fn handle(State(state): State<AppState>, RawQuery(query): RawQuery) -> Response {
    let q = match parse_q(query.as_deref()) {
        Ok(q) => q,
        Err(error) => return error.into_response(),
    };
    let cancel = CancelToken::with_timeout(state.settings().request_timeout);
    let _guard = cancel.cancel_on_drop();
    let deadline =
        tokio::time::Instant::from_std(cancel.deadline().expect("要求ごとのトークンは期限付き"));
    let result = tokio::time::timeout_at(deadline, state.geocoder().search(&q, &cancel))
        .await
        .unwrap_or(Err(GeocodeError::Interrupted(Interrupt::DeadlineExceeded)));
    match cancel.settle(result) {
        Ok(places) => respond(&places, &state.settings().geocoding_attribution),
        Err(GeocodeError::ExternalApi) => {
            ApiError::ExternalApi(GEOCODING_UNAVAILABLE).into_response()
        }
        Err(GeocodeError::Busy | GeocodeError::Interrupted(_)) => {
            ApiError::ServiceBusy.into_response()
        }
    }
}

fn respond(places: &[Place], attribution: &[Attribution]) -> Response {
    json_ok(&GeocodeDto {
        results: places
            .iter()
            .map(|p| ResultDto {
                lat: p.position.lat,
                lon: p.position.lon,
                label: &p.label,
            })
            .collect(),
        attribution: attribution
            .iter()
            .map(|a| AttributionDto {
                text: &a.text,
                url: &a.url,
            })
            .collect(),
    })
}
