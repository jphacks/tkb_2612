//! 自前経路探索（schema 3、担当：②）。
//!
//! モジュール構成と各型のバイト配置は `docs/routing-data-format.md`（以下「形式書」）の
//! 3〜15章と18章に従う。骨組みの段階では、型とシグネチャが形式書を写している。
//! `contract` と `domain` の既存モジュールだけを使い、`data` には依存しない。
//! ファイルの読み書きは `data::routing_files` が行う。

pub mod accident;
pub mod engine;
pub mod eval;
pub mod evaluate;
pub mod graph;
pub mod legality;
pub mod prep;
pub mod risk;
pub mod search;
pub mod snap;
pub mod topology;

pub use accident::{EdgeAccidentIndex, UnassignedGrid};
pub use engine::RoutingEngine;
pub use graph::restrictions::Restrictions;
pub use graph::{Graph, GraphParts};
pub use risk::RiskModel;
pub use search::alt::Landmarks;
pub use snap::OdIndex;
pub use topology::Topology;

/// 「なし」を表す番号（形式書5章）。ノード・辺・クラスタの番号に使う。
pub const NONE: u32 = u32::MAX;
/// リスクの固定小数点の倍率（形式書1章）。探索の比較だけに使う。
pub const RISK_SCALE: f64 = 1e6;
/// 出発地・目的地の吸着距離の上限（m、形式書11章）。上限内に辺がなければ `NoRoute`。
pub const SNAP_MAX_DISTANCE_M: f64 = 300.0;
/// 高リスク区間の件数のしきい値（behavior.md 7章。重み3以上）。
pub const HIGH_RISK_MIN_ACCIDENTS: usize = 3;

/// 常駐データ一式（形式書4.3節の検証を終えたもの）。リクエストの間は読み取り専用で、
/// 複数の探索が同時に参照する。
#[derive(Debug)]
pub struct RoutingData {
    /// `graph.bin`（6章）。
    pub graph: Graph,
    /// `restrictions.bin`（9章）。
    pub restrictions: Restrictions,
    /// `topology.bin`（10章）。
    pub topology: Topology,
    /// `odindex.bin`（11章）。
    pub odindex: OdIndex,
    /// `assignment.bin` を組み替えた辺ごと・クラスタごとの索引（13章）。
    pub accidents: EdgeAccidentIndex,
    /// 帰属不明の事故の格子索引（13章。72m判定用）。事故の座標が要るので、
    /// 事故のスナップショットを読んでから作る。
    pub unassigned: UnassignedGrid,
    /// `risk.bin`（14章）。
    pub risk: RiskModel,
    /// `alt.bin`（15章）。
    pub landmarks: Landmarks,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_follow_the_format_document() {
        assert_eq!(NONE, u32::MAX);
        assert_eq!(RISK_SCALE, 1e6);
        assert_eq!(SNAP_MAX_DISTANCE_M, 300.0);
        assert_eq!(HIGH_RISK_MIN_ACCIDENTS, 3);
    }
}
