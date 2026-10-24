//! 静的配信（behavior.md 10章、architecture.md 4章）。
//!
//! 配信rootは `FRONTEND_DIST_DIR` だけで、`/`・`/assets/{asset}`・`/manifest.webmanifest`・
//! `/sw.js` の4種類に限る。`/assets` は単一のファイル名だけを受け、要求ごとにrootと
//! ファイルを正規化して、root外（シンボリックリンクを含む）・ディレクトリ・
//! ドットで始まる名前を拒否する。ディレクトリ一覧・SPAフォールバックはしない。

use std::path::{Path, PathBuf};

use axum::extract::{Path as UrlPath, State};
use axum::handler::Handler;
use axum::http::header::{
    ALLOW, CACHE_CONTROL, CONTENT_TYPE, HeaderName, HeaderValue, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get};

use crate::api::AppState;

const NO_CACHE: &str = "no-cache";
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const SERVICE_WORKER_ALLOWED: HeaderName = HeaderName::from_static("service-worker-allowed");

/// GETだけを受ける静的ルート（HEADはGETと同じ）。ほかのメソッドは405。
pub fn method_router<H, T>(handler: H) -> MethodRouter<AppState>
where
    H: Handler<T, AppState>,
    T: 'static,
{
    get(handler).fallback(method_not_allowed)
}

async fn method_not_allowed() -> Response {
    let mut response = text(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed");
    response
        .headers_mut()
        .insert(ALLOW, HeaderValue::from_static("GET"));
    response
}

fn text(status: StatusCode, body: &'static str) -> Response {
    let mut response = (status, body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

/// 静的ファイルなし・配信対象外。
pub fn not_found() -> Response {
    text(StatusCode::NOT_FOUND, "Not Found")
}

/// Viteのアセット名（`^[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+)+$`）か。
pub fn is_asset_name(name: &str) -> bool {
    let mut parts = name.split('.');
    let valid = |p: &str| {
        !p.is_empty()
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    };
    parts.next().is_some_and(valid) && {
        let rest: Vec<&str> = parts.collect();
        !rest.is_empty() && rest.into_iter().all(valid)
    }
}

/// 正規化したdistの直下（`subdir` があればその直下）の通常ファイルを正規化して返す。
/// dist外（`subdir` 自体がdist外へのリンクの場合を含む）・ファイル以外は `None`。
async fn resolve(dist: &Path, subdir: Option<&str>, name: &str) -> Option<PathBuf> {
    let dist = tokio::fs::canonicalize(dist).await.ok()?;
    // 期待する親は、正規化したdistに名前を足したもの（`subdir` は正規化しない）
    let dir = match subdir {
        Some(subdir) => dist.join(subdir),
        None => dist,
    };
    let file = tokio::fs::canonicalize(dir.join(name)).await.ok()?;
    if file.parent() != Some(dir.as_path()) {
        return None;
    }
    tokio::fs::metadata(&file)
        .await
        .ok()
        .filter(|m| m.is_file())
        .map(|_| file)
}

async fn serve(
    dist: &Path,
    subdir: Option<&str>,
    name: &str,
    content_type: &'static str,
    cache_control: &'static str,
) -> Response {
    let Some(file) = resolve(dist, subdir, name).await else {
        return not_found();
    };
    let Ok(body) = tokio::fs::read(&file).await else {
        return not_found();
    };
    let mut response = (StatusCode::OK, body).into_response();
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static(cache_control));
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

fn dist(state: &AppState) -> &Path {
    &state.settings().frontend_dist_dir
}

pub async fn index(State(state): State<AppState>) -> Response {
    serve(
        dist(&state),
        None,
        "index.html",
        "text/html; charset=utf-8",
        NO_CACHE,
    )
    .await
}

pub async fn manifest(State(state): State<AppState>) -> Response {
    serve(
        dist(&state),
        None,
        "manifest.webmanifest",
        "application/manifest+json",
        NO_CACHE,
    )
    .await
}

pub async fn service_worker(State(state): State<AppState>) -> Response {
    let mut response = serve(dist(&state), None, "sw.js", "text/javascript", NO_CACHE).await;
    if response.status() == StatusCode::OK {
        response
            .headers_mut()
            .insert(SERVICE_WORKER_ALLOWED, HeaderValue::from_static("/"));
    }
    response
}

pub async fn asset(
    State(state): State<AppState>,
    uri: Uri,
    UrlPath(name): UrlPath<String>,
) -> Response {
    // 復号前のパスも同じ名前であること（%2E等の符号化した区切り・ドットを受けない）
    if !is_asset_name(&name) || uri.path() != format!("/assets/{name}") {
        return not_found();
    }
    serve(
        dist(&state),
        Some("assets"),
        &name,
        content_type(&name),
        IMMUTABLE,
    )
    .await
}

/// 拡張子からContent-Typeを決める。
pub fn content_type(name: &str) -> &'static str {
    let extension = name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "woff2" => "font/woff2",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "json" | "map" => "application/json",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names_follow_the_openapi_pattern() {
        for ok in ["index-abc123.js", "a.b.c", "icon-192_x.png", "x.y"] {
            assert!(is_asset_name(ok), "{ok}");
        }
        for ng in [
            "", "noext", ".env", "a..b", "a.", "../x.js", "a/b.js", "a b.js", "é.js", "a.js%00",
        ] {
            assert!(!is_asset_name(ng), "{ng}");
        }
    }
}
