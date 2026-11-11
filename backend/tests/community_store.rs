//! 口コミの保存方式。同じテスト群を、ローカル保存と、ループバックの偽Firestoreに対する
//! Firestore保存（`community_reports` コレクション）の両方で実行する。

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use accident_route_backend::providers::community::{
    Ballot, Bounds, CommunityError, CommunityStore, LocalCommunityStore, SafetyKind, StoredReport,
    VoterId, new_report,
};
use accident_route_backend::providers::community_firestore::{
    FirestoreCommunityStore, MAX_PAGES, PAGE_SIZE,
};
use accident_route_backend::providers::gate_firestore::StaticToken;
use axum::extract::State;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use common::LiveServer;

const NOW: i64 = 1_793_000_000_000;
const PREFIX: &str = "/v1/projects/test-project/databases/(default)/documents";

// ---- 偽Firestore ----------------------------------------------------------------

#[derive(Default)]
struct FakeFirestore {
    /// 文書名 → (版, フィールド)
    docs: Mutex<HashMap<String, (u64, Value)>>,
    /// トランザクション → 読んだ文書の版（未読はNone）
    transactions: Mutex<HashMap<String, Option<(String, u64)>>>,
    next_transaction: AtomicUsize,
    /// 0より大きい間、トランザクションのcommitを409にする
    conflicts: AtomicUsize,
    failing: Mutex<bool>,
    queries: Mutex<Vec<Value>>,
}

fn take_one(counter: &AtomicUsize) -> bool {
    let mut n = counter.load(Ordering::SeqCst);
    while n > 0 {
        match counter.compare_exchange_weak(n, n - 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(current) => n = current,
        }
    }
    false
}

fn json_reply(value: Value) -> Response {
    ([("content-type", "application/json")], value.to_string()).into_response()
}

async fn firestore_handler(
    State(fake): State<Arc<FakeFirestore>>,
    uri: Uri,
    headers: axum::http::HeaderMap,
    body: bytes::Bytes,
) -> Response {
    assert_eq!(headers["authorization"], "Bearer test-token");
    if *fake.failing.lock().unwrap() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let body: Value = serde_json::from_slice(&body).unwrap();
    let method = uri
        .path()
        .strip_prefix(&format!("{PREFIX}:"))
        .unwrap_or_else(|| panic!("想定外のパス: {}", uri.path()))
        .to_owned();
    let transaction = |body: &Value| body["transaction"].as_str().map(str::to_owned);
    match method.as_str() {
        "beginTransaction" => {
            let id = format!("t{}", fake.next_transaction.fetch_add(1, Ordering::SeqCst));
            fake.transactions.lock().unwrap().insert(id.clone(), None);
            json_reply(json!({"transaction": id}))
        }
        "batchGet" => {
            let name = body["documents"][0].as_str().unwrap().to_owned();
            let docs = fake.docs.lock().unwrap();
            let (version, result) = match docs.get(&name) {
                Some((v, fields)) => (*v, json!({"found": {"name": name, "fields": fields}})),
                None => (0, json!({"missing": name})),
            };
            fake.transactions
                .lock()
                .unwrap()
                .insert(transaction(&body).unwrap(), Some((name, version)));
            json_reply(json!([result]))
        }
        "commit" => {
            let write = &body["writes"][0];
            let name = write["update"]["name"].as_str().unwrap().to_owned();
            let fields = write["update"]["fields"].clone();
            let mut docs = fake.docs.lock().unwrap();
            let current = docs.get(&name).map(|(v, _)| *v);
            match transaction(&body) {
                Some(id) => {
                    if take_one(&fake.conflicts) {
                        return StatusCode::CONFLICT.into_response();
                    }
                    let read = fake
                        .transactions
                        .lock()
                        .unwrap()
                        .remove(&id)
                        .flatten()
                        .expect("読まずにcommit");
                    assert_eq!(read.0, name);
                    if current.unwrap_or(0) != read.1 {
                        return StatusCode::CONFLICT.into_response();
                    }
                }
                None => {
                    assert_eq!(write["currentDocument"], json!({"exists": false}));
                    if current.is_some() {
                        return StatusCode::CONFLICT.into_response();
                    }
                }
            }
            docs.insert(name, (current.unwrap_or(0) + 1, fields));
            json_reply(json!({}))
        }
        "rollback" => {
            fake.transactions
                .lock()
                .unwrap()
                .remove(&transaction(&body).unwrap());
            json_reply(json!({}))
        }
        "runQuery" => {
            fake.queries.lock().unwrap().push(body.clone());
            let query = &body["structuredQuery"];
            assert_eq!(query["from"][0]["collectionId"], "community_reports");
            let filters = query["where"]["compositeFilter"]["filters"]
                .as_array()
                .unwrap();
            let bound = |op: &str| {
                filters
                    .iter()
                    .find(|f| f["fieldFilter"]["op"] == op)
                    .unwrap()["fieldFilter"]["value"]["doubleValue"]
                    .as_f64()
                    .unwrap()
            };
            let (min, max) = (bound("GREATER_THAN_OR_EQUAL"), bound("LESS_THAN_OR_EQUAL"));
            // 緯度・文書名の順（単一フィールド索引で読める並び）だけを受け付ける
            assert_eq!(
                query["orderBy"],
                json!([
                    {"field": {"fieldPath": "lat"}, "direction": "ASCENDING"},
                    {"field": {"fieldPath": "__name__"}, "direction": "ASCENDING"},
                ])
            );
            let after = query.get("startAt").map(|start| {
                assert_eq!(start["before"], false);
                (
                    start["values"][0]["doubleValue"].as_f64().unwrap(),
                    start["values"][1]["referenceValue"]
                        .as_str()
                        .unwrap()
                        .to_owned(),
                )
            });
            let limit = query["limit"].as_u64().unwrap() as usize;
            let docs = fake.docs.lock().unwrap();
            let mut matched: Vec<(f64, &String, &Value)> = docs
                .iter()
                .map(|(name, (_, f))| (f["lat"]["doubleValue"].as_f64().unwrap(), name, f))
                .filter(|(lat, _, _)| (min..=max).contains(lat))
                .collect();
            matched.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(b.1)));
            let mut results: Vec<Value> = matched
                .into_iter()
                .filter(|(lat, name, _)| {
                    after.as_ref().is_none_or(|(after_lat, after_name)| {
                        lat.total_cmp(after_lat)
                            .then_with(|| (*name).cmp(after_name))
                            == std::cmp::Ordering::Greater
                    })
                })
                .take(limit)
                .map(|(_, name, f)| json!({"document": {"name": name, "fields": f}}))
                .collect();
            results.push(json!({"readTime": "2026-01-01T00:00:00Z"}));
            json_reply(Value::Array(results))
        }
        other => panic!("想定外のメソッド: {other}"),
    }
}

struct Fixture {
    fake: Arc<FakeFirestore>,
    _server: LiveServer,
    store: Arc<FirestoreCommunityStore>,
}

async fn firestore() -> Fixture {
    let fake = Arc::new(FakeFirestore::default());
    let app = axum::Router::new()
        .fallback(firestore_handler)
        .with_state(fake.clone());
    let server = LiveServer::start(app).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let store = Arc::new(FirestoreCommunityStore::new(
        client,
        &server.url("/v1/"),
        "test-project",
        "(default)",
        Arc::new(StaticToken("test-token".to_owned())),
    ));
    Fixture {
        fake,
        _server: server,
        store,
    }
}

// ---- 共通のテスト群 ---------------------------------------------------------------

fn voter(name: &str) -> VoterId {
    VoterId::from_client(&format!("{name}-0123456789abcdef")).unwrap()
}

fn ballot(kind: SafetyKind, text: Option<&str>) -> Ballot {
    Ballot {
        kind,
        text: text.map(str::to_owned),
    }
}

const TOKYO: Bounds = Bounds {
    min_lat: 35.6,
    min_lon: 139.7,
    max_lat: 35.7,
    max_lon: 139.8,
};

async fn check_round_trip(store: Arc<dyn CommunityStore>) {
    let inside = new_report(
        35.65,
        139.75,
        &voter("a"),
        ballot(SafetyKind::Danger, Some("暗い")),
        NOW,
    );
    // 緯度は範囲内でも経度が外
    let east = new_report(
        35.65,
        140.5,
        &voter("a"),
        ballot(SafetyKind::Safe, None),
        NOW + 1,
    );
    let north = new_report(
        36.5,
        139.75,
        &voter("a"),
        ballot(SafetyKind::Safe, None),
        NOW + 2,
    );
    for report in [&inside, &east, &north] {
        store.create(report.clone()).await.unwrap();
    }
    assert_eq!(store.list(TOKYO).await.unwrap(), vec![inside.clone()]);

    let voted = store
        .vote(
            &inside.id,
            &voter("b"),
            ballot(SafetyKind::Safe, Some("昼は明るい")),
            NOW + 10,
        )
        .await
        .unwrap();
    assert_eq!(voted.count(SafetyKind::Safe), 1);
    assert_eq!(voted.count(SafetyKind::Danger), 1);
    assert_eq!(voted.comments.len(), 2);
    assert_eq!(voted.vote_of(&voter("b")), Some(SafetyKind::Safe));
    assert_eq!(store.list(TOKYO).await.unwrap(), vec![voted]);

    assert_eq!(
        store
            .vote(
                &inside.id,
                &voter("b"),
                ballot(SafetyKind::Danger, None),
                NOW + 20
            )
            .await,
        Err(CommunityError::AlreadyVoted)
    );
    assert_eq!(
        store
            .vote(
                &"0".repeat(32),
                &voter("b"),
                ballot(SafetyKind::Danger, None),
                NOW
            )
            .await,
        Err(CommunityError::NotFound)
    );
}

#[tokio::test]
async fn local_round_trip() {
    check_round_trip(Arc::new(LocalCommunityStore::new())).await;
}

#[tokio::test]
async fn firestore_round_trip() {
    let fixture = firestore().await;
    check_round_trip(fixture.store.clone()).await;
    // 経度は複合索引を要らなくするためサーバ側で絞らない（緯度の帯だけで問い合わせる）
    let queries = fixture.fake.queries.lock().unwrap();
    assert!(
        queries
            .iter()
            .all(|q| q["structuredQuery"]["limit"] == PAGE_SIZE)
    );
    // 保存する投票者はハッシュだけ
    let docs = fixture.fake.docs.lock().unwrap();
    let all = serde_json::to_string(&*docs.values().map(|(_, f)| f).collect::<Vec<_>>()).unwrap();
    assert!(!all.contains("0123456789abcdef"));
}

#[tokio::test]
async fn firestore_retries_conflicts() {
    let fixture = firestore().await;
    let report = new_report(
        35.65,
        139.75,
        &voter("a"),
        ballot(SafetyKind::Safe, None),
        NOW,
    );
    fixture.store.create(report.clone()).await.unwrap();
    fixture.fake.conflicts.store(2, Ordering::SeqCst);
    let voted = fixture
        .store
        .vote(
            &report.id,
            &voter("b"),
            ballot(SafetyKind::Danger, None),
            NOW,
        )
        .await
        .unwrap();
    assert_eq!(voted.votes.len(), 2);

    // 競合が上限の回数続いたら失敗（票は増えない）
    fixture.fake.conflicts.store(10, Ordering::SeqCst);
    assert_eq!(
        fixture
            .store
            .vote(
                &report.id,
                &voter("c"),
                ballot(SafetyKind::Danger, None),
                NOW
            )
            .await,
        Err(CommunityError::Unavailable)
    );
    fixture.fake.conflicts.store(0, Ordering::SeqCst);
    assert_eq!(fixture.store.list(TOKYO).await.unwrap()[0].votes.len(), 2);
}

#[tokio::test]
async fn firestore_failures_are_unavailable() {
    let fixture = firestore().await;
    *fixture.fake.failing.lock().unwrap() = true;
    let report = new_report(
        35.65,
        139.75,
        &voter("a"),
        ballot(SafetyKind::Safe, None),
        NOW,
    );
    assert_eq!(
        fixture.store.create(report.clone()).await,
        Err(CommunityError::Unavailable)
    );
    assert_eq!(
        fixture.store.list(TOKYO).await,
        Err(CommunityError::Unavailable)
    );
    assert_eq!(
        fixture
            .store
            .vote(&report.id, &voter("b"), ballot(SafetyKind::Safe, None), NOW)
            .await,
        Err(CommunityError::Unavailable)
    );
}

/// 偽Firestoreへ地点を直接置く（大量の文書を用意するため。`create` と同じフィールド）。
fn put(fake: &FakeFirestore, report: &StoredReport) {
    fake.docs.lock().unwrap().insert(
        format!("{PREFIX_NAME}/community_reports/{}", report.id),
        (
            1,
            json!({
                "data": {"stringValue": serde_json::to_string(report).unwrap()},
                "lat": {"doubleValue": report.lat},
                "lon": {"doubleValue": report.lon},
            }),
        ),
    );
}

const PREFIX_NAME: &str = "projects/test-project/databases/(default)/documents";

#[tokio::test]
async fn firestore_reads_every_page_of_the_latitude_band() {
    let fixture = firestore().await;
    // 緯度は範囲内・経度は範囲外の新しい地点で、1ページ目と2ページ目を埋める
    for i in 0..2 * PAGE_SIZE {
        let east = new_report(
            35.61,
            140.5,
            &voter("a"),
            ballot(SafetyKind::Safe, None),
            NOW + 100 + i as i64,
        );
        put(&fixture.fake, &east);
    }
    // 範囲内の地点は緯度の並びで最後（3ページ目）にある
    let older = new_report(
        35.69,
        139.75,
        &voter("a"),
        ballot(SafetyKind::Danger, None),
        NOW,
    );
    let newer = new_report(
        35.69,
        139.76,
        &voter("a"),
        ballot(SafetyKind::Safe, None),
        NOW + 1,
    );
    put(&fixture.fake, &older);
    put(&fixture.fake, &newer);
    assert_eq!(fixture.store.list(TOKYO).await.unwrap(), vec![newer, older]);
    let queries = fixture.fake.queries.lock().unwrap();
    assert_eq!(queries.len(), 3);
    assert!(queries[0]["structuredQuery"].get("startAt").is_none());
    assert!(queries[1]["structuredQuery"].get("startAt").is_some());
}

#[tokio::test]
async fn firestore_fails_instead_of_returning_a_partial_list() {
    let fixture = firestore().await;
    for i in 0..MAX_PAGES * PAGE_SIZE {
        let east = new_report(
            35.61,
            140.5,
            &voter("a"),
            ballot(SafetyKind::Safe, None),
            NOW + i as i64,
        );
        put(&fixture.fake, &east);
    }
    assert_eq!(
        fixture.store.list(TOKYO).await,
        Err(CommunityError::Unavailable)
    );
    assert_eq!(fixture.fake.queries.lock().unwrap().len(), MAX_PAGES);
}

#[tokio::test]
async fn firestore_skips_broken_documents() {
    let fixture = firestore().await;
    let report = new_report(
        35.65,
        139.75,
        &voter("a"),
        ballot(SafetyKind::Safe, None),
        NOW,
    );
    fixture.store.create(report.clone()).await.unwrap();
    fixture.fake.docs.lock().unwrap().insert(
        format!(
            "projects/test-project/databases/(default)/documents/community_reports/{}",
            "f".repeat(32)
        ),
        (
            1,
            json!({"data": {"stringValue": "{"}, "lat": {"doubleValue": 35.65}}),
        ),
    );
    assert_eq!(fixture.store.list(TOKYO).await.unwrap(), vec![report]);
}
