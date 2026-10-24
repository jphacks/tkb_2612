//! 結合テストの共通部品：スタブで組んだテスト用アプリ、固定時計、偽の提供元、
//! ループバックの偽上流サーバ、応答JSONの比較規則。
//!
//! テストごとに使う部品が違うので、使わないものの警告を抑える。
#![allow(dead_code)]

mod accidents;
pub mod source;

#[allow(unused_imports)]
pub use accidents::*;

use std::collections::VecDeque;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use accident_route_backend::api::{ApiSettings, AppState, Services, build_router};
use accident_route_backend::config::{Attribution, Config};
use accident_route_backend::contract::{
    BoxFuture, Calendar, CancelToken, CandidateSet, DatasetInfo, DayClassification, DayTypeReason,
    EngineError, RouteEngine, RouteOutcome, RouteQuery, Router, RoutingRequest,
};
use accident_route_backend::providers::clock::Clock;
use accident_route_backend::providers::geoapify::{GeocodeError, Geocoder, Place};
use accident_route_backend::providers::open_meteo::{
    RoundedPoint, WeatherError, WeatherProvider, WeatherReport,
};
use accident_route_backend::testing::fixtures::{expected_route_outcome, fixture_dataset_info};
use accident_route_backend::testing::{FixedCalendar, ScriptedRouter, StubEngine, StubEvaluation};
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use bytes::Bytes;
use chrono::{DateTime, Utc};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

// ---- 時計 ---------------------------------------------------------------------

/// C2とテストで使う固定の現在時刻（2026-10-24T09:00:00+09:00）。
pub const FIXED_NOW: &str = "2026-10-24T00:00:00Z";

pub fn fixed_now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(FIXED_NOW)
        .unwrap()
        .with_timezone(&Utc)
}

/// 手で進める時計。
#[derive(Debug)]
pub struct ManualClock(Mutex<DateTime<Utc>>);

impl ManualClock {
    pub fn new(now: DateTime<Utc>) -> Arc<Self> {
        Arc::new(Self(Mutex::new(now)))
    }

    pub fn set(&self, now: DateTime<Utc>) {
        *self.0.lock().unwrap() = now;
    }

    pub fn advance(&self, by: Duration) {
        let mut now = self.0.lock().unwrap();
        *now += chrono::Duration::from_std(by).unwrap();
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

/// tokioの時計（`tokio::time::pause` で止まる）に従う時計。
#[derive(Debug)]
pub struct TokioClock {
    base: DateTime<Utc>,
    start: tokio::time::Instant,
}

impl TokioClock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            base: fixed_now(),
            start: tokio::time::Instant::now(),
        })
    }
}

impl Clock for TokioClock {
    fn now(&self) -> DateTime<Utc> {
        self.base + chrono::Duration::from_std(self.start.elapsed()).unwrap()
    }
}

// ---- 偽の提供元（ハンドラ試験用） ---------------------------------------------

/// 用意した応答を順に返す住所検索。用意が無いのに呼ばれたらpanicする。
#[derive(Default)]
pub struct FakeGeocoder {
    responses: Mutex<VecDeque<Result<Vec<Place>, GeocodeError>>>,
    calls: Mutex<Vec<String>>,
    /// trueなら期限か取消しまで応答しない。
    stall: bool,
}

impl FakeGeocoder {
    pub fn new(responses: impl IntoIterator<Item = Result<Vec<Place>, GeocodeError>>) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().collect()),
            ..Self::default()
        })
    }

    pub fn stalling() -> Arc<Self> {
        Arc::new(Self {
            stall: true,
            ..Self::default()
        })
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Geocoder for FakeGeocoder {
    fn search<'a>(
        &'a self,
        q: &'a str,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<Vec<Place>, GeocodeError>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(q.to_owned());
            if self.stall {
                while cancel.check().is_ok() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                return Err(GeocodeError::Interrupted(cancel.check().unwrap_err()));
            }
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("FakeGeocoderに応答が用意されていない")
        })
    }
}

/// 用意した応答を順に返す気象提供元。用意が無いのに呼ばれたらpanicする。
#[derive(Default)]
pub struct FakeWeather {
    responses: Mutex<VecDeque<Result<WeatherReport, WeatherError>>>,
    calls: Mutex<Vec<RoundedPoint>>,
    stall: bool,
}

impl FakeWeather {
    pub fn new(
        responses: impl IntoIterator<Item = Result<WeatherReport, WeatherError>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            responses: Mutex::new(responses.into_iter().collect()),
            ..Self::default()
        })
    }

    pub fn stalling() -> Arc<Self> {
        Arc::new(Self {
            stall: true,
            ..Self::default()
        })
    }

    pub fn calls(&self) -> Vec<RoundedPoint> {
        self.calls.lock().unwrap().clone()
    }
}

impl WeatherProvider for FakeWeather {
    fn fetch<'a>(
        &'a self,
        point: RoundedPoint,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<WeatherReport, WeatherError>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(point);
            if self.stall {
                while cancel.check().is_ok() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                return Err(WeatherError::Interrupted(cancel.check().unwrap_err()));
            }
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("FakeWeatherに応答が用意されていない")
        })
    }
}

/// どの段階で失敗するか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailAt {
    Collect,
    Evaluate,
}

/// 指定の `EngineError` を返すエンジン。
pub struct ErrorEngine {
    dataset: DatasetInfo,
    error: EngineError,
    at: FailAt,
}

impl ErrorEngine {
    pub fn new(error: EngineError, at: FailAt) -> Arc<Self> {
        Arc::new(Self {
            dataset: fixture_dataset_info(),
            error,
            at,
        })
    }
}

impl RouteEngine for ErrorEngine {
    fn dataset(&self) -> &DatasetInfo {
        &self.dataset
    }

    fn collect_candidates<'a>(
        &'a self,
        _router: &'a dyn Router,
        _query: &'a RouteQuery,
        _cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<CandidateSet, EngineError>> {
        Box::pin(async move {
            match self.at {
                FailAt::Collect => Err(self.error.clone()),
                FailAt::Evaluate => Ok(CandidateSet {
                    routes: Vec::new(),
                    warnings: Vec::new(),
                }),
            }
        })
    }

    fn evaluate(
        &self,
        _query: &RouteQuery,
        _candidates: CandidateSet,
        _cancel: &CancelToken,
    ) -> Result<RouteOutcome, EngineError> {
        Err(self.error.clone())
    }
}

/// 受け取ったクエリを記録して、内側のエンジンへ委ねるエンジン。
pub struct RecordingEngine {
    inner: Arc<dyn RouteEngine>,
    queries: Mutex<Vec<RouteQuery>>,
}

impl RecordingEngine {
    pub fn new(inner: Arc<dyn RouteEngine>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            queries: Mutex::new(Vec::new()),
        })
    }

    /// collect・evaluateの順に受け取ったクエリ。
    pub fn queries(&self) -> Vec<RouteQuery> {
        self.queries.lock().unwrap().clone()
    }
}

impl RouteEngine for RecordingEngine {
    fn dataset(&self) -> &DatasetInfo {
        self.inner.dataset()
    }

    fn collect_candidates<'a>(
        &'a self,
        router: &'a dyn Router,
        query: &'a RouteQuery,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<CandidateSet, EngineError>> {
        self.queries.lock().unwrap().push(query.clone());
        self.inner.collect_candidates(router, query, cancel)
    }

    fn evaluate(
        &self,
        query: &RouteQuery,
        candidates: CandidateSet,
        cancel: &CancelToken,
    ) -> Result<RouteOutcome, EngineError> {
        self.queries.lock().unwrap().push(query.clone());
        self.inner.evaluate(query, candidates, cancel)
    }
}

/// 集計でpanicするエンジン。
pub struct PanicEngine(DatasetInfo);

impl PanicEngine {
    pub fn new() -> Arc<Self> {
        Arc::new(Self(fixture_dataset_info()))
    }
}

impl RouteEngine for PanicEngine {
    fn dataset(&self) -> &DatasetInfo {
        &self.0
    }

    fn collect_candidates<'a>(
        &'a self,
        _router: &'a dyn Router,
        _query: &'a RouteQuery,
        _cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<CandidateSet, EngineError>> {
        Box::pin(async move {
            Ok(CandidateSet {
                routes: Vec::new(),
                warnings: Vec::new(),
            })
        })
    }

    fn evaluate(
        &self,
        _query: &RouteQuery,
        _candidates: CandidateSet,
        _cancel: &CancelToken,
    ) -> Result<RouteOutcome, EngineError> {
        panic!("テスト用のpanic");
    }
}

/// 呼ばれたら失敗するルーター（ScriptedRouterに応答を用意しない）。
pub fn unused_router() -> Arc<ScriptedRouter> {
    Arc::new(ScriptedRouter::new([]))
}

pub fn stub_engine(name: &str) -> Arc<StubEngine> {
    Arc::new(StubEngine::new(
        fixture_dataset_info(),
        StubEvaluation::Fixed(Box::new(expected_route_outcome(name))),
    ))
}

pub fn weekday() -> DayClassification {
    DayClassification {
        reason: DayTypeReason::Weekday,
        holiday_table_covered: true,
    }
}

pub fn default_attribution() -> Vec<Attribution> {
    vec![
        Attribution {
            text: "Powered by Geoapify".to_owned(),
            url: "https://www.geoapify.com/".to_owned(),
        },
        Attribution {
            text: "© OpenStreetMap contributors".to_owned(),
            url: "https://www.openstreetmap.org/copyright".to_owned(),
        },
    ]
}

// ---- テスト用アプリ ----------------------------------------------------------

/// スタブで組むテスト用アプリ。既定はready（`singleRoute` を返すStubEngine）、
/// 外部呼出しは用意が無ければpanicする。
pub struct TestAppBuilder {
    pub settings: ApiSettings,
    pub engine: Option<Arc<dyn RouteEngine>>,
    pub calendar: DayClassification,
    /// 指定したら `calendar` の代わりに使う（呼出しの記録用）。
    pub custom_calendar: Option<Arc<dyn Calendar>>,
    pub router: Arc<dyn Router>,
    pub geocoder: Arc<dyn Geocoder>,
    pub weather: Arc<dyn WeatherProvider>,
    pub clock: Arc<dyn Clock>,
    pub weather_cache_ttl: Duration,
}

impl Default for TestAppBuilder {
    fn default() -> Self {
        Self {
            settings: ApiSettings::with_defaults(
                default_attribution(),
                PathBuf::from("/nonexistent-frontend-dist"),
            ),
            engine: Some(stub_engine("singleRoute")),
            calendar: weekday(),
            custom_calendar: None,
            router: unused_router(),
            geocoder: FakeGeocoder::new([]),
            weather: FakeWeather::new([]),
            clock: ManualClock::new(fixed_now()),
            weather_cache_ttl: Duration::from_secs(600),
        }
    }
}

impl TestAppBuilder {
    pub fn build(self) -> TestApp {
        let state = AppState::new(
            self.settings,
            Services {
                calendar: self
                    .custom_calendar
                    .unwrap_or_else(|| Arc::new(FixedCalendar(self.calendar))),
                router: self.router,
                geocoder: self.geocoder,
                weather: self.weather,
                clock: self.clock,
                weather_cache_ttl: self.weather_cache_ttl,
            },
        );
        if let Some(engine) = self.engine {
            state.set_engine(engine);
        }
        let router = build_router(state.clone());
        TestApp { state, router }
    }
}

pub struct TestApp {
    pub state: AppState,
    pub router: axum::Router,
}

/// 応答の全体（本文は受信済み）。
#[derive(Debug)]
pub struct TestResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl TestResponse {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "JSONでない応答: {e}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(|v| v.to_str().unwrap())
    }

    /// エラー応答のcode。
    pub fn error_code(&self) -> String {
        self.json()["error"]["code"].as_str().unwrap().to_owned()
    }
}

impl TestApp {
    pub async fn send(&self, request: Request<Body>) -> TestResponse {
        let response = self.router.clone().oneshot(request).await.unwrap();
        let (parts, body) = response.into_parts();
        let body = body.collect().await.unwrap().to_bytes();
        TestResponse {
            status: parts.status,
            headers: parts.headers,
            body,
        }
    }

    pub async fn get(&self, target: &str) -> TestResponse {
        self.send(request(Method::GET, target, Body::empty())).await
    }

    pub async fn post_json(&self, target: &str, body: impl Into<Body>) -> TestResponse {
        self.send(
            Request::builder()
                .method(Method::POST)
                .uri(target)
                .header("content-type", "application/json")
                .body(body.into())
                .unwrap(),
        )
        .await
    }
}

pub fn request(method: Method, target: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(target)
        .body(body)
        .unwrap()
}

/// fixtureの例の正規リクエスト本文。
pub fn canonical_request(name: &str) -> Value {
    let doc: Value = serde_json::from_str(
        &std::fs::read_to_string(
            accident_route_backend::testing::fixtures::docs_fixture_path(
                "route-example-inputs.json",
            ),
        )
        .unwrap(),
    )
    .unwrap();
    doc["examples"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == name)
        .unwrap_or_else(|| panic!("例 {name} がありません"))["request"]
        .clone()
}

/// 有効なroute要求（東京駅付近の約1km）。
pub fn valid_route_body() -> Value {
    serde_json::json!({
        "origin": {"lat": 35.681, "lon": 139.767},
        "dest": {"lat": 35.69, "lon": 139.77},
    })
}

// ---- 応答JSONの比較規則（C1・DTO試験で共通） ---------------------------------

/// `aggregation_ms` が存在し有限の非負数であることを検査してから取り除く
/// （共通前提4章 C3(2)。値の比較からは除く）。
pub fn check_and_strip_aggregation_ms(value: &mut Value) {
    let object = value.as_object_mut().expect("route応答はobject");
    let aggregation = object
        .remove("aggregation_ms")
        .expect("aggregation_msがない");
    let number = aggregation
        .as_f64()
        .unwrap_or_else(|| panic!("aggregation_msが数値でない: {aggregation}"));
    assert!(
        number.is_finite() && number >= 0.0,
        "aggregation_msが有限の非負数でない: {number}"
    );
}

/// JSONを比較する。数値は数値として比べ（`0` と `0.0` を同一視）、それ以外は完全一致。
/// 最初の差のパスを返す。
pub fn json_diff(actual: &Value, expected: &Value) -> Option<String> {
    fn walk(path: &str, a: &Value, e: &Value) -> Option<String> {
        match (a, e) {
            (Value::Number(x), Value::Number(y)) => {
                let same = match (x.as_i64(), y.as_i64()) {
                    (Some(i), Some(j)) => i == j,
                    _ => x.as_f64() == y.as_f64(),
                };
                (!same).then(|| format!("{path}: {x} != {y}"))
            }
            (Value::Array(xs), Value::Array(ys)) => {
                if xs.len() != ys.len() {
                    return Some(format!("{path}: 長さ {} != {}", xs.len(), ys.len()));
                }
                xs.iter()
                    .zip(ys)
                    .enumerate()
                    .find_map(|(i, (x, y))| walk(&format!("{path}[{i}]"), x, y))
            }
            (Value::Object(xs), Value::Object(ys)) => {
                let mut keys_a: Vec<_> = xs.keys().collect();
                let mut keys_e: Vec<_> = ys.keys().collect();
                keys_a.sort();
                keys_e.sort();
                if keys_a != keys_e {
                    return Some(format!("{path}: キー {keys_a:?} != {keys_e:?}"));
                }
                xs.iter()
                    .find_map(|(k, x)| walk(&format!("{path}.{k}"), x, &ys[k]))
            }
            _ => (a != e).then(|| format!("{path}: {a} != {e}")),
        }
    }
    walk("$", actual, expected)
}

pub fn assert_json_matches(actual: &Value, expected: &Value) {
    if let Some(diff) = json_diff(actual, expected) {
        panic!("JSONが一致しない: {diff}");
    }
}

/// API応答（成功・失敗）の共通ヘッダー。
pub fn assert_api_headers(response: &TestResponse) {
    assert_eq!(response.header("content-type"), Some("application/json"));
    assert_eq!(response.header("cache-control"), Some("no-store"));
    assert_eq!(response.header("x-content-type-options"), Some("nosniff"));
}

// ---- ループバックの偽上流サーバ ------------------------------------------------

/// 偽上流が受けた要求。
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: HeaderMap,
}

/// 偽上流の応答。
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    /// 応答の先頭を返すまでの遅延。
    pub delay: Duration,
    /// 本文を分割して送る間隔（`None` なら一度に送る）。
    pub chunk_interval: Option<Duration>,
    /// Content-Lengthを付けない（チャンク転送）。
    pub chunked: bool,
}

impl Reply {
    pub fn json(body: &Value) -> Self {
        Self::bytes(200, serde_json::to_vec(body).unwrap())
    }

    pub fn bytes(status: u16, body: Vec<u8>) -> Self {
        Self {
            status,
            body,
            delay: Duration::ZERO,
            chunk_interval: None,
            chunked: false,
        }
    }

    pub fn delayed(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub fn trickle(mut self, interval: Duration) -> Self {
        self.chunk_interval = Some(interval);
        self.chunked = true;
        self
    }

    pub fn chunked(mut self) -> Self {
        self.chunked = true;
        self
    }
}

type Handler = dyn Fn(&Recorded) -> Reply + Send + Sync;

/// ループバックの空きポートで動く偽上流。
pub struct FakeUpstream {
    pub addr: SocketAddr,
    requests: Arc<Mutex<Vec<Recorded>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeUpstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeUpstream {
    pub async fn start(handler: impl Fn(&Recorded) -> Reply + Send + Sync + 'static) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let recorded = requests.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let handler = handler.clone();
                let recorded = recorded.clone();
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(
                        move |request: hyper::Request<hyper::body::Incoming>| {
                            let handler = handler.clone();
                            let recorded = recorded.clone();
                            async move {
                                let record = Recorded {
                                    method: request.method().to_string(),
                                    path: request.uri().path().to_owned(),
                                    query: request.uri().query().map(str::to_owned),
                                    headers: request.headers().clone(),
                                };
                                recorded.lock().unwrap().push(record.clone());
                                let reply = handler(&record);
                                tokio::time::sleep(reply.delay).await;
                                Ok::<_, Infallible>(build_reply(reply))
                            }
                        },
                    );
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Self {
            addr,
            requests,
            task,
        }
    }

    /// 固定の応答を返す偽上流。
    pub async fn fixed(reply: Reply) -> Self {
        Self::start(move |_| reply.clone()).await
    }

    pub fn base_url(&self) -> reqwest::Url {
        reqwest::Url::parse(&format!("http://{}/", self.addr)).unwrap()
    }

    pub fn url(&self, path: &str) -> reqwest::Url {
        reqwest::Url::parse(&format!("http://{}{path}", self.addr)).unwrap()
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

fn build_reply(reply: Reply) -> hyper::Response<axum::body::Body> {
    let status = StatusCode::from_u16(reply.status).unwrap();
    let body = if let Some(interval) = reply.chunk_interval {
        let chunks: Vec<Bytes> = reply.body.chunks(16).map(Bytes::copy_from_slice).collect();
        let stream = futures_util::stream::unfold(chunks.into_iter(), move |mut rest| async move {
            let chunk = rest.next()?;
            tokio::time::sleep(interval).await;
            Some((Ok::<_, Infallible>(chunk), rest))
        });
        Body::from_stream(stream)
    } else if reply.chunked {
        let chunk = Bytes::from(reply.body);
        Body::from_stream(futures_util::stream::iter([Ok::<_, Infallible>(chunk)]))
    } else {
        Body::from(reply.body)
    };
    let mut response = hyper::Response::new(body);
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    response
}

/// 分の差を `Duration` で（テストの読みやすさのため）。
pub fn minutes(n: u64) -> Duration {
    Duration::from_secs(n * 60)
}

/// 状態からのrouteリクエストの結果を待たずに使う、ループバックの実サーバ。
pub struct LiveServer {
    pub addr: SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    pub task: tokio::task::JoinHandle<()>,
}

impl LiveServer {
    pub async fn start(app: axum::Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            accident_route_backend::startup::serve(listener, app, async move {
                let _ = stopped.await;
            })
            .await;
        });
        Self {
            addr,
            stop: Some(stop),
            task,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = (&mut self.task).await;
    }
}

/// 日本矩形内の住所候補。
pub fn place(lat: f64, lon: f64, label: &str) -> Place {
    Place {
        position: accident_route_backend::contract::LatLon { lat, lon },
        label: label.to_owned(),
    }
}

/// ルーターの呼出しを記録したまま使う（呼出し要求の確認用）。
pub fn routing_calls(router: &ScriptedRouter) -> Vec<RoutingRequest> {
    router.calls()
}

/// 起動処理の試験用の設定。上流はすべて到達しない宛先（実通信しない）、ゲートはlocal。
pub fn test_config(data_dir: &std::path::Path, key_file: &std::path::Path) -> Config {
    Config::from_lookup(|name| {
        Some(match name {
            "ACCIDENT_DATA_DIR" => data_dir.to_str().unwrap().to_owned(),
            "BACKEND_BIND" => "127.0.0.1:0".to_owned(),
            "OSRM_WALK_BASE_URL" | "OSRM_BIKE_BASE_URL" | "OSRM_CAR_BASE_URL" => {
                "http://127.0.0.1:9/".to_owned()
            }
            "GEOAPIFY_BASE_URL" | "OPEN_METEO_BASE_URL" => "http://127.0.0.1:9/".to_owned(),
            "GEOAPIFY_API_KEY_FILE" => key_file.to_str().unwrap().to_owned(),
            "EXTERNAL_USER_AGENT" => "accident-route-test/0.1 (test@example.invalid)".to_owned(),
            "ROUTING_ATTRIBUTION" => "© OpenStreetMap contributors".to_owned(),
            "GEOCODING_ATTRIBUTION" => {
                r#"[{"text":"Powered by Geoapify","url":"https://www.geoapify.com/"}]"#.to_owned()
            }
            "UPSTREAM_GATE" => "local".to_owned(),
            _ => return None,
        })
    })
    .unwrap()
}
