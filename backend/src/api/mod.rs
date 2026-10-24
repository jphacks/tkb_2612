//! Axumハンドラ・DTO・エラー変換（担当：③）。
//!
//! JSONの検証・正規化で `contract::RouteQuery` を作り、`contract::RouteOutcome` を
//! OpenAPIのDTOへ変換する。`domain`・`data` の内部型には依存せず、`contract` と
//! `data::load_calendar` / `data::load_engine` だけを使う。
//!
//! [`AppState`] は起動処理から計算エンジン・祝日判定・提供元アダプタ・時計を受け取る
//! （本番は `data::load_*` と実アダプタ、テストはスタブ）。`/api/*` を静的配信より先に
//! 解決し、未定義の `/api/*` には404のJSONを返す。

pub mod access_log;
pub mod compress;
pub mod conditions;
pub mod error;
pub mod geocode;
pub mod health;
pub mod query;
pub mod route;
pub mod route_request;
pub mod route_response;
pub mod static_files;
pub mod transfer;
pub mod weather_cache;
pub mod weather_code;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::Router;
use axum::http::Uri;
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, any, get, post};
use tokio::sync::Semaphore;

use crate::api::error::ApiError;
use crate::api::weather_cache::WeatherCache;
use crate::config::Attribution;
use crate::contract::{Calendar, RouteEngine};
use crate::providers::clock::Clock;
use crate::providers::geoapify::Geocoder;
use crate::providers::open_meteo::WeatherProvider;

/// routeの受付上限（同時に受け付けるroute要求の数）。超過は503 service_busy。
///
/// 仕様に数値が無いので暫定の定数（未決事項）。Cloud Runの同時受付4より大きくし、
/// 1インスタンス内では実質的に上限にならないようにしている。
pub const ROUTE_ADMISSION_LIMIT: usize = 8;
/// 集計（`evaluate`）を同時に動かす数。仕様に数値が無いので暫定の定数（未決事項）。
/// Cloud Runの1 vCPUに合わせて1にしている。
pub const CPU_CONCURRENCY: usize = 1;

/// API層の設定（architecture.md 6章の一部と暫定定数）。
#[derive(Debug, Clone)]
pub struct ApiSettings {
    /// route/geocodeの受付から応答JSON準備（routeは圧縮）までの期限。
    pub request_timeout: Duration,
    /// routeの送信開始からの転送期限。
    pub route_transfer_timeout: Duration,
    /// conditionsの要求全体の期限。
    pub conditions_timeout: Duration,
    /// 予報を取得する日数。これより先はout_of_range。
    pub weather_forecast_days: u8,
    pub geocoding_attribution: Vec<Attribution>,
    /// 静的配信のroot（`frontend/dist`）。未生成でもAPIは動く。
    pub frontend_dist_dir: PathBuf,
    pub route_admission_limit: usize,
    pub cpu_concurrency: usize,
}

impl ApiSettings {
    /// 期限・日数を仕様の既定値にした設定。
    pub fn with_defaults(
        geocoding_attribution: Vec<Attribution>,
        frontend_dist_dir: PathBuf,
    ) -> Self {
        Self {
            request_timeout: Duration::from_millis(35_000),
            route_transfer_timeout: Duration::from_millis(120_000),
            conditions_timeout: Duration::from_millis(8_000),
            weather_forecast_days: 7,
            geocoding_attribution,
            frontend_dist_dir,
            route_admission_limit: ROUTE_ADMISSION_LIMIT,
            cpu_concurrency: CPU_CONCURRENCY,
        }
    }
}

/// ハンドラが使う外部の部品。
pub struct Services {
    pub calendar: Arc<dyn Calendar>,
    pub router: Arc<dyn crate::contract::Router>,
    pub geocoder: Arc<dyn Geocoder>,
    pub weather: Arc<dyn WeatherProvider>,
    pub clock: Arc<dyn Clock>,
    /// 気象キャッシュのTTL。
    pub weather_cache_ttl: Duration,
}

/// ハンドラの共有状態。複製は同じ状態を共有する。
#[derive(Clone)]
pub struct AppState {
    inner: Arc<StateInner>,
}

struct StateInner {
    settings: ApiSettings,
    calendar: Arc<dyn Calendar>,
    engine: OnceLock<Arc<dyn RouteEngine>>,
    router: Arc<dyn crate::contract::Router>,
    geocoder: Arc<dyn Geocoder>,
    weather: WeatherCache,
    clock: Arc<dyn Clock>,
    route_admission: Arc<Semaphore>,
    cpu: Arc<Semaphore>,
}

impl AppState {
    /// エンジン未設定（未ready）の状態を作る。
    pub fn new(settings: ApiSettings, services: Services) -> Self {
        let weather = WeatherCache::new(
            services.weather,
            services.clock.clone(),
            services.weather_cache_ttl,
        );
        Self {
            inner: Arc::new(StateInner {
                route_admission: Arc::new(Semaphore::new(settings.route_admission_limit)),
                cpu: Arc::new(Semaphore::new(settings.cpu_concurrency.max(1))),
                settings,
                calendar: services.calendar,
                engine: OnceLock::new(),
                router: services.router,
                geocoder: services.geocoder,
                weather,
                clock: services.clock,
            }),
        }
    }

    /// エンジンを設定してready（route/health）にする。2回目以降は無視して `false`。
    pub fn set_engine(&self, engine: Arc<dyn RouteEngine>) -> bool {
        self.inner.engine.set(engine).is_ok()
    }

    pub fn engine(&self) -> Option<&Arc<dyn RouteEngine>> {
        self.inner.engine.get()
    }

    pub fn is_ready(&self) -> bool {
        self.engine().is_some()
    }

    pub fn settings(&self) -> &ApiSettings {
        &self.inner.settings
    }

    pub fn calendar(&self) -> &Arc<dyn Calendar> {
        &self.inner.calendar
    }

    pub fn router(&self) -> &Arc<dyn crate::contract::Router> {
        &self.inner.router
    }

    pub fn geocoder(&self) -> &Arc<dyn Geocoder> {
        &self.inner.geocoder
    }

    pub fn weather(&self) -> &WeatherCache {
        &self.inner.weather
    }

    pub fn clock(&self) -> &Arc<dyn Clock> {
        &self.inner.clock
    }

    pub(crate) fn route_admission(&self) -> &Arc<Semaphore> {
        &self.inner.route_admission
    }

    pub(crate) fn cpu(&self) -> &Arc<Semaphore> {
        &self.inner.cpu
    }
}

/// 定義済みGET APIのメソッド規則。axumの `get` はHEADも受けるので、HEADを明示的に405にする。
fn api_get(handler: MethodRouter<AppState>) -> MethodRouter<AppState> {
    handler.head(get_not_allowed).fallback(get_not_allowed)
}

async fn get_not_allowed() -> Response {
    ApiError::MethodNotAllowed("GET").into_response()
}

async fn post_not_allowed() -> Response {
    ApiError::MethodNotAllowed("POST").into_response()
}

async fn api_not_found() -> Response {
    ApiError::NotFound.into_response()
}

/// どのルートにも当たらないパス。`/api` 配下はJSON、それ以外はtext/plain。
async fn fallback(uri: Uri) -> Response {
    let path = uri.path();
    if path == "/api" || path.starts_with("/api/") {
        ApiError::NotFound.into_response()
    } else {
        static_files::not_found()
    }
}

/// アプリ全体のルーター。
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/route", post(route::handle).fallback(post_not_allowed))
        .route("/api/geocode", api_get(get(geocode::handle)))
        .route("/api/conditions", api_get(get(conditions::handle)))
        .route("/api/health", api_get(get(health::handle)))
        .route("/api/status", api_get(get(health::handle)))
        .route("/api", any(api_not_found))
        .route("/api/", any(api_not_found))
        .route("/api/{*rest}", any(api_not_found))
        .route("/", static_files::method_router(static_files::index))
        .route(
            "/assets/{asset}",
            static_files::method_router(static_files::asset),
        )
        .route(
            "/manifest.webmanifest",
            static_files::method_router(static_files::manifest),
        )
        .route(
            "/sw.js",
            static_files::method_router(static_files::service_worker),
        )
        .fallback(fallback)
        .layer(axum::middleware::from_fn(access_log::access_log))
        .with_state(state)
}
