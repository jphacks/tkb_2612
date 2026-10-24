//! 全インスタンス共通の上流ゲート（backend-spec.md 6章、architecture.md 7章）。
//!
//! 提供元（OSRM・Geoapify・Open-Meteo）ごとに、通信開始間隔・同時通信数・集計期間別の上限を
//! 共有状態で管理し、満たしてから送信させる。有限の待ち行列はプロセスごとに持つ。
//!
//! 判定（[`apply`]）は保存方式から分けた純粋な状態遷移で、保存方式は [`GateStore`] で
//! 差し替える（ローカル開発用の [`LocalGateStore`]、本番のFirestore）。共有状態に持つのは
//! 提供元の識別子（キー）、呼出し開始時刻、リースの期限と世代番号、集計件数だけで、
//! 住所・座標・経路・利用者ID・リクエストIDは持たない。通信はトランザクションの外で行う。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::contract::{BoxFuture, CancelToken, Interrupt};
use crate::providers::clock::Clock;

/// 共有状態の読取失敗・競合の未解消・待ち行列の満杯・上限到達は、呼び出さずに `Busy`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GateError {
    Busy,
    Interrupted(Interrupt),
}

impl From<Interrupt> for GateError {
    fn from(value: Interrupt) -> Self {
        GateError::Interrupted(value)
    }
}

/// 上限を数える期間。境界はUTC。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QuotaPeriod {
    Minute,
    Hour,
    Day,
    Month,
}

const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 60 * MINUTE_MS;
const DAY_MS: i64 = 24 * HOUR_MS;

impl QuotaPeriod {
    /// `now_ms` を含む期間の開始（UNIXミリ秒）。
    pub fn start_ms(self, now_ms: i64) -> i64 {
        match self {
            QuotaPeriod::Minute => now_ms.div_euclid(MINUTE_MS) * MINUTE_MS,
            QuotaPeriod::Hour => now_ms.div_euclid(HOUR_MS) * HOUR_MS,
            QuotaPeriod::Day => now_ms.div_euclid(DAY_MS) * DAY_MS,
            QuotaPeriod::Month => {
                let now = DateTime::<Utc>::from_timestamp_millis(now_ms).unwrap_or_default();
                NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
                    .and_then(|d| d.and_hms_opt(0, 0, 0))
                    .map_or(0, |t| t.and_utc().timestamp_millis())
            }
        }
    }

    /// `now_ms` を含む期間の終わり（次の期間の開始、UNIXミリ秒）。
    pub fn end_ms(self, now_ms: i64) -> i64 {
        let start = self.start_ms(now_ms);
        match self {
            QuotaPeriod::Minute => start.saturating_add(MINUTE_MS),
            QuotaPeriod::Hour => start.saturating_add(HOUR_MS),
            QuotaPeriod::Day => start.saturating_add(DAY_MS),
            QuotaPeriod::Month => {
                let now = DateTime::<Utc>::from_timestamp_millis(start).unwrap_or_default();
                let (year, month) = if now.month() == 12 {
                    (now.year() + 1, 1)
                } else {
                    (now.year(), now.month() + 1)
                };
                NaiveDate::from_ymd_opt(year, month, 1)
                    .and_then(|d| d.and_hms_opt(0, 0, 0))
                    .map_or(i64::MAX, |t| t.and_utc().timestamp_millis())
            }
        }
    }
}

/// 集計期間別の上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quota {
    pub period: QuotaPeriod,
    pub limit: u64,
}

/// 提供元ごとの制御値。
#[derive(Debug, Clone, PartialEq)]
pub struct GatePolicy {
    /// 通信開始の最短間隔。
    pub min_interval: Duration,
    /// 同時通信数。
    pub max_concurrent: u32,
    /// このプロセスで送信を待てる呼出しの数。超えたら `Busy`。
    pub queue_capacity: usize,
    /// リースの有効期間。1通信の期限に余裕を足した値にする。
    pub lease_ttl: Duration,
    pub quotas: Vec<Quota>,
}

/// 同時通信のリース。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRecord {
    pub generation: u64,
    pub expires_at_ms: i64,
}

/// 集計期間の件数。期間が変わったら上書きする。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaCounter {
    pub period: QuotaPeriod,
    pub period_start_ms: i64,
    pub count: u64,
}

/// 提供元1つ分の共有状態。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateState {
    pub last_start_ms: Option<i64>,
    pub next_generation: u64,
    pub leases: Vec<LeaseRecord>,
    pub counters: Vec<QuotaCounter>,
}

/// 共有状態への操作。
#[derive(Debug, Clone, Copy)]
pub enum GateOp<'a> {
    /// `start_within` は、判定から送信開始までに許す遅れ（保存方式の応答待ちの分）。
    /// 開始時刻は判定時刻にこの分を足した値として記録し、送信はそれまでに始める。
    Acquire {
        policy: &'a GatePolicy,
        start_within: Duration,
    },
    Release {
        generation: u64,
    },
}

/// 操作の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// 送信してよい。`start_by_ms` までに送信を始め、リースを持って送信し、終わったら解放する。
    Granted {
        lease: LeaseRecord,
        start_by_ms: i64,
    },
    /// `until_ms` まで待ってから再判定する（同時数の空き待ちはもっと早く空くことがある）。
    Wait {
        until_ms: i64,
    },
    /// 集計期間の上限に達した。
    QuotaExhausted,
    Released,
}

/// 状態遷移（保存方式に依存しない判定）。
///
/// 期限切れのリースを除いてから判定する。解放は世代番号が一致するリースだけを消すので、
/// 期限切れで除かれた古い処理の終了が、後から発行された新しいリースを解放することはない。
pub fn apply(state: &mut GateState, op: GateOp<'_>, now_ms: i64) -> Decision {
    state.leases.retain(|l| l.expires_at_ms > now_ms);
    match op {
        GateOp::Release { generation } => {
            state.leases.retain(|l| l.generation != generation);
            Decision::Released
        }
        GateOp::Acquire {
            policy,
            start_within,
        } => acquire(state, policy, now_ms, duration_ms(start_within)),
    }
}

fn acquire(state: &mut GateState, policy: &GatePolicy, now_ms: i64, within_ms: i64) -> Decision {
    // 期間が終わった件数は上書きし、使わなくなった期間の件数は消す
    let mut counters = Vec::with_capacity(policy.quotas.len());
    for quota in &policy.quotas {
        let start = quota.period.start_ms(now_ms);
        let count = state
            .counters
            .iter()
            .find(|c| c.period == quota.period && c.period_start_ms == start)
            .map_or(0, |c| c.count);
        counters.push(QuotaCounter {
            period: quota.period,
            period_start_ms: start,
            count,
        });
    }
    state.counters = counters;
    if policy
        .quotas
        .iter()
        .zip(&state.counters)
        .any(|(q, c)| c.count >= q.limit)
    {
        return Decision::QuotaExhausted;
    }
    if state.leases.len() >= policy.max_concurrent as usize {
        let until_ms = state
            .leases
            .iter()
            .map(|l| l.expires_at_ms)
            .min()
            .unwrap_or(now_ms);
        return Decision::Wait { until_ms };
    }
    if let Some(last) = state.last_start_ms {
        let next = last.saturating_add(duration_ms(policy.min_interval));
        if now_ms < next {
            return Decision::Wait { until_ms: next };
        }
    }
    // 送信の開始は判定から最大 `within_ms` 遅れ得るので、その最も遅い時刻を開始時刻とし、
    // リースも送信開始からの有効期間を確保する
    let start_by_ms = now_ms.saturating_add(within_ms);
    // 送信が次の集計期間に入り得るなら、今の期間に数えずに境界まで待って判定し直す
    if let Some(boundary) = policy
        .quotas
        .iter()
        .map(|q| q.period.end_ms(now_ms))
        .filter(|&end| start_by_ms >= end)
        .min()
    {
        return Decision::Wait { until_ms: boundary };
    }
    let lease = LeaseRecord {
        generation: state.next_generation,
        expires_at_ms: start_by_ms.saturating_add(duration_ms(policy.lease_ttl)),
    };
    state.next_generation += 1;
    state.leases.push(lease);
    state.last_start_ms = Some(start_by_ms);
    for c in &mut state.counters {
        c.count += 1;
    }
    Decision::Granted { lease, start_by_ms }
}

fn duration_ms(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

/// 共有状態の読取・書込みに失敗した（競合が解消しない場合を含む）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StoreError;

/// 共有状態の保存方式。
pub trait GateStore: Send + Sync {
    /// `key` の状態を読み、[`apply`] を適用して原子的に書き戻す。
    ///
    /// 判定の時刻は、状態を読んだ後・[`apply`] の直前に `clock` から読む（再試行のたびに読み直す）。
    fn transact<'a>(
        &'a self,
        key: &'a str,
        op: GateOp<'a>,
        clock: &'a dyn Clock,
    ) -> BoxFuture<'a, Result<Decision, StoreError>>;

    /// 判定から呼出し元が結果を受け取るまでに許す遅れ（[`GateOp::Acquire`] の `start_within`）。
    /// これを超えて届いた許可では送信しない。プロセス内で即時に判定する保存方式は0で、
    /// その場合は遅れを検査しない。
    fn start_reserve(&self) -> Duration {
        Duration::ZERO
    }
}

/// 単一プロセス内の保存方式。ローカル開発専用（`UPSTREAM_GATE=local`）。
#[derive(Debug, Default)]
pub struct LocalGateStore {
    states: Mutex<HashMap<String, GateState>>,
}

impl LocalGateStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 現在の状態（テスト用）。
    pub fn snapshot(&self, key: &str) -> Option<GateState> {
        self.states.lock().unwrap().get(key).cloned()
    }
}

impl GateStore for LocalGateStore {
    fn transact<'a>(
        &'a self,
        key: &'a str,
        op: GateOp<'a>,
        clock: &'a dyn Clock,
    ) -> BoxFuture<'a, Result<Decision, StoreError>> {
        Box::pin(async move {
            let mut states = self.states.lock().map_err(|_| StoreError)?;
            let state = states.entry(key.to_owned()).or_default();
            Ok(apply(state, op, clock.now_ms()))
        })
    }
}

/// 同時数の空き待ちで共有状態を読み直す間隔の上限。他インスタンスの解放を拾うため。
const RECHECK_INTERVAL: Duration = Duration::from_millis(100);
/// 取消し・期限を確認する間隔。
const CANCEL_POLL: Duration = Duration::from_millis(10);

/// 提供元1つ分のゲート。複製は同じゲートを共有する。
#[derive(Clone)]
pub struct Gate {
    inner: Arc<GateInner>,
}

struct GateInner {
    key: String,
    policy: GatePolicy,
    store: Arc<dyn GateStore>,
    clock: Arc<dyn Clock>,
    /// 送信を待っている呼出し数（このプロセス内）。
    waiting: AtomicUsize,
    /// このプロセスの待ちを先着順に並べ、先頭だけが共有状態を読む。
    order: tokio::sync::Mutex<()>,
    /// このプロセスでリースを解放したときに待ちを起こす。
    released: Notify,
}

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gate")
            .field("key", &self.inner.key)
            .field("policy", &self.inner.policy)
            .finish_non_exhaustive()
    }
}

impl Gate {
    pub fn new(
        key: impl Into<String>,
        policy: GatePolicy,
        store: Arc<dyn GateStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            inner: Arc::new(GateInner {
                key: key.into(),
                policy,
                store,
                clock,
                waiting: AtomicUsize::new(0),
                order: tokio::sync::Mutex::new(()),
                released: Notify::new(),
            }),
        }
    }

    pub fn policy(&self) -> &GatePolicy {
        &self.inner.policy
    }

    /// 送信の許可を待つ。期限切れ・取消しで待ちを解除する。
    pub async fn acquire(&self, cancel: &CancelToken) -> Result<Lease, GateError> {
        cancel.check()?;
        let inner = &self.inner;
        let _slot =
            QueueSlot::take(&inner.waiting, inner.policy.queue_capacity).ok_or(GateError::Busy)?;
        let _turn = tokio::select! {
            biased;
            turn = inner.order.lock() => turn,
            interrupt = until_interrupted(cancel) => return Err(interrupt.into()),
        };
        let reserve = inner.store.start_reserve();
        let op = GateOp::Acquire {
            policy: &inner.policy,
            start_within: reserve,
        };
        loop {
            cancel.check()?;
            let decision = inner
                .store
                .transact(&inner.key, op, inner.clock.as_ref())
                .await
                .map_err(|_| GateError::Busy)?;
            match decision {
                Decision::Granted {
                    lease: record,
                    start_by_ms,
                } => {
                    let lease = Lease {
                        inner: Some(inner.clone()),
                        record,
                        start_by_ms: (!reserve.is_zero()).then_some(start_by_ms),
                    };
                    // 保存方式の応答が遅れ、記録した開始時刻までに送信を始められない
                    if lease.ensure_valid().is_err() {
                        lease.release().await;
                        return Err(GateError::Busy);
                    }
                    return Ok(lease);
                }
                Decision::QuotaExhausted | Decision::Released => return Err(GateError::Busy),
                Decision::Wait { until_ms } => {
                    let now_ms = inner.clock.now_ms();
                    let wait = Duration::from_millis(until_ms.saturating_sub(now_ms).max(1) as u64)
                        .min(RECHECK_INTERVAL);
                    tokio::select! {
                        biased;
                        interrupt = until_interrupted(cancel) => return Err(interrupt.into()),
                        () = inner.released.notified() => {}
                        () = tokio::time::sleep(wait) => {}
                    }
                }
            }
        }
    }
}

/// 期限切れか取消しまで待ち、その理由を返す。
pub(crate) async fn until_interrupted(cancel: &CancelToken) -> Interrupt {
    loop {
        if let Err(interrupt) = cancel.check() {
            return interrupt;
        }
        let step = cancel
            .remaining()
            .map_or(CANCEL_POLL, |r| r.min(CANCEL_POLL));
        tokio::time::sleep(step).await;
    }
}

/// 待ち行列の1枠。破棄で返す。
struct QueueSlot<'a>(&'a AtomicUsize);

impl<'a> QueueSlot<'a> {
    fn take(waiting: &'a AtomicUsize, capacity: usize) -> Option<Self> {
        // `fetch_update` は新しいRustで非推奨（`try_update` へ改名）、`try_update` は1.88に無いので
        // compare_exchangeで書く
        let mut n = waiting.load(Ordering::Acquire);
        loop {
            if n >= capacity {
                return None;
            }
            match waiting.compare_exchange_weak(n, n + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(QueueSlot(waiting)),
                Err(current) => n = current,
            }
        }
    }
}

impl Drop for QueueSlot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// 同時通信のリース。通信が終わったら [`Lease::release`] で返す。
/// 返さずに破棄した場合（要求の取消し等）も、非同期に解放する。
pub struct Lease {
    inner: Option<Arc<GateInner>>,
    record: LeaseRecord,
    /// 送信を始めてよい最後の時刻（共有状態に記録した開始時刻）。`None` は検査しない。
    start_by_ms: Option<i64>,
}

impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lease")
            .field("record", &self.record)
            .finish()
    }
}

impl Lease {
    pub fn record(&self) -> LeaseRecord {
        self.record
    }

    /// 送信の直前に呼ぶ。リースの期限が切れていたら、または記録した開始時刻を過ぎていたら
    /// 送信しない。
    ///
    /// 期限切れのリースは共有状態から除かれ、その枠は別の処理に渡り得るため。開始時刻を
    /// 過ぎて送ると、他のインスタンスとの開始間隔を守れないため。
    pub fn ensure_valid(&self) -> Result<(), GateError> {
        let inner = self.inner.as_ref().expect("解放前のリース");
        let now_ms = inner.clock.now_ms();
        let started_in_time = self.start_by_ms.is_none_or(|by| now_ms <= by);
        if started_in_time && now_ms < self.record.expires_at_ms {
            Ok(())
        } else {
            Err(GateError::Busy)
        }
    }

    /// リースの残りの有効期間。1通信の期限をこれ以下に抑える。
    pub fn remaining(&self) -> Duration {
        let inner = self.inner.as_ref().expect("解放前のリース");
        let left = self
            .record
            .expires_at_ms
            .saturating_sub(inner.clock.now_ms());
        Duration::from_millis(u64::try_from(left).unwrap_or(0))
    }

    /// リースを返す。共有状態の書込みに失敗しても、期限切れで自然に除かれる。
    pub async fn release(mut self) {
        if let Some(inner) = self.inner.take() {
            release(inner, self.record.generation).await;
        }
    }
}

async fn release(inner: Arc<GateInner>, generation: u64) {
    let _ = inner
        .store
        .transact(
            &inner.key,
            GateOp::Release { generation },
            inner.clock.as_ref(),
        )
        .await;
    inner.released.notify_waiters();
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(release(inner, self.record.generation));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> GatePolicy {
        GatePolicy {
            min_interval: Duration::from_millis(1050),
            max_concurrent: 1,
            queue_capacity: 16,
            lease_ttl: Duration::from_secs(10),
            quotas: vec![Quota {
                period: QuotaPeriod::Day,
                limit: 2,
            }],
        }
    }

    #[test]
    fn interval_concurrency_and_quota_are_decided_from_the_state_alone() {
        let p = policy();
        let mut state = GateState::default();
        let Decision::Granted { lease: first, .. } = apply(
            &mut state,
            GateOp::Acquire {
                policy: &p,
                start_within: Duration::ZERO,
            },
            1_000,
        ) else {
            panic!("最初は許可");
        };
        // 同時1：解放までは待つ（最短でリース期限まで）
        assert_eq!(
            apply(
                &mut state,
                GateOp::Acquire {
                    policy: &p,
                    start_within: Duration::ZERO
                },
                5_000
            ),
            Decision::Wait {
                until_ms: first.expires_at_ms
            }
        );
        apply(
            &mut state,
            GateOp::Release {
                generation: first.generation,
            },
            5_000,
        );
        // 開始間隔
        let mut early = state.clone();
        early.last_start_ms = Some(4_500);
        assert_eq!(
            apply(
                &mut early,
                GateOp::Acquire {
                    policy: &p,
                    start_within: Duration::ZERO
                },
                5_000
            ),
            Decision::Wait { until_ms: 5_550 }
        );
        assert!(matches!(
            apply(
                &mut state,
                GateOp::Acquire {
                    policy: &p,
                    start_within: Duration::ZERO
                },
                5_000
            ),
            Decision::Granted { .. }
        ));
        state.leases.clear();
        // 1日2件の上限
        assert_eq!(
            apply(
                &mut state,
                GateOp::Acquire {
                    policy: &p,
                    start_within: Duration::ZERO
                },
                7_000
            ),
            Decision::QuotaExhausted
        );
        // 翌日（UTC）は件数を上書きする
        assert!(matches!(
            apply(
                &mut state,
                GateOp::Acquire {
                    policy: &p,
                    start_within: Duration::ZERO
                },
                86_400_000 + 1
            ),
            Decision::Granted { .. }
        ));
        assert_eq!(state.counters[0].count, 1);
    }

    #[test]
    fn start_within_delays_the_recorded_start_and_the_lease() {
        let p = policy();
        let mut state = GateState::default();
        let op = GateOp::Acquire {
            policy: &p,
            start_within: Duration::from_millis(500),
        };
        assert_eq!(
            apply(&mut state, op, 1_000),
            Decision::Granted {
                lease: LeaseRecord {
                    generation: 0,
                    expires_at_ms: 11_500,
                },
                start_by_ms: 1_500,
            }
        );
        assert_eq!(state.last_start_ms, Some(1_500));
    }

    /// 送信の遅れが集計期間の境界をまたぎ得るなら、境界まで待たせて次の期間に数える。
    #[test]
    fn grant_that_could_start_in_the_next_period_waits_for_the_boundary() {
        let mut p = policy();
        p.quotas.push(Quota {
            period: QuotaPeriod::Minute,
            limit: 10,
        });
        let op = GateOp::Acquire {
            policy: &p,
            start_within: Duration::from_millis(500),
        };
        let mut state = GateState::default();
        let day = 86_400_000;
        assert_eq!(
            apply(&mut state, op, day - 500),
            Decision::Wait { until_ms: day }
        );
        assert_eq!(
            apply(&mut state, op, day - 1),
            Decision::Wait { until_ms: day }
        );
        let Decision::Granted { start_by_ms, .. } = apply(&mut state, op, day) else {
            panic!("境界の後は許可");
        };
        assert_eq!(start_by_ms, day + 500);
        assert!(
            state
                .counters
                .iter()
                .all(|c| c.period_start_ms == day && c.count == 1)
        );
        // 分の境界でも同じ（日の途中）
        let minute_end = day + 120_000;
        let mut state = GateState::default();
        assert_eq!(
            apply(&mut state, op, minute_end - 200),
            Decision::Wait {
                until_ms: minute_end
            }
        );
        assert!(state.counters.iter().all(|c| c.count == 0));
        // 遅れを検査しない保存方式（0）は境界の直前でも許可する
        let zero = GateOp::Acquire {
            policy: &p,
            start_within: Duration::ZERO,
        };
        assert!(matches!(
            apply(&mut GateState::default(), zero, day - 1),
            Decision::Granted { .. }
        ));
    }

    #[test]
    fn period_ends_at_the_next_utc_boundary() {
        let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().timestamp_millis();
        let t = at("2026-12-31T23:59:59.900Z");
        assert_eq!(QuotaPeriod::Minute.end_ms(t), at("2027-01-01T00:00:00Z"));
        assert_eq!(QuotaPeriod::Hour.end_ms(t), at("2027-01-01T00:00:00Z"));
        assert_eq!(QuotaPeriod::Day.end_ms(t), at("2027-01-01T00:00:00Z"));
        assert_eq!(QuotaPeriod::Month.end_ms(t), at("2027-01-01T00:00:00Z"));
        let t = at("2026-02-14T10:30:00Z");
        assert_eq!(QuotaPeriod::Month.end_ms(t), at("2026-03-01T00:00:00Z"));
        assert_eq!(QuotaPeriod::Hour.end_ms(t), at("2026-02-14T11:00:00Z"));
    }

    #[test]
    fn month_period_starts_at_utc_first_day() {
        let t = DateTime::parse_from_rfc3339("2026-10-24T09:00:00+09:00")
            .unwrap()
            .timestamp_millis();
        let start = QuotaPeriod::Month.start_ms(t);
        assert_eq!(
            DateTime::<Utc>::from_timestamp_millis(start)
                .unwrap()
                .to_rfc3339(),
            "2026-10-01T00:00:00+00:00"
        );
    }
}
