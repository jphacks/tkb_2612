//! 評価専用の探索の目的（`routing_eval route-batch` の `--objective length|random`）と、
//! 経路の道路種別ごとの長さ（`--paths true` の `class_lengths_m`）。
//!
//! サーバのエンジンは [`crate::domain::routing::risk::arc_cost::RiskArcCost`] だけを使う。ここにある
//! [`LengthArcCost`]・[`RandomArcCost`] は比較用で、探索の「リスク」の代わりに長さ（m）や
//! 長さ×乱数を使う。記録するリスクは、この目的とは別に `RiskView` で計算し直す。
//!
//! 固定小数点の倍率は [`crate::domain::routing::RISK_SCALE`]（1e6）をそのまま使う。長さは最大
//! 250,000m（`candidates::MAX_ROUTE_DISTANCE_DM`）なので、長さの固定小数点は最大 2.5e11、乱数を
//! 掛けても 5e11 で `u64` に収まる。0.1m は 1e5 になるので、長さの丸めで0になることもない。

use std::collections::BTreeMap;

use crate::domain::routing::graph::{DirEdge, Graph};

use super::ArcCost;
use super::Path;
use super::endpoints::travelled_lengths_dm;
use super::risk_to_fx;

/// 辺の長さ（m）。デシメートルを10で割る。
fn edge_length_m(graph: &Graph, d: DirEdge) -> f64 {
    f64::from(graph.edge(d.edge()).length_dm) / 10.0
}

/// 辺のリスクの代わりに長さ（m）を使う [`ArcCost`]。部分辺は通った長さ、クラスタの加算は0。
/// LARAC・RCSP は「時間の予算内で長さが最小」の経路を探す。
pub struct LengthArcCost<'a> {
    graph: &'a Graph,
}

impl<'a> LengthArcCost<'a> {
    pub fn new(graph: &'a Graph) -> Self {
        Self { graph }
    }
}

impl ArcCost for LengthArcCost<'_> {
    fn risk_fx(&mut self, d: DirEdge) -> u64 {
        risk_to_fx(edge_length_m(self.graph, d))
    }

    fn partial_risk_fx(&mut self, _d: DirEdge, from_dm: u32, to_dm: u32) -> u64 {
        if from_dm >= to_dm {
            0
        } else {
            risk_to_fx(f64::from(to_dm - from_dm) / 10.0)
        }
    }
}

/// 辺eの乱数 `u(e)`（0.5〜2.0）。`--seed` と辺番号だけから決まる（同じseed・同じ辺なら同じ値）。
/// splitmix64 の出力の上位53ビットを [0,1) の一様乱数にしてから、0.5〜2.0 に写す。
pub fn edge_unit(seed: u64, edge: u32) -> f64 {
    let mut z = seed ^ u64::from(edge).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    // 上位53ビット（f64の仮数部）を [0,1) にする。
    let unit = (z >> 11) as f64 / (1_u64 << 53) as f64;
    0.5 + 1.5 * unit
}

/// 辺eのリスクの代わりに「長さ（m）× `u(e)`」を使う [`ArcCost`]。部分辺も同じ `u(e)` を使い、
/// クラスタの加算は0。比較対象「上限内でのランダム選択」。
pub struct RandomArcCost<'a> {
    graph: &'a Graph,
    seed: u64,
}

impl<'a> RandomArcCost<'a> {
    pub fn new(graph: &'a Graph, seed: u64) -> Self {
        Self { graph, seed }
    }
}

impl ArcCost for RandomArcCost<'_> {
    fn risk_fx(&mut self, d: DirEdge) -> u64 {
        risk_to_fx(edge_length_m(self.graph, d) * edge_unit(self.seed, d.edge()))
    }

    fn partial_risk_fx(&mut self, d: DirEdge, from_dm: u32, to_dm: u32) -> u64 {
        if from_dm >= to_dm {
            0
        } else {
            let length_m = f64::from(to_dm - from_dm) / 10.0;
            risk_to_fx(length_m * edge_unit(self.seed, d.edge()))
        }
    }
}

/// 経路が実際に通る長さ（デシメートル）を、辺の `HighwayClass::code` ごとに合計する。
/// 出発・到着の部分辺は通った部分だけを数える。0の種別は含めない。
/// 経路の形が正しくないときは空。
pub fn class_lengths_dm(graph: &Graph, path: &Path) -> BTreeMap<u8, u64> {
    let mut lengths = BTreeMap::new();
    let Some(travelled) = travelled_lengths_dm(graph, &path.start, &path.end, &path.dedges) else {
        return lengths;
    };
    for (&d, &length_dm) in path.dedges.iter().zip(&travelled) {
        if length_dm == 0 {
            continue;
        }
        *lengths.entry(graph.edge(d.edge()).class).or_insert(0) += u64::from(length_dm);
    }
    lengths
}
