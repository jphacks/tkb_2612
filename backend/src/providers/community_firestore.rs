//! Firestoreに保存する口コミ（`COMMUNITY_STORE=firestore`）。
//!
//! 1地点を1文書（`community_reports/{id}`）にし、文字列フィールド `data` に
//! [`StoredReport`] のJSONを置く。範囲の検索用に `lat`・`lon` も数値で持つ。
//! 投票は上流ゲートと同じく読取り・判定・書込みを1トランザクションで行い、
//! 競合（409）なら読み直す。認証・接続先は上流ゲート（`gate_firestore`）と共有する。

use std::sync::Arc;

use serde_json::{Value, json};

use crate::contract::BoxFuture;
use crate::providers::community::{
    Ballot, Bounds, CommunityError, CommunityStore, StoredReport, VoterId, apply_vote,
    select_listed,
};
use crate::providers::gate_firestore::{
    API_ROOT, CallError, DATABASE, MAX_ATTEMPTS, MetadataToken, TokenSource, call_documents,
};

/// 地点を置くコレクション。
pub const COLLECTION: &str = "community_reports";
/// 範囲の検索で1回に読む文書の数（緯度の帯をこの件数ずつ読み、経度は読んだ後に絞る）。
pub const PAGE_SIZE: usize = 1_000;
/// 1回の一覧で読むページの上限。超えたら途中までの結果を返さず失敗にする
/// （範囲外の地点で上限が埋まり、範囲内の地点を取りこぼしたまま返さないため）。
pub const MAX_PAGES: usize = 50;

pub struct FirestoreCommunityStore {
    client: reqwest::Client,
    documents_url: String,
    documents_name: String,
    tokens: Arc<dyn TokenSource>,
}

impl std::fmt::Debug for FirestoreCommunityStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FirestoreCommunityStore")
            .finish_non_exhaustive()
    }
}

/// 1回の投票の試行の結果。
enum Attempt {
    Done(Result<StoredReport, CommunityError>),
    Conflict,
}

impl FirestoreCommunityStore {
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
        }
    }

    async fn call(&self, token: &str, method: &str, body: Value) -> Result<Value, CallError> {
        call_documents(&self.client, &self.documents_url, token, method, body).await
    }

    async fn token(&self) -> Result<String, CommunityError> {
        self.tokens
            .token()
            .await
            .map_err(|_| CommunityError::Unavailable)
    }

    fn name(&self, id: &str) -> String {
        format!("{}/{COLLECTION}/{id}", self.documents_name)
    }

    async fn attempt_vote(
        &self,
        id: &str,
        voter: &VoterId,
        ballot: &Ballot,
        now_ms: i64,
    ) -> Result<Attempt, CommunityError> {
        let token = self.token().await?;
        let begun = self
            .call(
                &token,
                "beginTransaction",
                json!({"options": {"readWrite": {}}}),
            )
            .await
            .map_err(|_| CommunityError::Unavailable)?;
        let transaction = begun
            .get("transaction")
            .and_then(Value::as_str)
            .ok_or(CommunityError::Unavailable)?
            .to_owned();
        let name = self.name(id);
        let result = self
            .read_vote_commit(&token, &transaction, &name, voter, ballot, now_ms)
            .await;
        if !matches!(result, Ok(Attempt::Done(Ok(_)))) {
            // 書かなかった・失敗したトランザクションは捨てる（失敗しても期限で消える）
            let _ = self
                .call(&token, "rollback", json!({"transaction": transaction}))
                .await;
        }
        result
    }

    async fn read_vote_commit(
        &self,
        token: &str,
        transaction: &str,
        name: &str,
        voter: &VoterId,
        ballot: &Ballot,
        now_ms: i64,
    ) -> Result<Attempt, CommunityError> {
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
            Err(CallError::Failed) => return Err(CommunityError::Unavailable),
        };
        let Some(mut report) = parse_batch_get(&read)? else {
            return Ok(Attempt::Done(Err(CommunityError::NotFound)));
        };
        if let Err(error) = apply_vote(&mut report, voter, ballot.clone(), now_ms) {
            return Ok(Attempt::Done(Err(error)));
        }
        let commit = json!({
            "writes": [{"update": {"name": name, "fields": fields(&report)?}}],
            "transaction": transaction,
        });
        match self.call(token, "commit", commit).await {
            Ok(_) => Ok(Attempt::Done(Ok(report))),
            Err(CallError::Conflict) => Ok(Attempt::Conflict),
            Err(CallError::Failed) => Err(CommunityError::Unavailable),
        }
    }
}

/// 文書のフィールド。
fn fields(report: &StoredReport) -> Result<Value, CommunityError> {
    let encoded = serde_json::to_string(report).map_err(|_| CommunityError::Unavailable)?;
    Ok(json!({
        "data": {"stringValue": encoded},
        "lat": {"doubleValue": report.lat},
        "lon": {"doubleValue": report.lon},
    }))
}

/// 文書（`fields.data`）から地点を読む。
fn parse_document(document: &Value) -> Option<StoredReport> {
    let encoded = document
        .get("fields")?
        .get("data")?
        .get("stringValue")?
        .as_str()?;
    serde_json::from_str(encoded).ok()
}

/// `batchGet` の応答（1文書）。文書が無ければ `None`。
fn parse_batch_get(value: &Value) -> Result<Option<StoredReport>, CommunityError> {
    let first = value
        .as_array()
        .and_then(|results| {
            results
                .iter()
                .find(|r| r.get("found").is_some() || r.get("missing").is_some())
        })
        .ok_or(CommunityError::Unavailable)?;
    match first.get("found") {
        None => Ok(None),
        Some(found) => parse_document(found)
            .map(Some)
            .ok_or(CommunityError::Unavailable),
    }
}

/// 前のページの最後の文書（次のページはこの直後から読む）。
struct Cursor {
    lat: f64,
    name: String,
}

/// 緯度の範囲で絞る `runQuery` の本文（経度は読んだ後に絞る。複合索引を要らなくするため）。
/// 緯度・文書名の順に並べ、`after` があればその直後から [`PAGE_SIZE`] 件を読む。
fn range_query(bounds: Bounds, after: Option<&Cursor>) -> Value {
    let filter = |op: &str, value: f64| {
        json!({"fieldFilter": {
            "field": {"fieldPath": "lat"},
            "op": op,
            "value": {"doubleValue": value},
        }})
    };
    let mut query = json!({
        "from": [{"collectionId": COLLECTION}],
        "where": {"compositeFilter": {"op": "AND", "filters": [
            filter("GREATER_THAN_OR_EQUAL", bounds.min_lat),
            filter("LESS_THAN_OR_EQUAL", bounds.max_lat),
        ]}},
        "orderBy": [
            {"field": {"fieldPath": "lat"}, "direction": "ASCENDING"},
            {"field": {"fieldPath": "__name__"}, "direction": "ASCENDING"},
        ],
        "limit": PAGE_SIZE,
    });
    if let Some(cursor) = after {
        query["startAt"] = json!({
            "values": [{"doubleValue": cursor.lat}, {"referenceValue": cursor.name}],
            "before": false,
        });
    }
    json!({"structuredQuery": query})
}

/// ページの最後の文書を次のページの起点にする。
fn cursor_of(document: &Value) -> Option<Cursor> {
    Some(Cursor {
        lat: document
            .get("fields")?
            .get("lat")?
            .get("doubleValue")?
            .as_f64()?,
        name: document.get("name")?.as_str()?.to_owned(),
    })
}

impl CommunityStore for FirestoreCommunityStore {
    fn list(&self, bounds: Bounds) -> BoxFuture<'_, Result<Vec<StoredReport>, CommunityError>> {
        Box::pin(async move {
            let token = self.token().await?;
            let mut listed = Vec::new();
            let mut after: Option<Cursor> = None;
            for _ in 0..MAX_PAGES {
                let results = self
                    .call(&token, "runQuery", range_query(bounds, after.as_ref()))
                    .await
                    .map_err(|_| CommunityError::Unavailable)?;
                let documents: Vec<&Value> = results
                    .as_array()
                    .ok_or(CommunityError::Unavailable)?
                    .iter()
                    .filter_map(|r| r.get("document"))
                    .collect();
                // 壊れた文書は読み飛ばす（1件のために一覧全体を失敗させない）。
                // ページごとに範囲内の新しい順に絞り、全体の上位だけを持ち続ける
                let page = documents.iter().filter_map(|d| parse_document(d));
                listed = select_listed(listed.into_iter().chain(page), bounds);
                if documents.len() < PAGE_SIZE {
                    return Ok(listed);
                }
                let last = documents.last().ok_or(CommunityError::Unavailable)?;
                after = Some(cursor_of(last).ok_or(CommunityError::Unavailable)?);
            }
            Err(CommunityError::Unavailable)
        })
    }

    fn create(&self, report: StoredReport) -> BoxFuture<'_, Result<(), CommunityError>> {
        Box::pin(async move {
            let token = self.token().await?;
            let commit = json!({"writes": [{
                "update": {"name": self.name(&report.id), "fields": fields(&report)?},
                "currentDocument": {"exists": false},
            }]});
            self.call(&token, "commit", commit)
                .await
                .map(drop)
                .map_err(|_| CommunityError::Unavailable)
        })
    }

    fn vote<'a>(
        &'a self,
        id: &'a str,
        voter: &'a VoterId,
        ballot: Ballot,
        now_ms: i64,
    ) -> BoxFuture<'a, Result<StoredReport, CommunityError>> {
        Box::pin(async move {
            for _ in 0..MAX_ATTEMPTS {
                match self.attempt_vote(id, voter, &ballot, now_ms).await? {
                    Attempt::Done(result) => return result,
                    Attempt::Conflict => continue,
                }
            }
            Err(CommunityError::Unavailable)
        })
    }
}
