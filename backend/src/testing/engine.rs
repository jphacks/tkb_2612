//! `RouteEngine` のスタブ。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::contract::routing::MAX_ROUTES_PER_CALL;
use crate::contract::{
    BoxFuture, CancelToken, CandidateSet, DatasetInfo, EngineError, RouteEngine, RouteOutcome,
    RouteQuery, Router, RoutingRequest,
};

/// `StubEngine::evaluate` の振る舞い。
#[derive(Debug, Clone)]
pub enum StubEvaluation {
    /// 期限・取消しを確認してから、決めた結果を返す。
    Fixed(Box<RouteOutcome>),
    /// 期限切れか取消しまでCPU処理を続ける（中断の伝達の検証用）。
    SpinUntilInterrupted,
}

/// 計算本体の代わりに、初回の経路取得だけを行い、決めた結果を返す。
///
/// 候補の検証・追加候補・重複排除・集計は行わない。
#[derive(Debug)]
pub struct StubEngine {
    dataset: DatasetInfo,
    evaluation: StubEvaluation,
    evaluate_started: AtomicBool,
    evaluate_finished: AtomicBool,
}

impl StubEngine {
    pub fn new(dataset: DatasetInfo, evaluation: StubEvaluation) -> Self {
        Self {
            dataset,
            evaluation,
            evaluate_started: AtomicBool::new(false),
            evaluate_finished: AtomicBool::new(false),
        }
    }

    /// `evaluate` が呼ばれたか。
    pub fn evaluate_started(&self) -> bool {
        self.evaluate_started.load(Ordering::SeqCst)
    }

    /// `evaluate` が（成功・中断を問わず）戻ったか。
    pub fn evaluate_finished(&self) -> bool {
        self.evaluate_finished.load(Ordering::SeqCst)
    }
}

/// 破棄時に `evaluate_finished` を立てる。
struct FinishFlag<'a>(&'a AtomicBool);

impl Drop for FinishFlag<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

const SPIN_INTERVAL: Duration = Duration::from_millis(1);

impl RouteEngine for StubEngine {
    fn dataset(&self) -> &DatasetInfo {
        &self.dataset
    }

    fn collect_candidates<'a>(
        &'a self,
        router: &'a dyn Router,
        query: &'a RouteQuery,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<CandidateSet, EngineError>> {
        Box::pin(async move {
            cancel.check()?;
            let request = RoutingRequest {
                origin: query.origin(),
                dest: query.dest(),
                mode: query.mode(),
                via: None,
            };
            let mut routes = router.route(&request, cancel).await?;
            routes.truncate(MAX_ROUTES_PER_CALL);
            if routes.is_empty() {
                return Err(EngineError::NoRoute);
            }
            Ok(CandidateSet {
                routes,
                warnings: Vec::new(),
            })
        })
    }

    fn evaluate(
        &self,
        _query: &RouteQuery,
        _candidates: CandidateSet,
        cancel: &CancelToken,
    ) -> Result<RouteOutcome, EngineError> {
        self.evaluate_started.store(true, Ordering::SeqCst);
        let _finish = FinishFlag(&self.evaluate_finished);
        match &self.evaluation {
            StubEvaluation::Fixed(outcome) => {
                cancel.check()?;
                Ok((**outcome).clone())
            }
            StubEvaluation::SpinUntilInterrupted => loop {
                cancel.check()?;
                std::thread::sleep(SPIN_INTERVAL);
            },
        }
    }
}
