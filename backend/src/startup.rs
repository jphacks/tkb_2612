//! 起動・待受け・停止（architecture.md 4〜7章）。
//!
//! 1. `ACCIDENT_DATA_DIR` を1回だけ正規化し、同じreleaseディレクトリを両方の読込に渡す。
//! 2. 祝日表を読む（失敗なら起動を止める）。待受け前に読むので、conditionsは
//!    事故データの準備を待たずに応答できる。
//! 3. 提供元アダプタと上流ゲートを作り、待受けを始める。
//! 4. 事故データを裏で全件ロードし、成功したらready（route/health）にする。
//!    失敗したらプロセスを止める（非0終了）。
//!
//! 待受けはhyperの接続ごとに [`ConnectionKill`] を要求へ付け、routeの転送期限で接続を
//! 切れるようにする。SIGTERM（Cloud Run）とCtrl-Cで新しい接続の受付をやめ、
//! 処理中の接続を最大 [`SHUTDOWN_GRACE`] 待つ。

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::TcpListener;
use tokio::sync::{Notify, oneshot};
use tower_service::Service;

use crate::api::transfer::ConnectionKill;
use crate::api::{ApiSettings, AppState, Services, build_router};
use crate::config::{Config, GateBackend};
use crate::contract::{Calendar, RouteEngine};
use crate::data::LoadError;
use crate::providers::clock::{Clock, SystemClock};
use crate::providers::gate::{Gate, GatePolicy, GateStore, LocalGateStore, Quota, QuotaPeriod};
use crate::providers::gate_firestore::{FirestoreGateStore, metadata_project_id};
use crate::providers::geoapify::GeoapifyGeocoder;
use crate::providers::http::UpstreamClient;
use crate::providers::open_meteo::OpenMeteoProvider;
use crate::providers::osrm::{OsrmEndpoints, OsrmRouter};

/// 停止時に処理中の接続を待つ上限。Cloud RunはSIGTERMから10秒で強制終了する。
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);
/// リースの有効期間に足す余裕（1通信の期限＋この値）。
pub const LEASE_MARGIN: Duration = Duration::from_secs(2);
/// 接続の受付が一時的に失敗したとき（ファイル記述子の枯渇等）に、次の受付まで待つ時間（暫定。未決）。
pub const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(10);
/// 提供元ごとの同時通信数（architecture.md 6章「同時通信は1」、backend-spec.md 6章）。
pub const UPSTREAM_MAX_CONCURRENT: u32 = 1;
/// 気象提供元の応答の上限（behavior.md 9章）。
pub const WEATHER_MAX_BYTES: usize = 1_000_000;
/// Open-Meteo無料APIの分/時/日/月の上限。公式の値（2026-10-24参照）で、
/// 配備前に利用規約・プランで再確認する（**要確認**）。
pub const OPEN_METEO_QUOTAS: [Quota; 4] = [
    Quota {
        period: QuotaPeriod::Minute,
        limit: 600,
    },
    Quota {
        period: QuotaPeriod::Hour,
        limit: 5_000,
    },
    Quota {
        period: QuotaPeriod::Day,
        limit: 10_000,
    },
    Quota {
        period: QuotaPeriod::Month,
        limit: 300_000,
    },
];

/// 上流ゲートの共有状態のキー（提供元ごと）。
pub const OSRM_GATE: &str = "osrm";
pub const GEOAPIFY_GATE: &str = "geoapify";
pub const OPEN_METEO_GATE: &str = "open-meteo";

type CalendarLoader =
    Box<dyn FnOnce(&Path) -> Result<Arc<dyn Calendar>, LoadError> + Send + 'static>;
type EngineLoader =
    Box<dyn FnOnce(&Path) -> Result<Arc<dyn RouteEngine>, LoadError> + Send + 'static>;

/// releaseの読込関数。本番は `data::load_calendar` / `data::load_engine`、テストはスタブ。
pub struct Loaders {
    pub calendar: CalendarLoader,
    pub engine: EngineLoader,
}

impl Loaders {
    pub fn production() -> Self {
        Self {
            calendar: Box::new(crate::data::load_calendar),
            engine: Box::new(crate::data::load_engine),
        }
    }
}

/// 起動の失敗。メッセージに秘密・URLを含めない。
#[derive(Debug)]
pub enum StartupError {
    /// releaseの解決・祝日表・事故データの読込の失敗。
    Data(LoadError),
    /// 上流HTTPクライアントを作れなかった。
    Client,
    /// Firestoreの上流ゲートのプロジェクトを特定できなかった。
    GateProject,
    Bind(std::io::Error),
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartupError::Data(e) => write!(f, "{e}"),
            StartupError::Client => f.write_str("上流HTTPクライアントを作れません"),
            StartupError::GateProject => {
                f.write_str("上流ゲートのFirestoreプロジェクトをメタデータサーバから取得できません")
            }
            StartupError::Bind(e) => write!(f, "待受けを開始できません: {e}"),
        }
    }
}

impl std::error::Error for StartupError {}

/// 待受け開始前まで準備したサーバ。
pub struct Server {
    listener: TcpListener,
    state: AppState,
    release_dir: PathBuf,
    engine_loader: EngineLoader,
}

/// releaseを解決して祝日表を読み、提供元を組み立てて待受けを始める。
pub async fn prepare(config: &Config, loaders: Loaders) -> Result<Server, StartupError> {
    let data_dir = config.accident_data_dir.clone();
    let calendar_loader = loaders.calendar;
    let (release_dir, calendar) = tokio::task::spawn_blocking(move || {
        let release_dir = std::fs::canonicalize(&data_dir).map_err(LoadError::Io)?;
        let calendar = calendar_loader(&release_dir)?;
        Ok::<_, LoadError>((release_dir, calendar))
    })
    .await
    .map_err(|_| StartupError::Data(LoadError::Invalid("祝日表の読込が異常終了しました".into())))?
    .map_err(StartupError::Data)?;

    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let store = gate_store(config).await?;
    let services = services(config, calendar, store, clock)?;
    let settings = ApiSettings {
        request_timeout: config.request_timeout,
        route_transfer_timeout: config.route_transfer_timeout,
        conditions_timeout: config.conditions_timeout,
        weather_forecast_days: config.weather_forecast_days,
        ..ApiSettings::with_defaults(
            config.geocoding_attribution.clone(),
            config.frontend_dist_dir.clone(),
        )
    };
    let state = AppState::new(settings, services);
    let listener = TcpListener::bind(config.bind)
        .await
        .map_err(StartupError::Bind)?;
    Ok(Server {
        listener,
        state,
        release_dir,
        engine_loader: loaders.engine,
    })
}

async fn gate_store(config: &Config) -> Result<Arc<dyn GateStore>, StartupError> {
    match config.upstream_gate {
        GateBackend::Local => {
            tracing::warn!("上流ゲートはプロセス内だけで共有します（ローカル開発専用）");
            Ok(Arc::new(LocalGateStore::new()))
        }
        GateBackend::Firestore => {
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()
                .map_err(|_| StartupError::Client)?;
            let project = metadata_project_id(&client)
                .await
                .map_err(|_| StartupError::GateProject)?;
            Ok(Arc::new(FirestoreGateStore::production(client, &project)))
        }
    }
}

fn services(
    config: &Config,
    calendar: Arc<dyn Calendar>,
    store: Arc<dyn GateStore>,
    clock: Arc<dyn Clock>,
) -> Result<Services, StartupError> {
    let client =
        UpstreamClient::new(&config.external_user_agent).map_err(|_| StartupError::Client)?;
    let gate = |key: &str, min_interval: Duration, timeout: Duration, quotas: Vec<Quota>| {
        Gate::new(
            key,
            GatePolicy {
                min_interval,
                max_concurrent: UPSTREAM_MAX_CONCURRENT,
                queue_capacity: config.upstream_queue_capacity,
                lease_ttl: timeout + LEASE_MARGIN,
                quotas,
            },
            store.clone(),
            clock.clone(),
        )
    };
    let router = OsrmRouter::new(
        OsrmEndpoints {
            walk: config.osrm_walk_base_url.clone(),
            bike: config.osrm_bike_base_url.clone(),
            car: config.osrm_car_base_url.clone(),
        },
        client.clone(),
        gate(
            OSRM_GATE,
            config.routing_min_interval,
            config.upstream_timeout,
            Vec::new(),
        ),
        config.upstream_timeout,
        config.upstream_max_bytes,
    );
    let geocoder = GeoapifyGeocoder::new(
        config.geoapify_base_url.clone(),
        config.geoapify_api_key.expose().to_owned(),
        client.clone(),
        gate(
            GEOAPIFY_GATE,
            config.geocoding_min_interval,
            config.upstream_timeout,
            vec![Quota {
                period: QuotaPeriod::Day,
                limit: config.geocoding_daily_limit,
            }],
        ),
        config.upstream_timeout,
        config.upstream_max_bytes,
    );
    let weather = OpenMeteoProvider::new(
        config.open_meteo_base_url.clone(),
        config
            .open_meteo_api_key
            .as_ref()
            .map(|k| k.expose().to_owned()),
        config.weather_forecast_days,
        client,
        gate(
            OPEN_METEO_GATE,
            config.weather_min_interval,
            config.weather_timeout,
            OPEN_METEO_QUOTAS.to_vec(),
        ),
        config.weather_timeout,
        WEATHER_MAX_BYTES,
    );
    Ok(Services {
        calendar,
        router: Arc::new(router),
        geocoder: Arc::new(geocoder),
        weather: Arc::new(weather),
        clock,
        weather_cache_ttl: config.weather_cache_ttl,
    })
}

impl Server {
    pub fn local_addr(&self) -> SocketAddr {
        self.listener
            .local_addr()
            .expect("待受け中のソケットのアドレス")
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// 事故データを裏でロードしながら、`shutdown` か読込失敗まで待ち受ける。
    ///
    /// 読込失敗なら `Err(Data)` を返す（呼出し側は非0で終了する）。
    pub async fn run(self, shutdown: impl Future<Output = ()> + Send) -> Result<(), StartupError> {
        let Server {
            listener,
            state,
            release_dir,
            engine_loader,
        } = self;
        let (fail_tx, fail_rx) = oneshot::channel::<LoadError>();
        let ready_state = state.clone();
        tokio::spawn(async move {
            let loaded = tokio::task::spawn_blocking(move || engine_loader(&release_dir)).await;
            match loaded {
                Ok(Ok(engine)) => {
                    let dataset = engine.dataset();
                    tracing::info!(
                        release_id = %dataset.release_id,
                        dataset_id = %dataset.dataset_id,
                        accidents = dataset.accidents,
                        load_seconds = dataset.load_seconds,
                        "事故データをロードしました"
                    );
                    ready_state.set_engine(engine);
                }
                Ok(Err(error)) => {
                    let _ = fail_tx.send(error);
                }
                Err(_) => {
                    let _ = fail_tx.send(LoadError::Invalid(
                        "事故データのロードが異常終了しました".into(),
                    ));
                }
            }
        });
        let failure = Arc::new(Mutex::new(None));
        let failure_slot = failure.clone();
        let stop = async move {
            tokio::select! {
                () = shutdown => {}
                Ok(error) = fail_rx => {
                    *failure_slot.lock().unwrap() = Some(error);
                }
            }
        };
        tracing::info!(addr = %listener.local_addr().map_err(StartupError::Bind)?, "待受けを開始しました");
        serve(listener, build_router(state), stop).await;
        match failure.lock().unwrap().take() {
            Some(error) => Err(StartupError::Data(error)),
            None => Ok(()),
        }
    }
}

/// 設定から起動し、`shutdown` まで待ち受ける。
pub async fn run(
    config: &Config,
    loaders: Loaders,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<(), StartupError> {
    prepare(config, loaders).await?.run(shutdown).await
}

/// HTTP/1.1で待ち受ける。接続ごとに [`ConnectionKill`] を要求の拡張に入れ、
/// 通知されたら接続を切る。`shutdown` で受付をやめ、処理中の接続を最大
/// [`SHUTDOWN_GRACE`] 待つ。
pub async fn serve(listener: TcpListener, app: axum::Router, shutdown: impl Future<Output = ()>) {
    let graceful = hyper_util::server::graceful::GracefulShutdown::new();
    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else {
                    tokio::time::sleep(ACCEPT_RETRY_DELAY).await;
                    continue;
                };
                let kill = Arc::new(Notify::new());
                let extension = ConnectionKill(kill.clone());
                let app = app.clone();
                let service = hyper::service::service_fn(move |mut request: hyper::Request<Incoming>| {
                    request.extensions_mut().insert(extension.clone());
                    let mut app = app.clone();
                    async move {
                        let response = app.call(request.map(axum::body::Body::new)).await;
                        Ok::<_, Infallible>(response.unwrap_or_else(|never| match never {}))
                    }
                });
                let connection = hyper::server::conn::http1::Builder::new()
                    .timer(TokioTimer::new())
                    .serve_connection(TokioIo::new(stream), service);
                let connection = graceful.watch(connection);
                tokio::spawn(async move {
                    tokio::select! {
                        _ = connection => {}
                        // 接続のfutureを破棄してソケットを閉じる
                        () = kill.notified() => {}
                    }
                });
            }
            () = &mut shutdown => break,
        }
    }
    drop(listener);
    if tokio::time::timeout(SHUTDOWN_GRACE, graceful.shutdown())
        .await
        .is_err()
    {
        tracing::warn!("停止猶予内に終わらなかった接続を切断します");
    }
}

/// SIGTERMかCtrl-Cを待つ。
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("停止要求を受けました");
}
