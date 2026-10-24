//! エラー応答 `{"error":{"code","message"}}` と、APIの共通ヘッダー。
//!
//! messageは日本語の利用者向け説明で、住所・座標・秘密・スタックトレース・上流URLを含めない。

use axum::http::header::{ALLOW, CACHE_CONTROL, CONTENT_TYPE, HeaderValue, X_CONTENT_TYPE_OPTIONS};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::contract::{EngineError, Interrupt};

/// アクセスログに出す匿名のエラーcode。応答の拡張に入れる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorCode(pub &'static str);

/// APIのエラー。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    InvalidRequest(&'static str),
    NotFound,
    /// `Allow` に入れるメソッド（`GET` または `POST`）。
    MethodNotAllowed(&'static str),
    RouteTooLong,
    NoRoute,
    /// 提供元ごとの説明文。
    ExternalApi(&'static str),
    DataUnavailable,
    ServiceBusy,
    Internal,
}

pub const ROUTING_UNAVAILABLE: &str =
    "経路サービスに接続できません。時間をおいて再試行してください。";
pub const GEOCODING_UNAVAILABLE: &str =
    "住所検索サービスに接続できません。時間をおいて再試行してください。";

impl ApiError {
    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::NotFound => StatusCode::NOT_FOUND,
            ApiError::MethodNotAllowed(_) => StatusCode::METHOD_NOT_ALLOWED,
            ApiError::RouteTooLong => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::NoRoute | ApiError::ExternalApi(_) => StatusCode::BAD_GATEWAY,
            ApiError::DataUnavailable | ApiError::ServiceBusy => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            ApiError::InvalidRequest(_) => "invalid_request",
            ApiError::NotFound => "not_found",
            ApiError::MethodNotAllowed(_) => "method_not_allowed",
            ApiError::RouteTooLong => "route_too_long",
            ApiError::NoRoute => "no_route",
            ApiError::ExternalApi(_) => "external_api_error",
            ApiError::DataUnavailable => "data_unavailable",
            ApiError::ServiceBusy => "service_busy",
            ApiError::Internal => "internal_error",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            ApiError::InvalidRequest(message) | ApiError::ExternalApi(message) => message,
            ApiError::NotFound => "APIが見つかりません。",
            ApiError::MethodNotAllowed(_) => "このAPIでは指定のメソッドを使用できません。",
            ApiError::RouteTooLong => "候補経路が80kmを超えました。近い地点を指定してください。",
            ApiError::NoRoute => {
                "経路が見つかりませんでした。地点や移動手段を変えて再試行してください。"
            }
            ApiError::DataUnavailable => {
                "事故データを準備しています。時間をおいて再試行してください。"
            }
            ApiError::ServiceBusy => "混み合っています。時間をおいて再試行してください。",
            ApiError::Internal => "処理に失敗しました。時間をおいて再試行してください。",
        }
    }
}

/// routeの計算の失敗（README.mdの対応表）。
impl From<EngineError> for ApiError {
    fn from(value: EngineError) -> Self {
        match value {
            EngineError::NoRoute => ApiError::NoRoute,
            EngineError::RouteTooLong => ApiError::RouteTooLong,
            EngineError::ExternalApi => ApiError::ExternalApi(ROUTING_UNAVAILABLE),
            EngineError::Busy => ApiError::ServiceBusy,
            EngineError::Interrupted(Interrupt::DeadlineExceeded) => ApiError::ServiceBusy,
            // 要求側が取り消した（接続は切断済み）。応答は届かないので503にしておく
            EngineError::Interrupted(Interrupt::Cancelled) => ApiError::ServiceBusy,
            EngineError::ContractViolation(_) => ApiError::Internal,
        }
    }
}

/// 要求全体の期限切れ・取消し。応答段階によらず503。
impl From<Interrupt> for ApiError {
    fn from(_: Interrupt) -> Self {
        ApiError::ServiceBusy
    }
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: ErrorDetail<'a>,
}

#[derive(Serialize)]
struct ErrorDetail<'a> {
    code: &'a str,
    message: &'a str,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::to_vec(&ErrorBody {
            error: ErrorDetail {
                code: self.code(),
                message: self.message(),
            },
        })
        .expect("固定の文字列だけのJSON");
        let mut response = json_response(self.status(), body);
        if let ApiError::MethodNotAllowed(allow) = self {
            response
                .headers_mut()
                .insert(ALLOW, HeaderValue::from_static(allow));
        }
        response.extensions_mut().insert(ErrorCode(self.code()));
        response
    }
}

/// API応答（成功・失敗）の共通ヘッダー。
pub fn insert_api_headers(headers: &mut HeaderMap) {
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
}

/// JSON本文の応答。
pub fn json_response(status: StatusCode, body: Vec<u8>) -> Response {
    let mut response = (status, body).into_response();
    insert_api_headers(response.headers_mut());
    response
}

/// 本文を値から作るJSON応答。
pub fn json_ok<T: Serialize>(value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => ApiError::Internal.into_response(),
    }
}
