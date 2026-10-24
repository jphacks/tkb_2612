//! 気象応答のメモリキャッシュ（behavior.md 9章「期限・キャッシュ・レート制御」）。
//!
//! 丸めた(lat, lon)をキーに、成功した上流応答だけをプロセスメモリに保持する。
//! TTLは `WEATHER_CACHE_TTL_SEC`、最大1024キーで、満杯なら挿入順の最古を除く。
//! 保存した各キーは、再アクセスが無くても保存からTTLで消す（architecture.md 8章「最大10分保持」）。
//! TTLが0なら保存しない。永続化・プロセス間共有はしない。利用者・要求との対応付けは持たない。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use crate::contract::CancelToken;
use crate::providers::clock::Clock;
use crate::providers::open_meteo::{RoundedPoint, WeatherError, WeatherProvider, WeatherReport};

/// 保持するキーの上限。
pub const MAX_ENTRIES: usize = 1024;

pub struct WeatherCache {
    provider: Arc<dyn WeatherProvider>,
    clock: Arc<dyn Clock>,
    ttl: Duration,
    ttl_ms: i64,
    entries: Arc<Mutex<Entries>>,
}

#[derive(Default)]
struct Entries {
    reports: HashMap<RoundedPoint, Entry>,
    /// 挿入順（古い順）。
    order: VecDeque<RoundedPoint>,
    next_generation: u64,
}

struct Entry {
    report: WeatherReport,
    stored_ms: i64,
    /// 保存ごとの番号。期限の消去が、後から保存し直した応答を消さないようにする。
    generation: u64,
}

impl Entries {
    fn remove(&mut self, point: &RoundedPoint) {
        if self.reports.remove(point).is_some() {
            self.order.retain(|p| p != point);
        }
    }
}

impl WeatherCache {
    pub fn new(provider: Arc<dyn WeatherProvider>, clock: Arc<dyn Clock>, ttl: Duration) -> Self {
        Self {
            provider,
            clock,
            ttl,
            ttl_ms: i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX),
            entries: Arc::new(Mutex::new(Entries::default())),
        }
    }

    /// 保持しているキーの数（テスト用）。
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().reports.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 保存から `TTL` 未満ならキャッシュを返し、そうでなければ上流から取得して保存する。
    pub async fn get(
        &self,
        point: RoundedPoint,
        cancel: &CancelToken,
    ) -> Result<WeatherReport, WeatherError> {
        let now_ms = self.clock.now_ms();
        {
            let mut entries = self.entries.lock().unwrap();
            match entries.reports.get(&point) {
                Some(e) if now_ms.saturating_sub(e.stored_ms) < self.ttl_ms => {
                    return Ok(e.report.clone());
                }
                // 期限切れは取り直しの成否に関係なく消す
                Some(_) => entries.remove(&point),
                None => {}
            }
        }
        let report = self.provider.fetch(point, cancel).await?;
        if self.ttl.is_zero() {
            return Ok(report);
        }
        let stored_ms = self.clock.now_ms();
        let generation = {
            let mut entries = self.entries.lock().unwrap();
            entries.remove(&point);
            while entries.reports.len() >= MAX_ENTRIES {
                let Some(oldest) = entries.order.pop_front() else {
                    break;
                };
                entries.reports.remove(&oldest);
            }
            let generation = entries.next_generation;
            entries.next_generation += 1;
            entries.reports.insert(
                point,
                Entry {
                    report: report.clone(),
                    stored_ms,
                    generation,
                },
            );
            entries.order.push_back(point);
            generation
        };
        tokio::spawn(expire(
            Arc::downgrade(&self.entries),
            point,
            generation,
            self.ttl,
        ));
        Ok(report)
    }
}

/// 保存から `ttl` 後に、その保存の応答がまだ残っていれば消す。キャッシュが破棄されていれば何もしない。
async fn expire(
    entries: Weak<Mutex<Entries>>,
    point: RoundedPoint,
    generation: u64,
    ttl: Duration,
) {
    tokio::time::sleep(ttl).await;
    let Some(entries) = entries.upgrade() else {
        return;
    };
    let mut entries = entries.lock().unwrap();
    if entries
        .reports
        .get(&point)
        .is_some_and(|e| e.generation == generation)
    {
        entries.remove(&point);
    }
}
