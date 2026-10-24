//! 壁時計の注入。
//!
//! `/api/conditions` の天候判定（現在±15分等）と、上流ゲートの集計期間・リース期限が使う。
//! テストとC2の生成では固定・手動の時計に差し替える。

use chrono::{DateTime, Utc};

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;

    /// UNIXエポックからのミリ秒。上流ゲートの共有状態はこの値で時刻を持つ。
    fn now_ms(&self) -> i64 {
        self.now().timestamp_millis()
    }
}

/// OSの時計。
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
