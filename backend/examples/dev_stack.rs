//! C4用の偽の上流（担当：③）。本番の処理からは使わない。
//!
//! 偽OSRM・偽Geoapify・偽Open-Meteoをループバックの空きポートで起動し、それらを指す
//! 環境変数の一式を標準出力へ `export NAME='value'` の形で出す（同じ内容を一時ディレクトリの
//! `env.sh` にも書く）。外部のネットワークには通信しない。手順は README.md「C4のローカル起動」。
//!
//! ```text
//! cargo run --example dev_stack                            # 偽上流だけ（Ctrl-Cで停止）
//! cargo run --example dev_stack -- --example twoSelected   # 偽OSRMが返す例を選ぶ
//! cargo run --example dev_stack -- --simple                # 簡易モード（C4の合格には使えない）
//! cargo run --example dev_stack -- --simple noAccidents    # 簡易モードで返す例を選ぶ
//! ```
//!
//! fixtureの例はどれも出発地・目的地が同じなので、座標だけでは例を区別できない。
//! `--example` を付けると、偽OSRMはmodeが一致する限りその例の経路を返す（付けなければ、
//! 座標とmodeが一致する最初の例）。
//!
//! 簡易モードは、人工release（②）が無い間の予行用に、`StubEngine`（`Fixed`）と
//! `FixedCalendar` で APIサーバを `127.0.0.1:8000` に起動する。経路はどの入力でも
//! 選んだ例（既定 `twoSelected`）の結果になり、実エンジンではない。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use accident_route_backend::config::Config;
use accident_route_backend::contract::{
    Calendar, DayClassification, DayTypeReason, LatLon, Mode, RouteEngine,
};
use accident_route_backend::logging;
use accident_route_backend::startup::{self, Loaders};
use accident_route_backend::testing::fixtures::{
    RouteExample, expected_route_outcome, fixture_dataset_info, route_examples,
};
use accident_route_backend::testing::{FixedCalendar, StubEngine, StubEvaluation};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use chrono::{Duration, DurationRound, Timelike, Utc};
use serde_json::{Value, json};
use tokio::net::TcpListener;

/// 簡易モードの待受け（frontend/vite.config.ts の `/api` の転送先）。
const SIMPLE_BIND: &str = "127.0.0.1:8000";
const DEFAULT_EXAMPLE: &str = "twoSelected";
/// 偽Geoapifyで、この語を含む検索は0件を返す。
const EMPTY_QUERY_MARKER: &str = "該当なし";

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (simple, selected) = match args.as_slice() {
        [] => (false, None),
        [flag, name] if flag == "--example" => (false, Some(name.clone())),
        [flag] if flag == "--simple" => (true, Some(DEFAULT_EXAMPLE.to_owned())),
        [flag, name] if flag == "--simple" => (true, Some(name.clone())),
        _ => {
            eprintln!("使い方: dev_stack [--example 例の名前 | --simple [例の名前]]");
            return std::process::ExitCode::FAILURE;
        }
    };
    let examples = Arc::new(route_examples());
    if let Some(name) = &selected
        && !examples.iter().any(|e| &e.name == name)
    {
        let names: Vec<&str> = examples.iter().map(|e| e.name.as_str()).collect();
        eprintln!("例 {name} がありません（{}）", names.join("、"));
        return std::process::ExitCode::FAILURE;
    }

    let osrm = start(axum::Router::new().fallback({
        let examples = examples.clone();
        let selected = selected.clone();
        move |uri: Uri| async move { fake_osrm(&uri, &examples, selected.as_deref()) }
    }))
    .await;
    let geoapify = start(axum::Router::new().fallback({
        let examples = examples.clone();
        move |uri: Uri| async move { fake_geoapify(&uri, &examples) }
    }))
    .await;
    let open_meteo =
        start(axum::Router::new().fallback(|uri: Uri| async move { fake_open_meteo(&uri) })).await;

    let temp = tempfile::tempdir().expect("一時ディレクトリを作れません");
    let key_file = temp.path().join("geoapify-dummy.key");
    std::fs::write(&key_file, "dummy-geoapify-key\n").expect("ダミー鍵を書けません");
    let vars = env_vars(&osrm, &geoapify, &open_meteo, &key_file);
    let script: String = vars
        .iter()
        .map(|(name, value)| format!("export {name}='{value}'\n"))
        .collect();
    let env_file = temp.path().join("env.sh");
    std::fs::write(&env_file, &script).expect("env.shを書けません");
    print!("{script}");
    eprintln!("# 偽上流を起動しました（同じ内容: {}）", env_file.display());

    match selected.filter(|_| simple) {
        None => {
            eprintln!(
                "# 別の端末で上の環境変数を設定し、backendを起動してください。Ctrl-Cで停止します。"
            );
            startup::shutdown_signal().await;
            std::process::ExitCode::SUCCESS
        }
        Some(name) => run_simple(&name, vars, temp.path()).await,
    }
}

/// ループバックの空きポートで待ち受け、そのベースURLを返す。
async fn start(app: axum::Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("ループバックで待ち受けられません");
    let addr = listener
        .local_addr()
        .expect("待受けアドレスを取得できません");
    tokio::spawn(startup::serve(listener, app, std::future::pending()));
    format!("http://{addr}/")
}

fn env_vars(
    osrm: &str,
    geoapify: &str,
    open_meteo: &str,
    key_file: &Path,
) -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        ("OSRM_WALK_BASE_URL", osrm.to_owned()),
        ("OSRM_BIKE_BASE_URL", osrm.to_owned()),
        ("OSRM_CAR_BASE_URL", osrm.to_owned()),
        ("GEOAPIFY_BASE_URL", geoapify.to_owned()),
        ("GEOAPIFY_API_KEY_FILE", key_file.display().to_string()),
        ("OPEN_METEO_BASE_URL", open_meteo.to_owned()),
        (
            "EXTERNAL_USER_AGENT",
            "accident-route-dev/0.1 (+https://github.com/jphacks/tkb_2612)".to_owned(),
        ),
        (
            "ROUTING_ATTRIBUTION",
            "© OpenStreetMap contributors".to_owned(),
        ),
        (
            "GEOCODING_ATTRIBUTION",
            r#"[{"text":"Powered by Geoapify","url":"https://www.geoapify.com/"}]"#.to_owned(),
        ),
        ("UPSTREAM_GATE", "local".to_owned()),
    ])
}

/// `StubEngine`（`Fixed`）＋`FixedCalendar`（平日）でAPIサーバを起動する。
async fn run_simple(
    name: &str,
    mut vars: BTreeMap<&'static str, String>,
    temp: &Path,
) -> std::process::ExitCode {
    // 8000番が使用中の環境では、環境変数 `BACKEND_BIND` で待受けを変えられる
    let bind = std::env::var("BACKEND_BIND").unwrap_or_else(|_| SIMPLE_BIND.to_owned());
    vars.insert("BACKEND_BIND", bind);
    let release = temp.join("release");
    std::fs::create_dir(&release).expect("releaseの代わりのディレクトリを作れません");
    vars.insert("ACCIDENT_DATA_DIR", release.display().to_string());
    let config = match Config::from_lookup(|n| vars.get(n).cloned()) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    if let Err(error) = logging::init(&config.rust_log) {
        eprintln!("{error}");
        return std::process::ExitCode::FAILURE;
    }
    let outcome = expected_route_outcome(name);
    let loaders = Loaders {
        calendar: Box::new(|_| {
            Ok(Arc::new(FixedCalendar(DayClassification {
                reason: DayTypeReason::Weekday,
                holiday_table_covered: true,
            })) as Arc<dyn Calendar>)
        }),
        engine: Box::new(move |_| {
            Ok(Arc::new(StubEngine::new(
                fixture_dataset_info(),
                StubEvaluation::Fixed(Box::new(outcome)),
            )) as Arc<dyn RouteEngine>)
        }),
    };
    eprintln!(
        "# 簡易モード：例 {name} の結果を返すAPIを http://{} で起動します（C4の合格には使えません）",
        config.bind
    );
    match startup::run(&config, loaders, startup::shutdown_signal()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn json_response(status: StatusCode, body: &Value) -> Response {
    (
        status,
        [("content-type", "application/json; charset=utf-8")],
        body.to_string(),
    )
        .into_response()
}

/// `GET /route/v1/{profile}/{lon,lat;…}`。選んだ例（`selected`）のmodeが一致すればその例の、
/// でなければ出発地・目的地・modeが一致する例の固定経路を返す。
/// 一致する例が無ければ、同じmodeの最初の例（無ければ最初の例）の経路を返す。
fn fake_osrm(uri: &Uri, examples: &[RouteExample], selected: Option<&str>) -> Response {
    let segments: Vec<&str> = uri.path().trim_start_matches('/').split('/').collect();
    let [.., "route", "v1", profile, points] = segments.as_slice() else {
        return json_response(StatusCode::NOT_FOUND, &json!({"code": "InvalidUrl"}));
    };
    let mode = match *profile {
        "walking" => Mode::Walk,
        "cycling" => Mode::Bike,
        "driving" => Mode::Car,
        _ => return json_response(StatusCode::BAD_REQUEST, &json!({"code": "InvalidOptions"})),
    };
    let text = |p: LatLon| format!("{:.7},{:.7}", p.lon, p.lat);
    let points: Vec<&str> = points.split(';').collect();
    let (Some(origin), Some(dest)) = (points.first(), points.last()) else {
        return json_response(StatusCode::BAD_REQUEST, &json!({"code": "InvalidQuery"}));
    };
    let example = examples
        .iter()
        .find(|e| Some(e.name.as_str()) == selected && e.query.mode() == mode)
        .or_else(|| {
            examples.iter().find(|e| {
                e.query.mode() == mode
                    && text(e.query.origin()) == *origin
                    && text(e.query.dest()) == *dest
            })
        })
        .or_else(|| examples.iter().find(|e| e.query.mode() == mode))
        .or_else(|| examples.first());
    let Some(example) = example else {
        return json_response(StatusCode::OK, &json!({"code": "NoRoute", "routes": []}));
    };
    let routes: Vec<Value> = example
        .routes
        .iter()
        .map(|r| {
            json!({
                "duration": r.duration_s,
                "distance": r.distance_m,
                "geometry": {
                    "type": "LineString",
                    "coordinates": r.coordinates.iter().map(|p| json!([p.lon, p.lat])).collect::<Vec<_>>(),
                },
            })
        })
        .collect();
    json_response(StatusCode::OK, &json!({"code": "Ok", "routes": routes}))
}

/// `GET …?text=…&format=json`。どの検索語にも、fixtureの例の出発地・目的地（重複を除く）を候補として返す。
/// 検索語に [`EMPTY_QUERY_MARKER`] を含むときは0件。
fn fake_geoapify(uri: &Uri, examples: &[RouteExample]) -> Response {
    let text = query_param(uri, "text").unwrap_or_default();
    if text.contains(EMPTY_QUERY_MARKER) {
        return json_response(StatusCode::OK, &json!({"results": []}));
    }
    let mut places: Vec<(LatLon, String)> = Vec::new();
    for e in examples {
        for (p, role) in [(e.query.origin(), "出発地"), (e.query.dest(), "目的地")] {
            if !places.iter().any(|(q, _)| *q == p) {
                places.push((p, format!("{}の{role}（偽Geoapify）", e.name)));
            }
        }
    }
    let results: Vec<Value> = places
        .iter()
        .take(5)
        .map(|(p, label)| json!({"lat": p.lat, "lon": p.lon, "formatted": label}))
        .collect();
    json_response(StatusCode::OK, &json!({"results": results}))
}

/// `GET …?forecast_days=N`。現在時刻（東京）を含む、今日0時からN日分の毎時の天気を返す。
/// 天気は日ごとに晴（1）・雨（61）・雪（71）を繰り返し、現在は晴。
fn fake_open_meteo(uri: &Uri) -> Response {
    let days: i64 = query_param(uri, "forecast_days")
        .and_then(|d| d.parse().ok())
        .unwrap_or(7);
    let now = (Utc::now() + Duration::hours(9)).naive_utc();
    let current = now
        .duration_trunc(Duration::minutes(15))
        .unwrap_or(now.with_second(0).unwrap_or(now));
    let midnight = now.date().and_hms_opt(0, 0, 0).unwrap_or(now);
    let hours = 0..days.clamp(1, 16) * 24;
    let format = |t: chrono::NaiveDateTime| t.format("%Y-%m-%dT%H:%M").to_string();
    let times: Vec<String> = hours
        .clone()
        .map(|h| format(midnight + Duration::hours(h)))
        .collect();
    let codes: Vec<i64> = hours.map(|h| [1, 61, 71][(h / 24 % 3) as usize]).collect();
    json_response(
        StatusCode::OK,
        &json!({
            "utc_offset_seconds": 32400,
            "current": {"time": format(current), "weather_code": 1},
            "hourly": {"time": times, "weather_code": codes},
        }),
    )
}

fn query_param(uri: &Uri, name: &str) -> Option<String> {
    form_urlencoded::parse(uri.query()?.as_bytes())
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}
