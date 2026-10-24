//! `GET /api/health`・`GET /api/status`（behavior.md 10章）。
//!
//! 上流へ疎通せず、ロード済みデータの識別情報だけを返す。未readyは503 data_unavailable。

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::api::AppState;
use crate::api::error::{ApiError, json_ok};
use crate::api::route_response::MAX_SAFE_INTEGER;

#[derive(Serialize)]
struct HealthDto<'a> {
    api_version: &'static str,
    status: &'static str,
    release_id: &'a str,
    dataset_id: &'a str,
    accidents: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    grid_cells: Option<u64>,
    load_seconds: f64,
    data_years: &'a [u16],
}

pub async fn handle(State(state): State<AppState>) -> Response {
    let Some(engine) = state.engine() else {
        return ApiError::DataUnavailable.into_response();
    };
    let dataset = engine.dataset();
    let load_seconds = (dataset.load_seconds * 1000.0).round() / 1000.0;
    let counts_ok = dataset.accidents <= MAX_SAFE_INTEGER
        && dataset.grid_cells.is_none_or(|n| n <= MAX_SAFE_INTEGER);
    if !counts_ok || !load_seconds.is_finite() || load_seconds < 0.0 {
        tracing::error!("healthのデータ識別情報が契約の範囲外です");
        return ApiError::Internal.into_response();
    }
    json_ok(&HealthDto {
        api_version: "2.0",
        status: "ok",
        release_id: &dataset.release_id,
        dataset_id: &dataset.dataset_id,
        accidents: dataset.accidents,
        grid_cells: dataset.grid_cells,
        // -0.0を0にする
        load_seconds: load_seconds + 0.0,
        data_years: &dataset.data_years,
    })
}
