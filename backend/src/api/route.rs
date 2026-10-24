//! `POST /api/route`（behavior.md 4〜8章、backend-spec.md 7章）。
//!
//! 受付から応答JSON準備（gzipを許可する要求は圧縮）までを要求全体の期限
//! （`REQUEST_TIMEOUT_MS`）に収める。期限切れは段階によらず503 service_busyで、
//! 期限後に得た結果は [`CancelToken::settle`] で捨てる。要求のfutureが破棄されたとき
//! （クライアント切断）は [`CancelGuard`](crate::contract::CancelGuard) で集計も止める。
//! 本文の転送は [`TransferBody`] が送信開始から `ROUTE_TRANSFER_TIMEOUT_MS` で打ち切る。

use axum::Extension;
use axum::body::Body;
use axum::extract::State;
use axum::http::header::{CONNECTION, CONTENT_ENCODING, HeaderValue, VARY};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;

use crate::api::AppState;
use crate::api::compress::{accepts_gzip, gzip};
use crate::api::error::{ApiError, insert_api_headers};
use crate::api::route_request::{
    BODY_SIZE, CONTENT_TYPE_MESSAGE, MAX_BODY_BYTES, is_json_content_type, parse_route_request,
};
use crate::api::route_response::render_route_response;
use crate::api::transfer::{ConnectionKill, TransferBody};
use crate::contract::CancelToken;

/// 応答本文の準備結果。
struct Prepared {
    body: Vec<u8>,
    gzip: bool,
}

pub async fn handle(
    State(state): State<AppState>,
    kill: Option<Extension<ConnectionKill>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let cancel = CancelToken::with_timeout(state.settings().request_timeout);
    let guard = cancel.cancel_on_drop();
    let result = prepare(&state, &headers, body, &cancel).await;
    match cancel.settle(result) {
        Ok(prepared) => {
            guard.disarm();
            success(&state, prepared, kill.as_ref().map(|k| &k.0))
        }
        // guardの破棄で、期限後もまだ動いている集計を止める
        Err(error) => error.into_response(),
    }
}

async fn prepare(
    state: &AppState,
    headers: &HeaderMap,
    body: Body,
    cancel: &CancelToken,
) -> Result<Prepared, ApiError> {
    let engine = state.engine().cloned().ok_or(ApiError::DataUnavailable)?;
    // 応答の準備が終わるまで受付枠を持つ
    let _admission = state
        .route_admission()
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::ServiceBusy)?;
    if !is_json_content_type(headers) {
        return Err(ApiError::InvalidRequest(CONTENT_TYPE_MESSAGE));
    }
    let deadline =
        tokio::time::Instant::from_std(cancel.deadline().expect("要求ごとのトークンは期限付き"));
    let bytes = tokio::time::timeout_at(deadline, axum::body::to_bytes(body, MAX_BODY_BYTES))
        .await
        .map_err(|_| ApiError::ServiceBusy)?
        .map_err(|_| ApiError::InvalidRequest(BODY_SIZE))?;
    let query = parse_route_request(&bytes).map_err(ApiError::InvalidRequest)?;
    let gzip_requested = accepts_gzip(headers);

    let work = async {
        let candidates = engine
            .collect_candidates(state.router().as_ref(), &query, cancel)
            .await?;
        let permit = state
            .cpu()
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ApiError::Internal)?;
        cancel.check()?;
        let engine = engine.clone();
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<u8>, ApiError> {
            let _permit = permit;
            let outcome = engine.evaluate(&query, candidates, &cancel)?;
            let body = render_route_response(&outcome).map_err(|e| {
                tracing::error!(problem = %e.0, "route応答の検査に失敗しました");
                ApiError::Internal
            })?;
            if gzip_requested {
                Ok(gzip(&body, &cancel)?)
            } else {
                Ok(body)
            }
        })
        .await
        .map_err(|_| ApiError::Internal)?
    };
    let body = tokio::time::timeout_at(deadline, work)
        .await
        .map_err(|_| ApiError::ServiceBusy)??;
    Ok(Prepared {
        body,
        gzip: gzip_requested,
    })
}

fn success(state: &AppState, prepared: Prepared, kill: Option<&ConnectionKill>) -> Response {
    let body = TransferBody::new(
        Bytes::from(prepared.body),
        state.settings().route_transfer_timeout,
        kill,
    );
    let mut response = (StatusCode::OK, Body::new(body)).into_response();
    let headers = response.headers_mut();
    insert_api_headers(headers);
    headers.insert(VARY, HeaderValue::from_static("accept-encoding"));
    if prepared.gzip {
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    // 転送期限で接続ごと切るので、同じ接続で次の要求を受けない
    headers.insert(CONNECTION, HeaderValue::from_static("close"));
    response
}
