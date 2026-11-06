//! `RouteEngine` の実装（schema 3、形式書18章）。
//!
//! `collect_candidates` はルーターを呼ばず空の `CandidateSet` を返し、`evaluate` が
//! 吸着・探索・集計・選定・文面までを行う。探索の部品は `evaluate` モジュールに置く。
use std::sync::Arc;

use crate::contract::{
    BoxFuture, CancelToken, CandidateSet, DatasetInfo, EngineError, Mode, RouteEngine,
    RouteOutcome, RouteQuery, Router,
};
use crate::domain::AccidentSnapshot;

use super::RoutingData;
use super::graph::restrictions::TurnAutomaton;
use super::search::SearchParams;

/// 新方式のエンジンが使う探索の設定（M5.6の決定。M5.md の時間予算の節）。
///
/// 自動車600組（全国300・関東4地域300）の計測で、処理時間の99%点が10秒以下に収まる範囲から選んだ。
/// `SearchParams::DEFAULT` は既存のテストが固定しているので変えず、エンジンではこちらを使う。
pub const ENGINE_SEARCH_PARAMS: SearchParams = SearchParams {
    rcsp_budget_ms: 2_000,
    larac_iterations: 10,
    ellipse_node_limit: 300_000,
    rcsp_label_limit: 2_000_000,
};

/// 手段の添字（walk・bike・car）。
fn mode_slot(mode: Mode) -> usize {
    match mode {
        Mode::Walk => 0,
        Mode::Bike => 1,
        Mode::Car => 2,
    }
}

/// 自前の道路グラフで候補を探すエンジン。
#[derive(Debug, Clone)]
pub struct RoutingEngine {
    data: Arc<RoutingData>,
    snapshot: Arc<AccidentSnapshot>,
    dataset: DatasetInfo,
    params: SearchParams,
    /// 手段ごとの右左折禁止の照合器（walk・bike・car）。
    automata: [TurnAutomaton; 3],
}

impl RoutingEngine {
    pub fn new(
        data: Arc<RoutingData>,
        snapshot: Arc<AccidentSnapshot>,
        dataset: DatasetInfo,
    ) -> Self {
        let automata = [
            TurnAutomaton::build(&data.restrictions, Mode::Walk),
            TurnAutomaton::build(&data.restrictions, Mode::Bike),
            TurnAutomaton::build(&data.restrictions, Mode::Car),
        ];
        Self {
            data,
            snapshot,
            dataset,
            params: ENGINE_SEARCH_PARAMS,
            automata,
        }
    }

    /// 探索の設定を差し替える（テスト用）。
    pub fn with_params(mut self, params: SearchParams) -> Self {
        self.params = params;
        self
    }

    pub fn data(&self) -> &RoutingData {
        &self.data
    }

    pub fn snapshot(&self) -> &AccidentSnapshot {
        &self.snapshot
    }

    /// [`RouteEngine::evaluate`] と同じ計算を行い、応答に出さない計測用の値も返す。
    /// 評価コマンド（`routing_eval engine-batch`）だけが使う。
    pub fn evaluate_with_report(
        &self,
        query: &RouteQuery,
        cancel: &CancelToken,
    ) -> Result<(RouteOutcome, super::evaluate::EvaluationReport), EngineError> {
        super::evaluate::evaluate_with_report(self, query, cancel)
    }

    pub(crate) fn params(&self) -> SearchParams {
        self.params
    }

    pub(crate) fn automaton(&self, mode: Mode) -> &TurnAutomaton {
        &self.automata[mode_slot(mode)]
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
        query: &RouteQuery,
        _candidates: CandidateSet,
        cancel: &CancelToken,
    ) -> Result<RouteOutcome, EngineError> {
        super::evaluate::evaluate(self, query, cancel)
    }
}
