//! `RouteEngine` の実装（schema 3、形式書18章）。
//!
//! 骨組みの段階では、`collect_candidates` はルーターを呼ばず空の `CandidateSet` を返し、
//! `evaluate` は未実装。
use std::sync::Arc;

use crate::contract::{
    BoxFuture, CancelToken, CandidateSet, DatasetInfo, EngineError, RouteEngine, RouteOutcome,
    RouteQuery, Router,
};
use crate::domain::AccidentSnapshot;

use super::RoutingData;

/// 自前の道路グラフで候補を探すエンジン。
#[derive(Debug, Clone)]
pub struct RoutingEngine {
    data: Arc<RoutingData>,
    snapshot: Arc<AccidentSnapshot>,
    dataset: DatasetInfo,
}

impl RoutingEngine {
    pub fn new(
        data: Arc<RoutingData>,
        snapshot: Arc<AccidentSnapshot>,
        dataset: DatasetInfo,
    ) -> Self {
        Self {
            data,
            snapshot,
            dataset,
        }
    }

    pub fn data(&self) -> &RoutingData {
        &self.data
    }

    pub fn snapshot(&self) -> &AccidentSnapshot {
        &self.snapshot
    }
}

impl RouteEngine for RoutingEngine {
    fn dataset(&self) -> &DatasetInfo {
        &self.dataset
    }

    /// 自前探索ではルーター（OSRM）を使わないので、呼ばずに空の候補を返す。
    fn collect_candidates<'a>(
        &'a self,
        _router: &'a dyn Router,
        _query: &'a RouteQuery,
        _cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<CandidateSet, EngineError>> {
        Box::pin(async {
            Ok(CandidateSet {
                routes: vec![],
                warnings: vec![],
            })
        })
    }

    fn evaluate(
        &self,
        _query: &RouteQuery,
        _candidates: CandidateSet,
        _cancel: &CancelToken,
    ) -> Result<RouteOutcome, EngineError> {
        todo!("探索・リスク集計・選定（作業書M3b以降）")
    }
}
