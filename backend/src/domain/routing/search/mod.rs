//! 探索（形式書18章、計画書6〜7章）：最短時間経路、楕円の切り出し、LARAC、制約付き最短路、
//! ALT、候補の整理。
//!
//! 中断は `&CancelToken` を引数に取り、`Result<_, Interrupt>` で返す。
//! 時間は `_ds`（デシ秒）、リスクは探索の比較だけに固定小数点 `_fx`（`RISK_SCALE` 倍）を使う
//! （形式書1章）。

pub mod alt;
pub mod candidates;
pub mod ellipse;
pub mod endpoints;
pub mod eval_cost;
pub mod fastest;
pub mod larac;
pub mod pipeline;
pub mod rcsp;
pub mod reduced;
pub mod route;
pub mod subgraph_search;
pub mod turn_dijkstra;

use crate::contract::Mode;
use crate::domain::routing::RISK_SCALE;
use crate::domain::routing::graph::restrictions::TurnAutomaton;
use crate::domain::routing::graph::{DirEdge, Graph};
use crate::domain::routing::snap::SnapPoint;
use crate::domain::routing::topology::Topology;

use self::alt::Landmarks;

/// 有向辺を通るときのリスク（固定小数点）。探索の外から与える。実装は `RiskView` を包むアダプタ。
///
/// 部分グラフの組み立て（`ellipse::assemble`）が、辺ごとに次の3つを足し合わせて `SubArc::risk_fx` にする。
/// 探索（LARAC・RCSP など）は `SubArc::risk_fx` の和だけを見る。
///
/// - 通常の辺：`risk_fx(d) + cluster_entry_risk_fx(d)`。
/// - 出発の辺（途中から通る）：`partial_risk_fx(d, ..) + cluster_entry_risk_fx(d)`。出発点から辺の終端
///   （`d` の到着ノード）までを通るので、到着ノードのクラスタに外から入る。
/// - 到着の辺（途中まで通る）：`partial_risk_fx(d, ..)` だけ。到着ノードには届かないので、クラスタの加算はない。
pub trait ArcCost {
    /// 有向辺 `d` を全長通るときのリスク（`RISK_SCALE` 倍して四捨五入した値）。
    fn risk_fx(&mut self, d: DirEdge) -> u64;

    /// 有向辺 `d` の辺のうち、辺の始点（`tail`）から `from_dm` 〜 `to_dm` の区間だけを通るときの
    /// リスク。区間は向きによらず辺の始点からの距離で表す（`0 ≤ from_dm < to_dm ≤ length_dm`）。
    /// 出発・到着の辺で使う。`RiskView::partial_edge_risk` に当たる。
    fn partial_risk_fx(&mut self, d: DirEdge, from_dm: u32, to_dm: u32) -> u64;

    /// 有向辺 `d` を通って交差点クラスタに外から入るときの、クラスタのリスク（クラスタに入らない
    /// 辺は0）。既定は0。`Topology::entering_cluster` が `Some` の辺で `RiskView::cluster_risk` を返す。
    fn cluster_entry_risk_fx(&mut self, _d: DirEdge) -> u64 {
        0
    }
}

/// リスク（`f64`）を固定小数点にする。
pub fn risk_to_fx(risk: f64) -> u64 {
    (risk * RISK_SCALE).round() as u64
}

/// 固定小数点のリスクを `f64` に戻す。応答値には使わず、確定した経路について `f64` で計算し直す。
pub fn risk_from_fx(risk_fx: u64) -> f64 {
    risk_fx as f64 / RISK_SCALE
}

/// 探索で見つけた経路。
///
/// 出発・到着の辺の扱い（部分有向辺）：`dedges[0]` の辺が出発の吸着点の辺（`start.edge`）なら、
/// 出発点から辺の終端までを通る（向き0なら `offset_dm` から辺の終点まで、向き1なら辺の始点まで
/// `offset_dm` の長さ）。`dedges` の最後の辺が到着の吸着点の辺（`end.edge`）なら、辺の始点側から
/// 到着点までだけを通る。それ以外の有向辺は全長を通る。吸着点が辺の端のノード（`offset_dm` が0か
/// 辺の長さ）のときは、そのノードから出発・到着するだけで、出発・到着の辺を通るとは限らない。
/// 出発・到着が同じ辺で `dedges` が1本のときは、その辺の2つの吸着点のあいだだけを通る。
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    /// 出発の辺（途中から）から到着の辺（途中まで）までの有向辺の並び。
    pub dedges: Vec<DirEdge>,
    /// 出発の吸着点。
    pub start: SnapPoint,
    /// 到着の吸着点。
    pub end: SnapPoint,
    /// 経路の時間（デシ秒）：辺の時間＋ノードの加算＋右左折コスト。
    pub time_ds: u64,
    /// 探索の比較に使ったリスク（固定小数点）。
    pub risk_fx: u64,
}

/// 探索の結果の状態（計画書6章）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchStatus {
    /// 最適性を確認した。
    Optimal,
    /// 時間予算内で、下界との差（gap）つきで打ち切った。
    Bounded,
    /// 探索範囲を縮小した（`reduced`、計画書6.4節）。
    Reduced,
    /// 最短時間経路だけを返す。
    FastestOnly,
}

/// 探索のパラメータ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchParams {
    /// 制約付き最短路に使う時間（ミリ秒）。
    pub rcsp_budget_ms: u64,
    /// LARACの反復回数の上限。
    pub larac_iterations: u32,
    /// 楕円の切り出しで許すノード数の上限。超えたら `EllipseError::TooLarge`。
    pub ellipse_node_limit: u32,
    /// 制約付き最短路のラベル数の上限（メモリの上限）。超えたら最良解を返し、最適とは記録しない。
    pub rcsp_label_limit: u32,
}

impl SearchParams {
    /// 初期値（M3bの計測で見直す）。
    pub const DEFAULT: SearchParams = SearchParams {
        rcsp_budget_ms: 2000,
        larac_iterations: 10,
        ellipse_node_limit: 3_000_000,
        rcsp_label_limit: 2_000_000,
    };
}

impl Default for SearchParams {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// 1回の探索が参照する常駐データ（読み取り専用）と手段。
#[derive(Clone, Copy)]
pub struct SearchContext<'a> {
    pub graph: &'a Graph,
    pub topology: &'a Topology,
    /// この手段の禁止の照合器（`TurnAutomaton::build(restrictions, mode)`）。
    pub automaton: &'a TurnAutomaton,
    pub landmarks: &'a Landmarks,
    pub mode: Mode,
    pub params: SearchParams,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_fixed_point() {
        assert_eq!(SearchParams::DEFAULT.rcsp_budget_ms, 2000);
        assert_eq!(SearchParams::DEFAULT.larac_iterations, 10);
        assert_eq!(SearchParams::DEFAULT.ellipse_node_limit, 3_000_000);
        assert_eq!(SearchParams::default(), SearchParams::DEFAULT);
        assert_eq!(risk_to_fx(1.5), 1_500_000);
        assert_eq!(risk_to_fx(0.000_000_4), 0);
        assert_eq!(risk_to_fx(0.000_000_5), 1);
        assert_eq!(risk_from_fx(2_500_000), 2.5);
    }

    #[test]
    fn label_limit_default() {
        assert_eq!(SearchParams::DEFAULT.rcsp_label_limit, 2_000_000);
    }
}
