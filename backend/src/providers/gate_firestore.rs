//! Firestore Standardのトランザクションで共有する上流ゲートの保存方式（`UPSTREAM_GATE=firestore`）。
//!
//! 判定は [`apply`] をそのまま使い、保存先だけを替える。通信はトランザクションの外で行い、
//! ここではゲートの状態の読取・書込みだけを行う。1提供元を1文書（`upstream_gates/{key}`）にし、
//! 文字列フィールド `state` に [`GateState`] のJSONを置く（提供元の識別子・開始時刻・リースの
//! 期限と世代番号・集計件数だけで、住所・座標・経路・利用者ID・リクエストIDを含まない）。
//!
//! 認証とプロジェクトはCloud Runのメタデータサーバから得る（architecture.md 6章に
//! Firestore用の環境変数が無いため、新しい変数を作らない。コレクション名・データベースは
//! 暫定の定数で、統括の決定待ち）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::contract::BoxFuture;
use crate::providers::clock::Clock;
use crate::providers::gate::{Decision, GateOp, GateState, GateStore, StoreError, apply};

/// ゲートの状態を置くコレクション（暫定）。
pub const COLLECTION: &str = "upstream_gates";
/// データベース（暫定）。
pub const DATABASE: &str = "(default)";
/// 競合（ABORTED）時に読み直す回数の上限。超えたら共有状態の競合未解消として失敗。
pub const MAX_ATTEMPTS: usize = 3;
/// Firestore・メタデータサーバへの1通信の期限。
pub const CALL_TIMEOUT: Duration = Duration::from_secs(2);
/// 判定（`batchGet` の後）から `commit` の応答を受け取るまでに許す遅れ（暫定。仕様に値が無く、
/// 統括の決定待ち）。共有状態にはこの分だけ遅い開始時刻を記録し、これを超えて届いた許可では
/// 送信しない（[`GateStore::start_reserve`]）。
pub const START_RESERVE: Duration = Duration::from_millis(500);

const API_ROOT: &str = "https://firestore.googleapis.com/v1/";
const METADATA_ROOT: &str = "http://metadata.google.internal/computeMetadata/v1/";

/// アクセストークンの取得元。
pub trait TokenSource: Send + Sync {
    fn token(&self) -> BoxFuture<'_, Result<String, StoreError>>;
}

/// 固定のトークン（テスト用の偽Firestoreへ接続するとき等）。
#[derive(Clone)]
pub struct StaticToken(pub String);

impl std::fmt::Debug for StaticToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StaticToken(..)")
    }
}

impl TokenSource for StaticToken {
    fn token(&self) -> BoxFuture<'_, Result<String, StoreError>> {
        Box::pin(async move { Ok(self.0.clone()) })
    }
}

/// Cloud Runのメタデータサーバからサービスアカウントのトークンを得る。期限前に取り直す。
pub struct MetadataToken {
    client: reqwest::Client,
    cached: tokio::sync::Mutex<Option<(String, Instant)>>,
}

/// キャッシュしたトークンを表示しない。
impl std::fmt::Debug for MetadataToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetadataToken").finish_non_exhaustive()
    }
}

impl MetadataToken {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            cached: tokio::sync::Mutex::new(None),
        }
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

impl TokenSource for MetadataToken {
    fn token(&self) -> BoxFuture<'_, Result<String, StoreError>> {
        Box::pin(async move {
            let mut cached = self.cached.lock().await;
            if let Some((token, until)) = cached.as_ref()
                && Instant::now() < *until
            {
                return Ok(token.clone());
            }
            let body =
                metadata_get(&self.client, "instance/service-accounts/default/token").await?;
            let parsed: TokenResponse = serde_json::from_slice(&body).map_err(|_| StoreError)?;
            // 期限の1分前に取り直す
            let until = Instant::now() + Duration::from_secs(parsed.expires_in.saturating_sub(60));
            *cached = Some((parsed.access_token.clone(), until));
            Ok(parsed.access_token)
        })
    }
}

async fn metadata_get(client: &reqwest::Client, path: &str) -> Result<bytes::Bytes, StoreError> {
    let response = client
        .get(format!("{METADATA_ROOT}{path}"))
        .header("Metadata-Flavor", "Google")
        .timeout(CALL_TIMEOUT)
        .send()
        .await
        .map_err(|_| StoreError)?;
    if !response.status().is_success() {
        return Err(StoreError);
    }
    response.bytes().await.map_err(|_| StoreError)
}

/// メタデータサーバからプロジェクトIDを得る。起動時に1回呼ぶ。
pub async fn metadata_project_id(client: &reqwest::Client) -> Result<String, StoreError> {
    let body = metadata_get(client, "project/project-id").await?;
    let id = String::from_utf8(body.to_vec()).map_err(|_| StoreError)?;
    let id = id.trim();
    if id.is_empty() {
        return Err(StoreError);
    }
    Ok(id.to_owned())
}

/// Firestoreに保存する [`GateStore`]。
pub struct FirestoreGateStore {
    client: reqwest::Client,
    /// `https://…/v1/projects/{p}/databases/{d}/documents`
    documents_url: String,
    /// `projects/{p}/databases/{d}/documents`
    documents_name: String,
    tokens: Arc<dyn TokenSource>,
    start_reserve: Duration,
}

impl std::fmt::Debug for FirestoreGateStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FirestoreGateStore").finish_non_exhaustive()
    }
}

/// 1回の試行の結果。
enum Attempt {
    Done(Decision),
    Conflict,
}

impl FirestoreGateStore {
    /// 本番の接続先（メタデータサーバの認証、`(default)` データベース）。
    pub fn production(client: reqwest::Client, project: &str) -> Self {
        let tokens = Arc::new(MetadataToken::new(client.clone()));
        Self::new(client, API_ROOT, project, DATABASE, tokens)
    }

    /// 接続先を指定して作る（テストの偽Firestore用）。`api_root` は `…/v1/` まで。
    pub fn new(
        client: reqwest::Client,
        api_root: &str,
        project: &str,
        database: &str,
        tokens: Arc<dyn TokenSource>,
    ) -> Self {
        let documents_name = format!("projects/{project}/databases/{database}/documents");
        Self {
            client,
            documents_url: format!("{}/{documents_name}", api_root.trim_end_matches('/')),
            documents_name,
            tokens,
            start_reserve: START_RESERVE,
        }
    }

    /// 判定から応答受取りまでに許す遅れを変える（テスト用）。
    pub fn with_start_reserve(mut self, reserve: Duration) -> Self {
        self.start_reserve = reserve;
        self
    }

    async fn call(&self, token: &str, method: &str, body: Value) -> Result<Value, CallError> {
        let response = self
            .client
            .post(format!("{}:{method}", self.documents_url))
            .bearer_auth(token)
            .json(&body)
            .timeout(CALL_TIMEOUT)
            .send()
            .await
            .map_err(|_| CallError::Failed)?;
        let status = response.status();
        if status == reqwest::StatusCode::CONFLICT {
            return Err(CallError::Conflict);
        }
        if !status.is_success() {
            return Err(CallError::Failed);
        }
        response.json().await.map_err(|_| CallError::Failed)
    }

    async fn attempt(
        &self,
        key: &str,
        op: GateOp<'_>,
        clock: &dyn Clock,
    ) -> Result<Attempt, StoreError> {
        let token = self.tokens.token().await?;
        let begun = self
            .call(
                &token,
                "beginTransaction",
                json!({"options": {"readWrite": {}}}),
            )
            .await
            .map_err(|_| StoreError)?;
        let transaction = begun
            .get("transaction")
            .and_then(Value::as_str)
            .ok_or(StoreError)?
            .to_owned();
        let name = format!("{}/{COLLECTION}/{key}", self.documents_name);
        let result = self
            .read_apply_commit(&token, &transaction, &name, op, clock)
            .await;
        if !matches!(result, Ok(Attempt::Done(_))) {
            // 失敗・競合したトランザクションは捨てる（失敗しても期限で消える）
            let _ = self
                .call(&token, "rollback", json!({"transaction": transaction}))
                .await;
        }
        result
    }

    async fn read_apply_commit(
        &self,
        token: &str,
        transaction: &str,
        name: &str,
        op: GateOp<'_>,
        clock: &dyn Clock,
    ) -> Result<Attempt, StoreError> {
        let read = match self
            .call(
                token,
                "batchGet",
                json!({"documents": [name], "transaction": transaction}),
            )
            .await
        {
            Ok(read) => read,
            Err(CallError::Conflict) => return Ok(Attempt::Conflict),
            Err(CallError::Failed) => return Err(StoreError),
        };
        let before = parse_batch_get(&read).ok_or(StoreError)?;
        let mut state = before.clone();
        // 判定の時刻は読取りの後に読む（トークン取得・読取りの遅れを開始時刻に含めない）
        let decision = apply(&mut state, op, clock.now_ms());
        if state == before {
            // 書き込みが不要（待ち・上限到達で状態が変わらない）
            let _ = self
                .call(token, "rollback", json!({"transaction": transaction}))
                .await;
            return Ok(Attempt::Done(decision));
        }
        let encoded = serde_json::to_string(&state).map_err(|_| StoreError)?;
        let commit = json!({
            "writes": [{
                "update": {"name": name, "fields": {"state": {"stringValue": encoded}}}
            }],
            "transaction": transaction,
        });
        match self.call(token, "commit", commit).await {
            Ok(_) => Ok(Attempt::Done(decision)),
            Err(CallError::Conflict) => Ok(Attempt::Conflict),
            Err(CallError::Failed) => Err(StoreError),
        }
    }
}

enum CallError {
    /// 競合（HTTP 409 / ABORTED）。読み直して再試行する。
    Conflict,
    Failed,
}

/// `batchGet` の応答（1文書）から状態を読む。文書が無ければ初期状態。
fn parse_batch_get(value: &Value) -> Option<GateState> {
    let first = value
        .as_array()?
        .iter()
        .find(|r| r.get("found").is_some() || r.get("missing").is_some())?;
    if first.get("missing").is_some() {
        return Some(GateState::default());
    }
    let encoded = first
        .get("found")?
        .get("fields")?
        .get("state")?
        .get("stringValue")?
        .as_str()?;
    serde_json::from_str(encoded).ok()
}

impl GateStore for FirestoreGateStore {
    fn transact<'a>(
        &'a self,
        key: &'a str,
        op: GateOp<'a>,
        clock: &'a dyn Clock,
    ) -> BoxFuture<'a, Result<Decision, StoreError>> {
        Box::pin(async move {
            for _ in 0..MAX_ATTEMPTS {
                match self.attempt(key, op, clock).await? {
                    Attempt::Done(decision) => return Ok(decision),
                    Attempt::Conflict => continue,
                }
            }
            Err(StoreError)
        })
    }

    fn start_reserve(&self) -> Duration {
        self.start_reserve
    }
}
