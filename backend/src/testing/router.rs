//! `Router` のスタブ。

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::contract::{
    BoxFuture, CancelToken, ProviderRoute, RoutedPath, Router, RoutingError, RoutingRequest,
};

/// 応答の1回分。
pub type RouterResponse = Result<Vec<ProviderRoute>, RoutingError>;

/// すべて正常な経路の応答。
pub fn valid_routes(routes: &[RoutedPath]) -> RouterResponse {
    Ok(routes.iter().cloned().map(Ok).collect())
}

/// 呼出し順に決めた応答を返し、受けた要求を記録する。
///
/// 用意した応答より多く呼ばれたらpanicする（上流呼出し回数の検証のため）。
/// 送信前に `cancel.check()` で止まった要求は記録しない。
#[derive(Debug, Default)]
pub struct ScriptedRouter {
    script: Mutex<VecDeque<RouterResponse>>,
    calls: Mutex<Vec<RoutingRequest>>,
}

impl ScriptedRouter {
    pub fn new(responses: impl IntoIterator<Item = RouterResponse>) -> Self {
        Self {
            script: Mutex::new(responses.into_iter().collect()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// 受けた要求（呼出し順）。
    pub fn calls(&self) -> Vec<RoutingRequest> {
        self.calls.lock().unwrap().clone()
    }

    /// まだ返していない応答の数。
    pub fn remaining(&self) -> usize {
        self.script.lock().unwrap().len()
    }
}

impl Router for ScriptedRouter {
    fn route<'a>(
        &'a self,
        request: &'a RoutingRequest,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, RouterResponse> {
        Box::pin(async move {
            cancel.check()?;
            self.calls.lock().unwrap().push(*request);
            self.script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| panic!("ScriptedRouter: 用意した応答を超えて呼ばれました"))
        })
    }
}

/// 応答せず、要求全体の期限切れか取消しまで待ってから中断を返す。
/// 契約どおりに期限を守るアダプタの模型で、③の期限処理の検証に使う。
#[derive(Debug, Default)]
pub struct StallingRouter {
    calls: AtomicUsize,
}

impl StallingRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

const POLL_INTERVAL: Duration = Duration::from_millis(5);

impl Router for StallingRouter {
    fn route<'a>(
        &'a self,
        _request: &'a RoutingRequest,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, RouterResponse> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            loop {
                cancel.check()?;
                let wait = cancel
                    .remaining()
                    .map_or(POLL_INTERVAL, |r| r.min(POLL_INTERVAL));
                tokio::time::sleep(wait).await;
            }
        })
    }
}
