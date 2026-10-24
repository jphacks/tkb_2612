//! アクセスログと、ハンドラのpanicの500変換。
//!
//! 記録するのはメソッド・ルートのテンプレート・status・処理時間・エラーcodeだけで、
//! クエリ・本文・住所・座標・ヘッダーは記録しない。

use std::panic::AssertUnwindSafe;
use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use futures_util::FutureExt;

use crate::api::error::{ApiError, ErrorCode};

pub async fn access_log(request: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().clone();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "(unmatched)".to_owned(), |p| p.as_str().to_owned());
    let response = match AssertUnwindSafe(next.run(request)).catch_unwind().await {
        Ok(response) => response,
        Err(_) => {
            tracing::error!(%method, %route, "ハンドラが異常終了しました");
            ApiError::Internal.into_response()
        }
    };
    let code = response.extensions().get::<ErrorCode>().map(|c| c.0);
    tracing::info!(
        target: "access",
        %method,
        %route,
        status = response.status().as_u16(),
        ms = started.elapsed().as_millis() as u64,
        code = code.unwrap_or("-"),
    );
    response
}
