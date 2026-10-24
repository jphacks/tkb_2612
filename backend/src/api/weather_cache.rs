//! 気象応答のメモリキャッシュ（behavior.md 9章「期限・キャッシュ・レート制御」）。
//!
//! 丸めた(lat, lon)をキーに、成功した上流応答だけをプロセスメモリに保持する。
//! TTLは `WEATHER_CACHE_TTL_SEC`、最大1024キーで、満杯なら挿入順の最古を除く。
//! 永続化・プロセス間共有はしない。利用者・要求との対応付けは持たない。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::contract::CancelToken;
use crate::providers::clock::Clock;
use crate::providers::open_meteo::{RoundedPoint, WeatherError, WeatherProvider, WeatherReport};

/// 保持するキーの上限。
pub const MAX_ENTRIES: usize = 1024;

pub struct WeatherCache {
    provider: Arc<dyn WeatherProvider>,
    clock: Arc<dyn Clock>,
    ttl_ms: i64,
    entries: Mutex<Entries>,
}

#[derive(Default)]
struct Entries {
    reports: HashMap<RoundedPoint, (WeatherReport, i64)>,
    /// 挿入順（古い順）。
    order: VecDeque<RoundedPoint>,
}

impl WeatherCache {
    pub fn new(provider: Arc<dyn WeatherProvider>, clock: Arc<dyn Clock>, ttl: Duration) -> Self {
        Self {
            provider,
            clock,
            ttl_ms: i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX),
            entries: Mutex::new(Entries::default()),
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
        if let Some((report, stored_ms)) = self.entries.lock().unwrap().reports.get(&point)
            && now_ms.saturating_sub(*stored_ms) < self.ttl_ms
        {
            return Ok(report.clone());
        }
        let report = self.provider.fetch(point, cancel).await?;
        let stored_ms = self.clock.now_ms();
        let mut entries = self.entries.lock().unwrap();
        if entries.reports.remove(&point).is_some() {
            entries.order.retain(|p| *p != point);
        }
        while entries.reports.len() >= MAX_ENTRIES {
            let Some(oldest) = entries.order.pop_front() else {
                break;
            };
            entries.reports.remove(&oldest);
        }
        entries.reports.insert(point, (report.clone(), stored_ms));
        entries.order.push_back(point);
        Ok(report)
    }
}
