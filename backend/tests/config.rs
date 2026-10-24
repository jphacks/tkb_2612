//! 環境変数の検査（architecture.md 6章）。誤設定は起動前に変数名つきで失敗し、値を出さない。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use accident_route_backend::config::{
    Attribution, Config, ConfigError, GateBackend, MAX_CONDITIONS_TIMEOUT_MS, MAX_DURATION_MS,
    MAX_ROUTE_TRANSFER_TIMEOUT_MS, MAX_UPSTREAM_BYTES, MAX_UPSTREAM_QUEUE_CAPACITY,
    MAX_WEATHER_CACHE_TTL_SEC, MAX_WEATHER_FORECAST_DAYS, repo_root,
};

struct Env {
    _temp: tempfile::TempDir,
    vars: HashMap<&'static str, String>,
    key: PathBuf,
}

fn env() -> Env {
    let temp = tempfile::tempdir().unwrap();
    let key = temp.path().join("geoapify.key");
    std::fs::write(&key, "  geo-key-123\n").unwrap();
    let vars = HashMap::from([
        ("OSRM_WALK_BASE_URL", "http://127.0.0.1:5001/".to_owned()),
        ("OSRM_BIKE_BASE_URL", "http://127.0.0.1:5002".to_owned()),
        (
            "OSRM_CAR_BASE_URL",
            "https://osrm.example.invalid/car/".to_owned(),
        ),
        (
            "GEOAPIFY_BASE_URL",
            "https://api.geoapify.example.invalid".to_owned(),
        ),
        ("GEOAPIFY_API_KEY_FILE", key.to_str().unwrap().to_owned()),
        (
            "OPEN_METEO_BASE_URL",
            "https://api.open-meteo.example.invalid".to_owned(),
        ),
        (
            "EXTERNAL_USER_AGENT",
            "accident-route/0.1 (+mailto:ops@example.invalid)".to_owned(),
        ),
        (
            "ROUTING_ATTRIBUTION",
            "© OpenStreetMap contributors".to_owned(),
        ),
        (
            "GEOCODING_ATTRIBUTION",
            r#"[{"text":"Powered by Geoapify","url":"https://www.geoapify.com/"}]"#.to_owned(),
        ),
    ]);
    Env {
        _temp: temp,
        vars,
        key,
    }
}

impl Env {
    fn with(mut self, name: &'static str, value: &str) -> Self {
        self.vars.insert(name, value.to_owned());
        self
    }

    fn without(mut self, name: &'static str) -> Self {
        self.vars.remove(name);
        self
    }

    fn load(&self) -> Result<Config, ConfigError> {
        Config::from_lookup(|name| self.vars.get(name).cloned())
    }

    fn error(&self) -> ConfigError {
        self.load().expect_err("設定の誤りになるはず")
    }
}

#[test]
fn defaults_follow_chapter_6() {
    let config = env().load().unwrap();
    assert_eq!(config.bind, "127.0.0.1:8000".parse::<SocketAddr>().unwrap());
    assert_eq!(
        config.accident_data_dir,
        repo_root().join("backend/data/current")
    );
    assert_eq!(config.frontend_dist_dir, repo_root().join("frontend/dist"));
    assert_eq!(config.geoapify_api_key.expose(), "geo-key-123");
    assert_eq!(config.geocoding_daily_limit, 2_000);
    assert_eq!(config.upstream_timeout, Duration::from_millis(8_000));
    assert_eq!(config.upstream_max_bytes, 12_000_000);
    assert_eq!(config.routing_min_interval, Duration::from_millis(1_050));
    assert_eq!(config.geocoding_min_interval, Duration::from_millis(1_050));
    assert_eq!(config.upstream_queue_capacity, 16);
    assert_eq!(config.request_timeout, Duration::from_millis(35_000));
    assert_eq!(
        config.route_transfer_timeout,
        Duration::from_millis(120_000)
    );
    assert_eq!(config.upstream_gate, GateBackend::Firestore);
    assert_eq!(config.open_meteo_api_key, None);
    assert_eq!(config.weather_timeout, Duration::from_millis(5_000));
    assert_eq!(config.weather_forecast_days, 7);
    assert_eq!(config.weather_cache_ttl, Duration::from_secs(600));
    assert_eq!(config.weather_min_interval, Duration::from_millis(200));
    assert_eq!(config.conditions_timeout, Duration::from_millis(8_000));
    assert_eq!(config.rust_log, "info");
    assert_eq!(
        config.geocoding_attribution,
        [Attribution {
            text: "Powered by Geoapify".to_owned(),
            url: "https://www.geoapify.com/".to_owned(),
        }]
    );
}

#[test]
fn explicit_values_and_paths() {
    let e = env();
    let key = e.key.to_str().unwrap().to_owned();
    let config = e
        .with("ACCIDENT_DATA_DIR", "/srv/release")
        .with("FRONTEND_DIST_DIR", "frontend/build")
        .with("UPSTREAM_GATE", "local")
        .with("OPEN_METEO_API_KEY_FILE", &key)
        .with("REQUEST_TIMEOUT_MS", "100")
        .with("WEATHER_CACHE_TTL_SEC", "0")
        .with("WEATHER_FORECAST_DAYS", "16")
        .with("ROUTING_PROVIDER", "osrm")
        .with("GEOCODING_PROVIDER", "geoapify")
        .with("WEATHER_PROVIDER", "open-meteo")
        .with("RUST_LOG", "debug")
        .load()
        .unwrap();
    assert_eq!(config.accident_data_dir, PathBuf::from("/srv/release"));
    assert_eq!(config.frontend_dist_dir, repo_root().join("frontend/build"));
    assert_eq!(config.upstream_gate, GateBackend::Local);
    assert_eq!(
        config.open_meteo_api_key.as_ref().map(|k| k.expose()),
        Some("geo-key-123")
    );
    assert_eq!(config.request_timeout, Duration::from_millis(100));
    assert_eq!(config.weather_cache_ttl, Duration::ZERO);
    assert_eq!(config.weather_forecast_days, 16);
    assert_eq!(config.rust_log, "debug");
}

/// `BACKEND_BIND` があればそれ、無くて `PORT` があれば `0.0.0.0:$PORT`（未決事項5の暫定）。
#[test]
fn bind_address_priority() {
    let port = env().with("PORT", "8080").load().unwrap();
    assert_eq!(port.bind, "0.0.0.0:8080".parse::<SocketAddr>().unwrap());
    let both = env()
        .with("PORT", "8080")
        .with("BACKEND_BIND", "127.0.0.1:9000")
        .load()
        .unwrap();
    assert_eq!(both.bind, "127.0.0.1:9000".parse::<SocketAddr>().unwrap());
    assert_eq!(env().with("PORT", "70000").error().variable, "PORT");
    assert_eq!(
        env().with("BACKEND_BIND", "localhost").error().variable,
        "BACKEND_BIND"
    );
}

#[test]
fn required_variables_are_checked() {
    for name in [
        "OSRM_WALK_BASE_URL",
        "OSRM_BIKE_BASE_URL",
        "OSRM_CAR_BASE_URL",
        "GEOAPIFY_BASE_URL",
        "GEOAPIFY_API_KEY_FILE",
        "OPEN_METEO_BASE_URL",
        "EXTERNAL_USER_AGENT",
        "ROUTING_ATTRIBUTION",
        "GEOCODING_ATTRIBUTION",
    ] {
        let error = env().without(name).error();
        assert_eq!(
            error,
            ConfigError {
                variable: name,
                problem: "必須です"
            }
        );
        let error = env().with(name, "  ").error();
        assert_eq!(error.variable, name);
    }
}

#[test]
fn invalid_values_are_rejected_with_the_variable_name() {
    let cases: [(&'static str, &str); 30] = [
        ("ROUTING_PROVIDER", "google"),
        ("GEOCODING_PROVIDER", "nominatim"),
        ("WEATHER_PROVIDER", "jma"),
        ("EXTERNAL_USER_AGENT", "bad\nagent"),
        ("OSRM_WALK_BASE_URL", "not a url"),
        ("OSRM_WALK_BASE_URL", "ftp://127.0.0.1/"),
        ("OSRM_WALK_BASE_URL", "mailto:a@example.invalid"),
        ("GEOAPIFY_BASE_URL", "https://api.example.invalid/?apiKey=x"),
        ("GEOAPIFY_BASE_URL", "https://api.example.invalid/#x"),
        (
            "OPEN_METEO_BASE_URL",
            "https://user:pass@api.example.invalid/",
        ),
        ("GEOCODING_ATTRIBUTION", "Powered by Geoapify"),
        ("GEOCODING_ATTRIBUTION", "[]"),
        ("GEOCODING_ATTRIBUTION", r#"[{"text":"x"}]"#),
        (
            "GEOCODING_ATTRIBUTION",
            r#"[{"text":" ","url":"https://a.example/"}]"#,
        ),
        (
            "GEOCODING_ATTRIBUTION",
            r#"[{"text":"x","url":"http://a.example/"}]"#,
        ),
        (
            "GEOCODING_ATTRIBUTION",
            r#"[{"text":"x","url":"https://a.example/","extra":1}]"#,
        ),
        ("GEOCODING_DAILY_LIMIT", "0"),
        ("GEOCODING_DAILY_LIMIT", "-1"),
        ("UPSTREAM_TIMEOUT_MS", "0"),
        ("UPSTREAM_TIMEOUT_MS", "8s"),
        ("UPSTREAM_MAX_BYTES", "0"),
        ("UPSTREAM_QUEUE_CAPACITY", "0"),
        ("REQUEST_TIMEOUT_MS", "1.5"),
        ("ROUTE_TRANSFER_TIMEOUT_MS", "0"),
        ("UPSTREAM_GATE", "redis"),
        ("WEATHER_FORECAST_DAYS", "0"),
        ("WEATHER_FORECAST_DAYS", "17"),
        ("WEATHER_CACHE_TTL_SEC", "601"),
        ("CONDITIONS_TIMEOUT_MS", "10000"),
        ("RUST_LOG", ""),
    ];
    for (name, value) in cases {
        let error = env().with(name, value).error();
        assert_eq!(error.variable, name, "{name}={value}");
    }
}

/// 6章が上限を定めない数値は、名前つきの上限（暫定）まで受け付け、超えたら変数名つきで失敗する。
#[test]
fn numeric_upper_bounds_are_inclusive() {
    let cases = [
        ("UPSTREAM_TIMEOUT_MS", MAX_DURATION_MS),
        ("ROUTING_MIN_INTERVAL_MS", MAX_DURATION_MS),
        ("GEOCODING_MIN_INTERVAL_MS", MAX_DURATION_MS),
        ("REQUEST_TIMEOUT_MS", MAX_DURATION_MS),
        ("WEATHER_TIMEOUT_MS", MAX_DURATION_MS),
        ("WEATHER_MIN_INTERVAL_MS", MAX_DURATION_MS),
        ("ROUTE_TRANSFER_TIMEOUT_MS", MAX_ROUTE_TRANSFER_TIMEOUT_MS),
        ("UPSTREAM_MAX_BYTES", MAX_UPSTREAM_BYTES),
        ("UPSTREAM_QUEUE_CAPACITY", MAX_UPSTREAM_QUEUE_CAPACITY),
        ("WEATHER_CACHE_TTL_SEC", MAX_WEATHER_CACHE_TTL_SEC),
        ("WEATHER_FORECAST_DAYS", MAX_WEATHER_FORECAST_DAYS),
        ("CONDITIONS_TIMEOUT_MS", MAX_CONDITIONS_TIMEOUT_MS),
    ];
    for (name, max) in cases {
        assert!(
            env().with(name, &max.to_string()).load().is_ok(),
            "{name}={max}"
        );
        let error = env().with(name, &(max + 1).to_string()).error();
        assert_eq!(error.variable, name, "{name}={}", max + 1);
    }
}

#[test]
fn secret_files_are_checked() {
    let e = env();
    let dir = e.key.parent().unwrap().to_owned();
    let empty = dir.join("empty.key");
    std::fs::write(&empty, "\n").unwrap();
    let control = dir.join("control.key");
    std::fs::write(&control, "ab\u{7}cd").unwrap();
    for path in [dir.join("missing.key"), empty, control, dir.clone()] {
        for name in ["GEOAPIFY_API_KEY_FILE", "OPEN_METEO_API_KEY_FILE"] {
            let error = env().with(name, path.to_str().unwrap()).error();
            assert_eq!(error.variable, name, "{}", path.display());
        }
    }
}

/// エラー表示・Debug表示に秘密・URLの値を出さない。
#[test]
fn errors_and_debug_output_do_not_include_values() {
    let error = env()
        .with(
            "GEOAPIFY_BASE_URL",
            "https://user:p4ss@api.example.invalid/?apiKey=S3CRET",
        )
        .error();
    let text = format!("{error} {error:?}");
    for secret in ["p4ss", "S3CRET", "example.invalid"] {
        assert!(!text.contains(secret), "{text}");
    }
    let config = env().load().unwrap();
    assert!(!format!("{config:?}").contains("geo-key-123"));
}
