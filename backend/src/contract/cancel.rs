//! 期限・キャンセルの伝達。
//!
//! ③が要求ごとに [`CancelToken`] を作り（route/geocodeは受付から35秒）、②と経路アダプタへ渡す。
//! - 非同期処理（上流待ち）は、③が期限付きで待つか、要求のfutureが破棄されて止まる。
//! - CPU処理（集計）はfutureの破棄では止まらないので、②が [`CancelToken::check`] を
//!   定期的に呼んで止まる。③は応答を返さずに終わる経路で [`CancelGuard`] を使い、必ず取消す。
//! - 結果を採用する直前に③が [`CancelToken::settle`] を呼び、期限後の結果を捨てる。
//!
//! HTTP応答への変換は③の責務（README.mdの対応表）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// 処理を中断した理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Interrupt {
    /// 要求全体の期限を過ぎた。route/geocodeでは503 service_busy。
    DeadlineExceeded,
    /// 要求側が取り消した（クライアント切断等）。応答は送らない。
    Cancelled,
}

/// 要求全体の期限と取消し状態。複製は同じ状態を共有する。
#[derive(Debug, Clone)]
pub struct CancelToken {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    deadline: Option<Instant>,
    cancelled: AtomicBool,
}

impl CancelToken {
    /// 期限付きのトークン。
    pub fn with_deadline(deadline: Instant) -> Self {
        Self::build(Some(deadline))
    }

    /// 今から `timeout` 後を期限とするトークン。
    pub fn with_timeout(timeout: Duration) -> Self {
        Self::with_deadline(Instant::now() + timeout)
    }

    /// 期限なし。前処理コマンドやテスト用で、取消しだけを受け付ける。
    pub fn unbounded() -> Self {
        Self::build(None)
    }

    fn build(deadline: Option<Instant>) -> Self {
        Self {
            inner: Arc::new(Inner {
                deadline,
                cancelled: AtomicBool::new(false),
            }),
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.inner.deadline
    }

    /// 期限までの残り時間。期限なしは `None`、期限切れは0。
    pub fn remaining(&self) -> Option<Duration> {
        self.inner
            .deadline
            .map(|d| d.saturating_duration_since(Instant::now()))
    }

    /// 取り消す。複数回呼んでよい。
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    /// 続行してよいか。取消しを期限切れより優先して返す。
    ///
    /// ②はCPU処理の中で、上流呼出しの前後と、少なくとも約10msごと
    /// （例：事故・小線分のループ1024回ごと）に呼ぶ。1回の呼出しは軽い。
    pub fn check(&self) -> Result<(), Interrupt> {
        if self.is_cancelled() {
            return Err(Interrupt::Cancelled);
        }
        match self.inner.deadline {
            Some(deadline) if Instant::now() >= deadline => Err(Interrupt::DeadlineExceeded),
            _ => Ok(()),
        }
    }

    /// 結果を採用する直前に呼ぶ。期限切れ・取消し済みなら、成功・失敗を問わず中断に置き換える。
    ///
    /// 処理の完了と期限切れが同時に観測された場合（`timeout_at` は処理側を先にpollする）
    /// も、期限後の結果を200にしないため。③はJSONの準備と圧縮が終わった時点でこれを呼ぶ。
    pub fn settle<T, E: From<Interrupt>>(&self, result: Result<T, E>) -> Result<T, E> {
        match self.check() {
            Err(interrupt) => Err(E::from(interrupt)),
            Ok(()) => result,
        }
    }

    /// 破棄時に取り消すガード。要求のfutureが途中で破棄されたときに、
    /// `spawn_blocking` 等で動く集計も止めるために③が使う。
    pub fn cancel_on_drop(&self) -> CancelGuard {
        CancelGuard {
            token: Some(self.clone()),
        }
    }
}

/// [`CancelToken::cancel_on_drop`] のガード。正常終了時は [`CancelGuard::disarm`] で解除する。
#[derive(Debug)]
pub struct CancelGuard {
    token: Option<CancelToken>,
}

impl CancelGuard {
    /// 取り消さずにガードを外す。
    pub fn disarm(mut self) {
        self.token = None;
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if let Some(token) = self.token.take() {
            token.cancel();
        }
    }
}
