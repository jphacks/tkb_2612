//! S15：起動配線（releaseの解決、祝日表→待受け→事故データの順、ready、失敗時停止、終了）。
//!
//! 読込関数はスタブを注入する。上流には通信しない。

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use accident_route_backend::contract::{Calendar, RouteEngine};
use accident_route_backend::data::LoadError;
use accident_route_backend::startup::{self, Loaders, StartupError};
use accident_route_backend::testing::FixedCalendar;

struct Release {
    _temp: tempfile::TempDir,
    /// 正規化前の指定（`.` を含む）。
    given: PathBuf,
    canonical: PathBuf,
    key: PathBuf,
}

fn release() -> Release {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("release");
    std::fs::create_dir(&dir).unwrap();
    let key = temp.path().join("key");
    std::fs::write(&key, "dummy-key").unwrap();
    Release {
        given: temp.path().join(".").join("release"),
        canonical: std::fs::canonicalize(&dir).unwrap(),
        key,
        _temp: temp,
    }
}

type Seen = Arc<Mutex<Vec<(&'static str, PathBuf)>>>;

fn calendar_ok(
    seen: &Seen,
) -> impl FnOnce(&Path) -> Result<Arc<dyn Calendar>, LoadError> + Send + 'static {
    let seen = seen.clone();
    move |path| {
        seen.lock().unwrap().push(("calendar", path.to_owned()));
        Ok(Arc::new(FixedCalendar(common::weekday())) as Arc<dyn Calendar>)
    }
}

async fn status(client: &reqwest::Client, url: String) -> u16 {
    client.get(url).send().await.unwrap().status().as_u16()
}

/// 未ready中はroute/healthが503、conditionsは200。読込完了でreadyになる。
/// 同じ正規化済みのreleaseディレクトリを両方の読込に渡す（`current` を足さない）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delayed_ready_after_the_engine_loads() {
    let release = release();
    let config = common::test_config(&release.given, &release.key);
    let seen: Seen = Arc::default();
    let (go, wait) = mpsc::channel::<()>();
    let engine_seen = seen.clone();
    let loaders = Loaders {
        calendar: Box::new(calendar_ok(&seen)),
        engine: Box::new(move |path| {
            engine_seen
                .lock()
                .unwrap()
                .push(("engine", path.to_owned()));
            wait.recv().unwrap();
            Ok(common::stub_engine("singleRoute") as Arc<dyn RouteEngine>)
        }),
    };
    let server = startup::prepare(&config, loaders).await.unwrap();
    // 祝日表は待受け前に読む
    assert_eq!(seen.lock().unwrap().len(), 1);
    let base = format!("http://{}", server.local_addr());
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let running = tokio::spawn(server.run(async move {
        let _ = stopped.await;
    }));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    assert_eq!(status(&client, format!("{base}/api/health")).await, 503);
    assert_eq!(status(&client, format!("{base}/api/status")).await, 503);
    let route = client
        .post(format!("{base}/api/route"))
        .header("content-type", "application/json")
        .body(common::valid_route_body().to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(route.status(), 503);
    let body: serde_json::Value = route.json().await.unwrap();
    assert_eq!(body["error"]["code"], "data_unavailable");
    assert_eq!(
        status(
            &client,
            format!("{base}/api/conditions?datetime=2026-10-24T09:00%2B09:00")
        )
        .await,
        200
    );

    go.send(()).unwrap();
    let started = std::time::Instant::now();
    while status(&client, format!("{base}/api/health")).await != 200 {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "readyにならない"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        *seen.lock().unwrap(),
        [
            ("calendar", release.canonical.clone()),
            ("engine", release.canonical.clone()),
        ]
    );

    stop.send(()).unwrap();
    assert!(running.await.unwrap().is_ok(), "停止要求で正常終了する");
}

/// 事故データの読込失敗で待受けをやめ、`Err(Data)` を返す（main.rsが非0で終了する）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_load_failure_stops_the_server() {
    let release = release();
    let config = common::test_config(&release.given, &release.key);
    let seen: Seen = Arc::default();
    let loaders = Loaders {
        calendar: Box::new(calendar_ok(&seen)),
        engine: Box::new(|_| {
            std::thread::sleep(Duration::from_millis(100));
            Err(LoadError::Invalid("ハッシュ不一致".into()))
        }),
    };
    let server = startup::prepare(&config, loaders).await.unwrap();
    let addr = server.local_addr();
    let result = tokio::time::timeout(Duration::from_secs(5), server.run(std::future::pending()))
        .await
        .expect("読込失敗で止まる");
    assert!(
        matches!(&result, Err(StartupError::Data(LoadError::Invalid(m))) if m == "ハッシュ不一致"),
        "{result:?}"
    );
    assert!(
        tokio::net::TcpStream::connect(addr).await.is_err(),
        "待受けをやめている"
    );
}

/// 集計エンジンの読込がpanicしても停止する。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_loader_panic_stops_the_server() {
    let release = release();
    let config = common::test_config(&release.given, &release.key);
    let seen: Seen = Arc::default();
    let loaders = Loaders {
        calendar: Box::new(calendar_ok(&seen)),
        engine: Box::new(|_| panic!("テスト用のpanic")),
    };
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        startup::run(&config, loaders, std::future::pending()),
    )
    .await
    .expect("panicで止まる");
    assert!(matches!(result, Err(StartupError::Data(_))), "{result:?}");
}

/// 祝日表の読込失敗・releaseが無いときは、待受け前に `Err(Data)`。
#[tokio::test]
async fn calendar_failure_and_missing_release_fail_before_listening() {
    let release = release();
    let config = common::test_config(&release.given, &release.key);
    let engine_called = Arc::new(Mutex::new(false));
    let called = engine_called.clone();
    let loaders = Loaders {
        calendar: Box::new(|_| Err(LoadError::Invalid("祝日表が不正".into()))),
        engine: Box::new(move |_| {
            *called.lock().unwrap() = true;
            Err(LoadError::Invalid("呼ばれない".into()))
        }),
    };
    let result = startup::prepare(&config, loaders).await;
    assert!(matches!(result, Err(StartupError::Data(_))));
    assert!(!*engine_called.lock().unwrap());

    let missing = common::test_config(&release.given.join("missing"), &release.key);
    let seen: Seen = Arc::default();
    let loaders = Loaders {
        calendar: Box::new(calendar_ok(&seen)),
        engine: Box::new(|_| Err(LoadError::Invalid("呼ばれない".into()))),
    };
    let result = startup::prepare(&missing, loaders).await;
    assert!(matches!(result, Err(StartupError::Data(LoadError::Io(_)))));
    assert!(seen.lock().unwrap().is_empty());
}

/// 実 `data::load_*` は②の実装まで仮実装なので、起動処理はロード失敗と同じく `Err` を返す。
#[tokio::test]
async fn real_loaders_fail_like_a_load_failure_until_data_is_implemented() {
    let release = release();
    let config = common::test_config(&release.given, &release.key);
    let result = startup::run(&config, Loaders::production(), std::future::pending()).await;
    assert!(matches!(result, Err(StartupError::Data(_))), "{result:?}");
}

/// 停止要求で `Ok` を返し、その後は新しい接続を受け付けない。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_returns_ok_and_stops_accepting() {
    let release = release();
    let config = common::test_config(&release.given, &release.key);
    let seen: Seen = Arc::default();
    let loaders = Loaders {
        calendar: Box::new(calendar_ok(&seen)),
        engine: Box::new(|_| Ok(common::stub_engine("singleRoute") as Arc<dyn RouteEngine>)),
    };
    let server = startup::prepare(&config, loaders).await.unwrap();
    let addr = server.local_addr();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let running = tokio::spawn(server.run(async move {
        let _ = stopped.await;
    }));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let started = std::time::Instant::now();
    while status(&client, format!("http://{addr}/api/health")).await != 200 {
        assert!(started.elapsed() < Duration::from_secs(5));
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .expect("停止する")
        .unwrap()
        .unwrap();
    assert!(tokio::net::TcpStream::connect(addr).await.is_err());
}
