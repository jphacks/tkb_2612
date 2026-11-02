//! 手段別の通行規則（形式書7章、`jp-rules-1`）。
//!
//! P1（`routing_prep osm-graph`）が、wayのタグから `class`・`access`・`flags`・`maxspeed_kmh` を決める
//! 純粋関数。規則を変えたら版を上げ、`graph.bin` の `PARAMS.rules_version` と `metadata.json` に記録する。
//! タグがないことを「禁止なし」や「安全」と同じに扱わない（計画書3.2節）。
// 骨組み：実装時に外す
#![allow(unused_variables)]

use super::HighwayClass;

/// 規則の版（形式書7章）。
pub const RULES_VERSION: &str = "jp-rules-1";

/// wayの分類の結果（形式書6.2節の `EdgeRecord` のうちタグから決まる部分）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WayClassification {
    /// 道路の種類（6.5節）。
    pub class: HighwayClass,
    /// `access` のビット（7.1節・7.2節）。向きは「wayのノード順」を始点→終点とする。
    pub access: u8,
    /// `edge_flags` のビット。
    pub flags: u8,
    /// `maxspeed`（7.4節）。0は不明。
    pub maxspeed_kmh: u8,
}

/// wayのタグ（キー、値）の組から分類する。グラフに入れないwayや、`access` が0になるwayは `None`
/// （形式書6.5節・6.2節）。
pub fn classify_way(tags: &[(&str, &str)]) -> Option<WayClassification> {
    todo!("形式書7.1節・7.2節・7.4節")
}

/// ノードのタグから、`NODEFLG` のビット4〜6（car・bike・walkの通過不可、形式書7.3節）を返す。
/// 障害物でなければ0。
pub fn classify_barrier(tags: &[(&str, &str)]) -> u8 {
    todo!("形式書7.3節")
}

/// `maxspeed` タグの値をkm/hにする（形式書7.4節）。解釈できない値は0、255以上は255。
pub fn parse_maxspeed(value: &str) -> u8 {
    todo!("形式書7.4節")
}
