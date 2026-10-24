//! 環境変数の読込と検査（architecture.md 6章）。
//!
//! 起動時に型・範囲・必須値を検査し、誤設定なら起動を止める。エラーには変数名と
//! 問題の種類だけを出し、入力された値（秘密・URLのクエリを含み得る）は出さない。
//!
//! `PORT` と `BACKEND_BIND` の優先関係（6章は「Cloud Runでは `0.0.0.0:$PORT`」とだけ記す）は
//! 次のとおりに決めた：`BACKEND_BIND` があればそれを使い、無くて `PORT` があれば
//! `0.0.0.0:$PORT`、どちらも無ければ `127.0.0.1:8000`。
//! 相対パスはrepo root（このクレートの親ディレクトリ）を基準に解決する。本番は絶対パスを渡す。

use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

/// 秘密値。`Debug` で中身を出さない。
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// 帰属表示1件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribution {
    pub text: String,
    pub url: String,
}

/// 上流ゲートの共有状態の保存先。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateBackend {
    /// 全インスタンス共通（本番必須）。
    Firestore,
    /// 単一プロセス内（ローカル開発専用）。
    Local,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub accident_data_dir: PathBuf,
    pub frontend_dist_dir: PathBuf,
    pub osrm_walk_base_url: reqwest::Url,
    pub osrm_bike_base_url: reqwest::Url,
    pub osrm_car_base_url: reqwest::Url,
    pub geoapify_base_url: reqwest::Url,
    pub geoapify_api_key: Secret,
    pub geocoding_daily_limit: u64,
    pub external_user_agent: String,
    pub routing_attribution: String,
    pub geocoding_attribution: Vec<Attribution>,
    pub upstream_timeout: Duration,
    pub upstream_max_bytes: usize,
    pub routing_min_interval: Duration,
    pub geocoding_min_interval: Duration,
    pub upstream_queue_capacity: usize,
    pub request_timeout: Duration,
    pub route_transfer_timeout: Duration,
    pub upstream_gate: GateBackend,
    pub open_meteo_base_url: reqwest::Url,
    pub open_meteo_api_key: Option<Secret>,
    pub weather_timeout: Duration,
    pub weather_forecast_days: u8,
    pub weather_cache_ttl: Duration,
    pub weather_min_interval: Duration,
    pub conditions_timeout: Duration,
    pub rust_log: String,
}

/// 設定の誤り。値そのものは含めない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub variable: &'static str,
    pub problem: &'static str,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "環境変数 {} が不正です: {}", self.variable, self.problem)
    }
}

impl std::error::Error for ConfigError {}

/// repo root（相対パスの基準）。
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("backend/ の親がrepo root")
        .to_path_buf()
}

impl Config {
    /// プロセスの環境変数から読む。
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// `lookup` から読む（テスト用に差し替えられる）。
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let env = Env {
            lookup: &lookup,
            root: repo_root(),
        };
        env.provider("ROUTING_PROVIDER", "osrm")?;
        env.provider("GEOCODING_PROVIDER", "geoapify")?;
        env.provider("WEATHER_PROVIDER", "open-meteo")?;
        let external_user_agent = env.required("EXTERNAL_USER_AGENT")?;
        if reqwest::header::HeaderValue::from_str(&external_user_agent).is_err() {
            return Err(env.error("EXTERNAL_USER_AGENT", "ヘッダーに使えない文字があります"));
        }
        let routing_attribution = env.required("ROUTING_ATTRIBUTION")?;
        let upstream_timeout = env.millis("UPSTREAM_TIMEOUT_MS", 8_000, 1, 600_000)?;
        Ok(Self {
            bind: env.bind()?,
            accident_data_dir: env.path("ACCIDENT_DATA_DIR", "backend/data/current")?,
            frontend_dist_dir: env.path("FRONTEND_DIST_DIR", "frontend/dist")?,
            osrm_walk_base_url: env.base_url("OSRM_WALK_BASE_URL")?,
            osrm_bike_base_url: env.base_url("OSRM_BIKE_BASE_URL")?,
            osrm_car_base_url: env.base_url("OSRM_CAR_BASE_URL")?,
            geoapify_base_url: env.base_url("GEOAPIFY_BASE_URL")?,
            geoapify_api_key: env
                .secret_file("GEOAPIFY_API_KEY_FILE")?
                .ok_or_else(|| env.error("GEOAPIFY_API_KEY_FILE", "必須です"))?,
            geocoding_daily_limit: env.integer("GEOCODING_DAILY_LIMIT", 2_000, 1, u64::MAX)?,
            external_user_agent,
            routing_attribution,
            geocoding_attribution: env.attribution("GEOCODING_ATTRIBUTION")?,
            upstream_timeout,
            upstream_max_bytes: env.integer("UPSTREAM_MAX_BYTES", 12_000_000, 1, 1 << 32)? as usize,
            routing_min_interval: env.millis("ROUTING_MIN_INTERVAL_MS", 1_050, 0, 600_000)?,
            geocoding_min_interval: env.millis("GEOCODING_MIN_INTERVAL_MS", 1_050, 0, 600_000)?,
            upstream_queue_capacity: env.integer("UPSTREAM_QUEUE_CAPACITY", 16, 1, 10_000)?
                as usize,
            request_timeout: env.millis("REQUEST_TIMEOUT_MS", 35_000, 1, 600_000)?,
            route_transfer_timeout: env.millis(
                "ROUTE_TRANSFER_TIMEOUT_MS",
                120_000,
                1,
                3_600_000,
            )?,
            upstream_gate: match env.optional("UPSTREAM_GATE")?.as_deref() {
                None | Some("firestore") => GateBackend::Firestore,
                Some("local") => GateBackend::Local,
                Some(_) => {
                    return Err(env.error("UPSTREAM_GATE", "firestore または local を指定します"));
                }
            },
            open_meteo_base_url: env.base_url("OPEN_METEO_BASE_URL")?,
            open_meteo_api_key: env.secret_file("OPEN_METEO_API_KEY_FILE")?,
            weather_timeout: env.millis("WEATHER_TIMEOUT_MS", 5_000, 1, 600_000)?,
            weather_forecast_days: env.integer("WEATHER_FORECAST_DAYS", 7, 1, 16)? as u8,
            weather_cache_ttl: Duration::from_secs(env.integer(
                "WEATHER_CACHE_TTL_SEC",
                600,
                0,
                86_400,
            )?),
            weather_min_interval: env.millis("WEATHER_MIN_INTERVAL_MS", 200, 0, 600_000)?,
            // クライアントの10秒より短くする
            conditions_timeout: env.millis("CONDITIONS_TIMEOUT_MS", 8_000, 1, 9_999)?,
            rust_log: env
                .optional("RUST_LOG")?
                .unwrap_or_else(|| "info".to_owned()),
        })
    }
}

struct Env<'a> {
    lookup: &'a dyn Fn(&str) -> Option<String>,
    root: PathBuf,
}

impl Env<'_> {
    fn error(&self, variable: &'static str, problem: &'static str) -> ConfigError {
        ConfigError { variable, problem }
    }

    /// 空文字列は未設定と区別し、誤りとして扱う。
    fn optional(&self, name: &'static str) -> Result<Option<String>, ConfigError> {
        match (self.lookup)(name) {
            None => Ok(None),
            Some(v) if v.trim().is_empty() => Err(self.error(name, "空です")),
            Some(v) => Ok(Some(v)),
        }
    }

    fn required(&self, name: &'static str) -> Result<String, ConfigError> {
        self.optional(name)?
            .ok_or_else(|| self.error(name, "必須です"))
    }

    fn provider(&self, name: &'static str, supported: &str) -> Result<(), ConfigError> {
        match self.optional(name)? {
            None => Ok(()),
            Some(v) if v == supported => Ok(()),
            Some(_) => Err(self.error(name, "未対応の提供元です")),
        }
    }

    fn integer(
        &self,
        name: &'static str,
        default: u64,
        min: u64,
        max: u64,
    ) -> Result<u64, ConfigError> {
        let Some(text) = self.optional(name)? else {
            return Ok(default);
        };
        let value: u64 = text
            .parse()
            .map_err(|_| self.error(name, "非負の整数ではありません"))?;
        if !(min..=max).contains(&value) {
            return Err(self.error(name, "範囲外です"));
        }
        Ok(value)
    }

    fn millis(
        &self,
        name: &'static str,
        default: u64,
        min: u64,
        max: u64,
    ) -> Result<Duration, ConfigError> {
        self.integer(name, default, min, max)
            .map(Duration::from_millis)
    }

    fn bind(&self) -> Result<SocketAddr, ConfigError> {
        if let Some(text) = self.optional("BACKEND_BIND")? {
            return text
                .parse()
                .map_err(|_| self.error("BACKEND_BIND", "host:port の形式ではありません"));
        }
        if let Some(text) = self.optional("PORT")? {
            let port: u16 = text
                .parse()
                .map_err(|_| self.error("PORT", "ポート番号ではありません"))?;
            return Ok(SocketAddr::from(([0, 0, 0, 0], port)));
        }
        Ok(SocketAddr::from(([127, 0, 0, 1], 8000)))
    }

    fn resolve(&self, path: &str) -> PathBuf {
        let path = Path::new(path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        }
    }

    fn path(&self, name: &'static str, default: &str) -> Result<PathBuf, ConfigError> {
        Ok(self.resolve(&self.optional(name)?.unwrap_or_else(|| default.to_owned())))
    }

    /// `/v1/...` 等を付加するベースURL。http(s)で、クエリ・フラグメント・認証情報を持たない。
    fn base_url(&self, name: &'static str) -> Result<reqwest::Url, ConfigError> {
        let text = self.required(name)?;
        let url = reqwest::Url::parse(&text).map_err(|_| self.error(name, "URLではありません"))?;
        if !matches!(url.scheme(), "http" | "https") || url.cannot_be_a_base() {
            return Err(self.error(name, "http(s)のURLを指定します"));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(self.error(name, "クエリ・フラグメントを含めません"));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(self.error(name, "認証情報を含めません"));
        }
        Ok(url)
    }

    /// 秘密値を置いたファイルを読む。前後の空白（末尾の改行等）を除く。
    fn secret_file(&self, name: &'static str) -> Result<Option<Secret>, ConfigError> {
        let Some(path) = self.optional(name)? else {
            return Ok(None);
        };
        let text = std::fs::read_to_string(self.resolve(&path))
            .map_err(|_| self.error(name, "ファイルを読めません"))?;
        let key = text.trim();
        if key.is_empty() {
            return Err(self.error(name, "ファイルが空です"));
        }
        if key.chars().any(char::is_control) {
            return Err(self.error(name, "制御文字を含みます"));
        }
        Ok(Some(Secret(key.to_owned())))
    }

    /// `{text,url}` の配列のJSON。1件以上、textは非空、urlは `https://` で始まる。
    fn attribution(&self, name: &'static str) -> Result<Vec<Attribution>, ConfigError> {
        let text = self.required(name)?;
        let invalid = || self.error(name, "{text,url}の配列のJSONではありません");
        let value: Value = serde_json::from_str(&text).map_err(|_| invalid())?;
        let items = value
            .as_array()
            .filter(|a| !a.is_empty())
            .ok_or_else(invalid)?;
        items
            .iter()
            .map(|item| {
                let object = item
                    .as_object()
                    .filter(|o| o.len() == 2)
                    .ok_or_else(invalid)?;
                let text = object
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|t| !t.trim().is_empty())
                    .ok_or_else(invalid)?;
                let url = object
                    .get("url")
                    .and_then(Value::as_str)
                    .filter(|u| {
                        u.starts_with("https://")
                            && reqwest::Url::parse(u).is_ok_and(|p| p.host().is_some())
                    })
                    .ok_or_else(invalid)?;
                Ok(Attribution {
                    text: text.to_owned(),
                    url: url.to_owned(),
                })
            })
            .collect()
    }
}
