//! `RiskView` を探索の [`ArcCost`] に包むアダプタと、確定した経路のリスク（形式書14.5節、18章）。
//!
//! 探索は固定小数点 `_fx` の和だけを見る。応答の `weighted_risk` などは、確定した経路について
//! [`path_risk`] で `f64` のまま計算し直す（形式書1章、behavior.md 5章）。

use crate::domain::routing::graph::DirEdge;
use crate::domain::routing::search::endpoints::{Placement, placement, travelled_lengths_dm};
use crate::domain::routing::search::{ArcCost, Path, risk_to_fx};

use super::RiskView;

/// [`RiskView`] を [`ArcCost`] として使うためのアダプタ。
pub struct RiskArcCost<'v, 'a> {
    view: &'v mut RiskView<'a>,
}

impl<'v, 'a> RiskArcCost<'v, 'a> {
    pub fn new(view: &'v mut RiskView<'a>) -> Self {
        Self { view }
    }
}

impl ArcCost for RiskArcCost<'_, '_> {
    fn risk_fx(&mut self, d: DirEdge) -> u64 {
        risk_to_fx(self.view.edge_risk(d.edge()))
    }

    fn partial_risk_fx(&mut self, d: DirEdge, from_dm: u32, to_dm: u32) -> u64 {
        risk_to_fx(self.view.partial_edge_risk(d.edge(), from_dm, to_dm))
    }

    fn cluster_entry_risk_fx(&mut self, d: DirEdge) -> u64 {
        let entering = self.view.topology().entering_cluster(d, self.view.graph());
        match entering {
            Some(cluster) => risk_to_fx(self.view.cluster_risk(cluster)),
            None => 0,
        }
    }
}

/// 確定した経路のリスクを `f64` で求める（形式書14.5節、behavior.md 5章）。
///
/// [`Path`] の出発・到着の部分有向辺の規則に従う。
///
/// - 出発の辺（吸着点が辺の途中）：吸着点から辺の終端までの部分＋到着ノードのクラスタ。
/// - 途中の辺：全長＋到着ノードのクラスタ。
/// - 到着の辺（吸着点が辺の途中）：辺の始点から吸着点までの部分だけ（クラスタなし）。
/// - 出発と到着が同じ辺の途中で、1本の有向辺で直接行ける場合：2つの吸着点のあいだだけ。
///
/// 経路の形が正しくない（[`travelled_lengths_dm`] が `None`）ときは0。
pub fn path_risk(view: &mut RiskView<'_>, path: &Path) -> f64 {
    let graph = view.graph();
    let Some(lengths) = travelled_lengths_dm(graph, &path.start, &path.end, &path.dedges) else {
        return 0.0;
    };
    let start_inside = placement(graph, &path.start) == Some(Placement::Inside);
    let end_inside = placement(graph, &path.end) == Some(Placement::Inside);
    let last = path.dedges.len() - 1;
    // 同じ辺の途中から途中へ、1本の有向辺で直接行く場合。
    let direct = path.dedges.len() == 1 && start_inside && end_inside;
    let mut total = 0.0;
    for (i, &d) in path.dedges.iter().enumerate() {
        let edge = d.edge();
        let length = graph.edge(edge).length_dm;
        let (from_dm, to_dm) = if direct {
            (
                path.start.offset_dm.min(path.end.offset_dm),
                path.start.offset_dm.max(path.end.offset_dm),
            )
        } else if i == 0 && start_inside {
            if d.is_reverse() {
                (0, path.start.offset_dm)
            } else {
                (path.start.offset_dm, length)
            }
        } else if i == last && end_inside {
            if d.is_reverse() {
                (path.end.offset_dm, length)
            } else {
                (0, path.end.offset_dm)
            }
        } else {
            (0, length)
        };
        // lengths は travelled_lengths_dm が形を確かめた値。全長ならキャッシュつきの edge_risk を使う。
        let full = from_dm == 0 && to_dm == length && lengths[i] == length;
        let mut risk = if full {
            view.edge_risk(edge)
        } else {
            view.partial_edge_risk(edge, from_dm, to_dm)
        };
        // 到着の辺（途中まで）は到着ノードに届かないので、クラスタを加えない。
        // 1本の辺の中で2つの吸着点のあいだを行く場合も、辺の端点には届かない。
        let arrival_partial = !direct && i == last && end_inside;
        if !direct && !arrival_partial {
            let entering = view.topology().entering_cluster(d, view.graph());
            if let Some(cluster) = entering {
                risk += view.cluster_risk(cluster);
            }
        }
        total += risk;
    }
    total
}
