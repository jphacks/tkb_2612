//! S7・S13：上流ゲート。同じテスト群を、ローカル保存（`UPSTREAM_GATE=local`）と、
//! ループバックの偽Firestoreに対するFirestore保存の両方で実行する。
//!
//! 時刻の判定は手動の時計（`ManualClock`）で進め、待ちの解除は短い期限・取消しで確かめる。

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use accident_route_backend::contract::{CancelToken, Interrupt};
use accident_route_backend::providers::gate::{
    Gate, GateError, GatePolicy, GateStore, LocalGateStore, Quota, QuotaPeriod,
};
use accident_route_backend::providers::gate_firestore::{FirestoreGateStore, StaticToken};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use common::{LiveServer, ManualClock, TokioClock};

const SHORT: Duration = Duration::from_millis(150);

fn policy() -> GatePolicy {
    GatePolicy {
        min_interval: Duration::from_millis(1_050),
        max_concurrent: 1,
        queue_capacity: 16,
        lease_ttl: Duration::from_secs(10),
        quotas: vec![Quota {
            period: QuotaPeriod::Day,
            limit: 2_000,
        }],
    }
}

// ---- 偽Firestore ----------------------------------------------------------------

#[derive(Default)]
struct FakeFirestore {
    /// 文書名 → (版, stateの文字列)
    docs: Mutex<HashMap<String, (u64, String)>>,
    /// トランザクション → 読んだ文書の版（未読はNone）
    transactions: Mutex<HashMap<String, Option<(String, u64)>>>,
    next_transaction: AtomicUsize,
    /// 0より大きい間、commitを409にする
    conflicts: AtomicUsize,
    /// trueなら全呼出しを500にする
    failing: Mutex<bool>,
    commits: AtomicUsize,
}

impl FakeFirestore {
    fn stored_states(&self) -> Vec<Value> {
        self.docs
            .lock()
            .unwrap()
            .values()
            .map(|(_, s)| serde_json::from_str(s).unwrap())
            .collect()
    }
}

async fn firestore_handler(
    State(fake): State<Arc<FakeFirestore>>,
    uri: Uri,
    headers: HeaderMap,
    body: bytes::Bytes,
) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(headers["authorization"], "Bearer test-token");
    if *fake.failing.lock().unwrap() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let path = uri.path();
    let prefix = "/v1/projects/test-project/databases/(default)/documents:";
    let method = path
        .strip_prefix(prefix)
        .unwrap_or_else(|| panic!("想定外のパス: {path}"));
    let transaction = |body: &Value| body["transaction"].as_str().unwrap().to_owned();
    match method {
        "beginTransaction" => {
            let id = format!("t{}", fake.next_transaction.fetch_add(1, Ordering::SeqCst));
            fake.transactions.lock().unwrap().insert(id.clone(), None);
            json_reply(json!({"transaction": id}))
        }
        "batchGet" => {
            let name = body["documents"][0].as_str().unwrap().to_owned();
            assert!(
                name.ends_with("/documents/upstream_gates/osrm"),
                "文書名: {name}"
            );
            let docs = fake.docs.lock().unwrap();
            let (version, found) = match docs.get(&name) {
                Some((v, s)) => (
                    *v,
                    json!({"found": {"name": name, "fields": {"state": {"stringValue": s}}}}),
                ),
                None => (0, json!({"missing": name})),
            };
            fake.transactions
                .lock()
                .unwrap()
                .insert(transaction(&body), Some((name, version)));
            json_reply(json!([found]))
        }
        "commit" => {
            if fake
                .conflicts
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return StatusCode::CONFLICT.into_response();
            }
            let read = fake
                .transactions
                .lock()
                .unwrap()
                .remove(&transaction(&body))
                .flatten()
                .expect("読まずにcommit");
            let write = &body["writes"][0]["update"];
            let name = write["name"].as_str().unwrap().to_owned();
            assert_eq!(name, read.0);
            let mut docs = fake.docs.lock().unwrap();
            let current = docs.get(&name).map_or(0, |(v, _)| *v);
            if current != read.1 {
                return StatusCode::CONFLICT.into_response();
            }
            let state = write["fields"]["state"]["stringValue"]
                .as_str()
                .unwrap()
                .to_owned();
            docs.insert(name, (current + 1, state));
            fake.commits.fetch_add(1, Ordering::SeqCst);
            json_reply(json!({}))
        }
        "rollback" => {
            fake.transactions
                .lock()
                .unwrap()
                .remove(&transaction(&body));
            json_reply(json!({}))
        }
        other => panic!("想定外のメソッド: {other}"),
    }
}

fn json_reply(value: Value) -> Response {
    ([("content-type", "application/json")], value.to_string()).into_response()
}

struct FirestoreFixture {
    fake: Arc<FakeFirestore>,
    server: LiveServer,
    store: Arc<FirestoreGateStore>,
}

async fn firestore() -> FirestoreFixture {
    let fake = Arc::new(FakeFirestore::default());
    let app = axum::Router::new()
        .fallback(firestore_handler)
        .with_state(fake.clone());
    let server = LiveServer::start(app).await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let store = Arc::new(FirestoreGateStore::new(
        client,
        &server.url("/v1/"),
        "test-project",
        "(default)",
        Arc::new(StaticToken("test-token".to_owned())),
    ));
    FirestoreFixture {
        fake,
        server,
        store,
    }
}

// ---- 共通のテスト群 ---------------------------------------------------------------

async fn acquire_short(gate: &Gate) -> Result<(), GateError> {
    gate.acquire(&CancelToken::with_timeout(SHORT))
        .await
        .map(drop)
}

/// 1,050ms間隔：直後と1,049ms後は送信できず、1,050ms後に送信できる。
async fn check_interval(store: Arc<dyn GateStore>) {
    let clock = ManualClock::new(common::fixed_now());
    let gate = Gate::new("osrm", policy(), store, clock.clone());
    let lease = gate.acquire(&CancelToken::unbounded()).await.unwrap();
    lease.release().await;
    assert_eq!(
        acquire_short(&gate).await,
        Err(GateError::Interrupted(Interrupt::DeadlineExceeded))
    );
    clock.advance(Duration::from_millis(1_049));
    assert_eq!(
        acquire_short(&gate).await,
        Err(GateError::Interrupted(Interrupt::DeadlineExceeded))
    );
    clock.advance(Duration::from_millis(1));
    let lease = gate
        .acquire(&CancelToken::with_timeout(SHORT))
        .await
        .unwrap();
    lease.release().await;
}

/// 同時1：リースを持っている間は次が送信できず、解放で送信できる。
async fn check_concurrency(store: Arc<dyn GateStore>) {
    let clock = ManualClock::new(common::fixed_now());
    let gate = Gate::new("osrm", policy(), store, clock.clone());
    let held = gate.acquire(&CancelToken::unbounded()).await.unwrap();
    clock.advance(Duration::from_secs(5));
    assert_eq!(
        acquire_short(&gate).await,
        Err(GateError::Interrupted(Interrupt::DeadlineExceeded))
    );
    // 待っている間に解放されたら、すぐ送信できる
    let waiter = {
        let gate = gate.clone();
        tokio::spawn(async move {
            gate.acquire(&CancelToken::with_timeout(Duration::from_secs(5)))
                .await
                .map(|l| l.record())
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    let started = Instant::now();
    held.release().await;
    let record = waiter.await.unwrap().unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(record.generation >= 1);
}

/// 待ち行列16：送信待ちが16個あると17個目は待たずにBusy。
async fn check_queue_capacity(store: Arc<dyn GateStore>) {
    let clock = ManualClock::new(common::fixed_now());
    let gate = Gate::new("osrm", policy(), store, clock.clone());
    let _held = gate.acquire(&CancelToken::unbounded()).await.unwrap();
    let cancel = CancelToken::unbounded();
    let waiters: Vec<_> = (0..16)
        .map(|_| {
            let gate = gate.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move { gate.acquire(&cancel).await.map(|_| ()) })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = Instant::now();
    assert_eq!(
        gate.acquire(&CancelToken::with_timeout(Duration::from_secs(5)))
            .await
            .map(|_| ()),
        Err(GateError::Busy)
    );
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "待たずに失敗する"
    );
    cancel.cancel();
    for waiter in waiters {
        assert_eq!(
            waiter.await.unwrap(),
            Err(GateError::Interrupted(Interrupt::Cancelled))
        );
    }
}

/// 1日の上限：上限に達したら送信せずBusy、翌日（UTC）に戻る。
async fn check_quota(store: Arc<dyn GateStore>) {
    let clock = ManualClock::new(common::fixed_now());
    let mut policy = policy();
    policy.quotas = vec![Quota {
        period: QuotaPeriod::Day,
        limit: 2,
    }];
    let gate = Gate::new("osrm", policy, store, clock.clone());
    for _ in 0..2 {
        gate.acquire(&CancelToken::unbounded())
            .await
            .unwrap()
            .release()
            .await;
        clock.advance(Duration::from_secs(2));
    }
    assert_eq!(acquire_short(&gate).await, Err(GateError::Busy));
    // 2026-10-24T00:00:00Z起点。翌日0時（UTC）に戻る
    clock.set("2026-10-25T00:00:00Z".parse().unwrap());
    gate.acquire(&CancelToken::with_timeout(SHORT))
        .await
        .unwrap()
        .release()
        .await;
}

/// 失効したリースは送信せず、古い処理の終了は後から発行されたリースを解放しない。
async fn check_lease_expiry(store: Arc<dyn GateStore>) {
    let clock = ManualClock::new(common::fixed_now());
    let mut policy = policy();
    policy.lease_ttl = Duration::from_secs(10);
    let gate = Gate::new("osrm", policy, store, clock.clone());
    let old = gate.acquire(&CancelToken::unbounded()).await.unwrap();
    assert_eq!(old.ensure_valid(), Ok(()));
    clock.advance(Duration::from_secs(10));
    assert_eq!(
        old.ensure_valid(),
        Err(GateError::Busy),
        "失効後は送信しない"
    );
    // 失効したリースの枠は新しい処理に渡る
    let new = gate
        .acquire(&CancelToken::with_timeout(SHORT))
        .await
        .unwrap();
    assert_ne!(new.record().generation, old.record().generation);
    old.release().await;
    clock.advance(Duration::from_secs(2));
    assert_eq!(
        acquire_short(&gate).await,
        Err(GateError::Interrupted(Interrupt::DeadlineExceeded)),
        "古い世代の解放で新しいリースが外れない"
    );
    new.release().await;
    gate.acquire(&CancelToken::with_timeout(SHORT))
        .await
        .unwrap()
        .release()
        .await;
}

/// 取消しで待ちが即時に解除される（期限まで待たない）。
async fn check_cancellation(store: Arc<dyn GateStore>) {
    let clock = ManualClock::new(common::fixed_now());
    let gate = Gate::new("osrm", policy(), store, clock.clone());
    let _held = gate.acquire(&CancelToken::unbounded()).await.unwrap();
    let cancel = CancelToken::with_timeout(Duration::from_secs(30));
    let waiter = {
        let gate = gate.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move { gate.acquire(&cancel).await.map(|_| ()) })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    let started = Instant::now();
    cancel.cancel();
    assert_eq!(
        waiter.await.unwrap(),
        Err(GateError::Interrupted(Interrupt::Cancelled))
    );
    assert!(started.elapsed() < Duration::from_millis(100));
    // 取消し済みのトークンでは共有状態を読まずに失敗する
    assert_eq!(
        gate.acquire(&cancel).await.map(|_| ()),
        Err(GateError::Interrupted(Interrupt::Cancelled))
    );
}

macro_rules! for_both_stores {
    ($($name:ident => $check:ident),* $(,)?) => {
        mod local {
            use super::*;
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
                async fn $name() {
                    $check(Arc::new(LocalGateStore::new())).await;
                }
            )*
        }
        mod firestore {
            use super::*;
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
                async fn $name() {
                    let fixture = super::firestore().await;
                    $check(fixture.store.clone()).await;
                    for state in fixture.fake.stored_states() {
                        super::assert_state_has_only_counters(&state);
                    }
                    fixture.server.stop().await;
                }
            )*
        }
    };
}

for_both_stores! {
    interval_is_1050ms => check_interval,
    concurrency_is_one => check_concurrency,
    queue_is_bounded_at_16 => check_queue_capacity,
    quota_is_enforced_per_utc_day => check_quota,
    expired_lease_does_not_send => check_lease_expiry,
    cancellation_releases_the_wait => check_cancellation,
}

/// 共有状態は開始時刻・リースの期限と世代・件数だけを持つ（住所・座標・ID等を含まない）。
fn assert_state_has_only_counters(state: &Value) {
    let keys: Vec<_> = state.as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        ["counters", "last_start_ms", "leases", "next_generation"]
    );
    for lease in state["leases"].as_array().unwrap() {
        let mut keys: Vec<_> = lease.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["expires_at_ms", "generation"]);
    }
    for counter in state["counters"].as_array().unwrap() {
        let mut keys: Vec<_> = counter.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["count", "period", "period_start_ms"]);
    }
}

// ---- 保存方式に固有の検査 ---------------------------------------------------------

/// tokioの時間制御で、連続した送信の開始が1,050ms間隔になる（ローカル保存）。
#[tokio::test(start_paused = true)]
async fn consecutive_sends_are_spaced_by_1050ms_under_paused_time() {
    let clock = TokioClock::new();
    let gate = Gate::new("osrm", policy(), Arc::new(LocalGateStore::new()), clock);
    let start = tokio::time::Instant::now();
    let mut starts = Vec::new();
    for _ in 0..4 {
        let lease = gate.acquire(&CancelToken::unbounded()).await.unwrap();
        starts.push(start.elapsed());
        lease.release().await;
    }
    for pair in starts.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            gap >= Duration::from_millis(1_050) && gap < Duration::from_millis(1_150),
            "間隔 {gap:?}"
        );
    }
}

/// 提供元ごとに独立して制御する。
#[tokio::test]
async fn providers_are_controlled_independently() {
    let clock = ManualClock::new(common::fixed_now());
    let store: Arc<dyn GateStore> = Arc::new(LocalGateStore::new());
    let osrm = Gate::new("osrm", policy(), store.clone(), clock.clone());
    let geoapify = Gate::new("geoapify", policy(), store, clock.clone());
    let _a = osrm.acquire(&CancelToken::unbounded()).await.unwrap();
    let _b = geoapify
        .acquire(&CancelToken::with_timeout(SHORT))
        .await
        .unwrap();
}

/// 共有状態が読めないときは送信せずBusy。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unreadable_shared_state_is_busy() {
    let fixture = firestore().await;
    *fixture.fake.failing.lock().unwrap() = true;
    let gate = Gate::new(
        "osrm",
        policy(),
        fixture.store.clone(),
        ManualClock::new(common::fixed_now()),
    );
    assert_eq!(acquire_short(&gate).await, Err(GateError::Busy));
    fixture.server.stop().await;
}

/// 競合は読み直して解消し、解消しない（3回続く）ときは送信せずBusy。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conflicts_are_retried_and_unresolved_conflicts_are_busy() {
    let fixture = firestore().await;
    let gate = Gate::new(
        "osrm",
        policy(),
        fixture.store.clone(),
        ManualClock::new(common::fixed_now()),
    );
    fixture.fake.conflicts.store(2, Ordering::SeqCst);
    let lease = gate
        .acquire(&CancelToken::with_timeout(SHORT))
        .await
        .unwrap();
    assert_eq!(fixture.fake.conflicts.load(Ordering::SeqCst), 0);
    lease.release().await;

    let other = Gate::new(
        "osrm",
        policy(),
        fixture.store.clone(),
        ManualClock::new(common::fixed_now() + chrono::Duration::seconds(5)),
    );
    fixture.fake.conflicts.store(3, Ordering::SeqCst);
    assert_eq!(acquire_short(&other).await, Err(GateError::Busy));
    fixture.server.stop().await;
}

/// 別インスタンス（別のゲート・同じ共有状態）のリースも同時数に数える。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn leases_are_shared_across_instances() {
    let fixture = firestore().await;
    let clock = ManualClock::new(common::fixed_now());
    let a = Gate::new("osrm", policy(), fixture.store.clone(), clock.clone());
    let b = Gate::new("osrm", policy(), fixture.store.clone(), clock.clone());
    let held = a.acquire(&CancelToken::unbounded()).await.unwrap();
    clock.advance(Duration::from_secs(2));
    assert_eq!(
        acquire_short(&b).await,
        Err(GateError::Interrupted(Interrupt::DeadlineExceeded))
    );
    held.release().await;
    b.acquire(&CancelToken::with_timeout(Duration::from_secs(1)))
        .await
        .unwrap()
        .release()
        .await;
    assert!(fixture.fake.commits.load(Ordering::SeqCst) >= 4);
    fixture.server.stop().await;
}
