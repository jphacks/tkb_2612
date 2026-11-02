//! リスクモデル（形式書14章）：係数表、辺・交差点の種別、リクエストごとのリスク（`RiskView`）。
//!
//! 事故の観測件数 `n_e` はファイルに持たない。リクエストごとに、`assignment.bin` の索引と
//! 事故の列から、探索で触れた辺だけ計算する。
// 骨組み：実装時に外す
#![allow(unused_variables)]

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::contract::{ConditionFilter, DayType, Mode, Period};
use crate::domain::AccidentSnapshot;
use crate::domain::routing::accident::EdgeAccidentIndex;
use crate::domain::routing::graph::time::PerMode;
use crate::domain::routing::graph::{EdgeId, Graph};
use crate::domain::routing::topology::{ClusterId, Topology};

/// `EDGEK` の値：リスクを持たない辺（リスク0）。
pub const EDGE_K_NO_RISK: u8 = 0xFF;
/// `EDGEK` の値：歩道。親の車道（`SWPARENT`）のリスク密度を引き継ぐ。
pub const EDGE_K_SIDEWALK: u8 = 0xFE;
/// セルの数（時間帯5×曜日区分2）。
pub const CELL_COUNT: usize = 10;
/// `KTAB`・`JTAB` の件数（3手段×256種別×10セル）。
pub const TABLE_LEN: usize = 3 * 256 * CELL_COUNT;

/// `KTAB`・`JTAB` のレコード（形式書14.1節、48バイト）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CoefRecord {
    /// 辺：基準の事故密度 R（その手段が関与した、そのセルの事故の、1mあたり・1年あたりの期待件数）。
    /// 交差点：1クラスタあたり・1年あたりの期待件数。
    pub r: f64,
    /// 平均の深刻度 S（`10×死者+負傷者` の、1件あたりの平均）。
    pub s: f64,
    /// 縮小の母数。辺は a_k（1mあたり）、交差点は α_j。同じ (m,種別) の10セルで同じ値。
    pub a: f64,
    /// 天候 w（1〜5）の事故の割合 ρ。添字は `w-1`。
    pub rho: [f32; 5],
    /// 予約（0）。
    pub pad: f32,
}

impl CoefRecord {
    /// `KTAB`・`JTAB` の `elem_size`。
    pub const SIZE: usize = 48;
}

/// 天候の倍率M（形式書14.4節の `weather_multiplier`、現行の値は雨2・霧4・雪4）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeatherMultiplier {
    pub rain: f64,
    pub fog: f64,
    pub snow: f64,
}

/// `risk.bin` の `PARAMS`（形式書14.4節）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RiskParams {
    /// 学習に使ったsource_yearの昇順。
    pub train_source_years: Vec<u16>,
    /// T（学習年数）。
    pub years: u32,
    /// 自動車の生活道路の係数c（計画書5.5節、初期値1.0）。
    pub c: f64,
    /// cを掛けるgroup（`[5, 6, 7, 8]`）。
    pub c_groups: Vec<u8>,
    /// 天候の縮小の母数β（手段ごと）。
    pub beta: PerMode<f64>,
    pub weather_multiplier: WeatherMultiplier,
    /// urbanの推定の格子の大きさ・割合・件数のしきい値（14.2節）。
    #[serde(default)]
    pub urban: Value,
    /// 種別のまとめ方。
    #[serde(default)]
    pub pooling: Value,
    /// 推定の監査値。
    #[serde(default)]
    pub fit: Value,
}

/// `risk.bin` の中身。
#[derive(Debug, Clone, PartialEq)]
pub struct RiskModel {
    pub params: RiskParams,
    /// `PARAMS` のJSON全体（`params` に入らないキーも保つ）。
    pub raw_params: Value,
    /// `EDGEK`：辺の道路種別k。`EDGE_K_NO_RISK`・`EDGE_K_SIDEWALK` を含む。長さE。
    pub edge_k: Vec<u8>,
    /// `CLUSJ`：交差点クラスタの種別j。長さC。
    pub cluster_j: Vec<u8>,
    /// `KTAB`：辺の係数。添字は `(m×256 + k)×10 + cell`。長さ `TABLE_LEN`。
    pub k_table: Vec<CoefRecord>,
    /// `JTAB`：交差点の係数。添字は `(m×256 + j)×10 + cell`。長さ `TABLE_LEN`。
    pub j_table: Vec<CoefRecord>,
}

/// 手段の添字（0：walk、1：bike、2：car。`KTAB` の m）。
pub const fn mode_index(mode: Mode) -> usize {
    match mode {
        Mode::Walk => 0,
        Mode::Bike => 1,
        Mode::Car => 2,
    }
}

/// セル番号 `period_code×2 + day_type_code`（0〜9、形式書1章）。
pub const fn cell_index(period: Period, day_type: DayType) -> usize {
    period.code() as usize * 2 + day_type.code() as usize
}

/// `KTAB`・`JTAB` の添字 `(m×256 + 種別)×10 + cell`。
pub const fn table_index(mode: Mode, kind: u8, cell: usize) -> usize {
    (mode_index(mode) * 256 + kind as usize) * CELL_COUNT + cell
}

impl RiskModel {
    /// 辺の係数。
    pub fn k_coef(&self, mode: Mode, k: u8, cell: usize) -> &CoefRecord {
        &self.k_table[table_index(mode, k, cell)]
    }

    /// 交差点の係数。
    pub fn j_coef(&self, mode: Mode, j: u8, cell: usize) -> &CoefRecord {
        &self.j_table[table_index(mode, j, cell)]
    }
}

/// 道路種別k（形式書14.2節）：`k = group×8 + band×2 + urban`（group 0〜15、band 0〜3、urban 0〜1）。
pub const fn road_class_k(group: u8, band: u8, urban: bool) -> u8 {
    group * 8 + band * 2 + urban as u8
}

/// 交差点の種別j（形式書14.3節）：`j = top_group×8 + branch_band×2 + signal`。
/// `top_group` はクラスタの `top_class` を14.2節のgroupにしたもの（0〜9）。
pub const fn cluster_class_j(top_group: u8, branch_band: u8, signal: bool) -> u8 {
    top_group * 8 + branch_band * 2 + signal as u8
}

/// `branches`（クラスタに外から接する車道の辺の数）から `branch_band` へ（形式書14.3節）：
/// 3以下で0、4で1、5以上で2。
pub const fn branch_band(branches: u8) -> u8 {
    match branches {
        0..=3 => 0,
        4 => 1,
        _ => 2,
    }
}

/// 道路の種類（`HighwayClass::code`）から道路種別のgroup（形式書14.2節の表）。
/// 歩道・横断歩道の辺の扱い（`EDGE_K_SIDEWALK` など）は P6 が `flags` を見て決める。
/// 表にない種類（`code > 21`）は `None`。
pub const fn road_group(class: u8) -> Option<u8> {
    match class {
        0 | 1 => Some(0),    // motorway・motorway_link
        2 | 3 => Some(1),    // trunk・trunk_link
        4 | 5 => Some(2),    // primary・primary_link
        6 | 7 => Some(3),    // secondary・secondary_link
        8 | 9 => Some(4),    // tertiary・tertiary_link
        10 => Some(5),       // unclassified
        11 => Some(6),       // residential
        12 => Some(7),       // living_street
        13 => Some(8),       // service
        14 => Some(9),       // road
        15 => Some(10),      // track
        16 | 21 => Some(11), // path・bridleway
        17 => Some(12),      // cycleway
        18 => Some(13),      // footway
        19 => Some(14),      // pedestrian
        20 => Some(15),      // steps
        _ => None,
    }
}

/// 1リクエスト分のリスクの計算器（形式書14.5節）。探索で触れた辺・クラスタだけを計算して覚える。
/// 手段 `mode`、条件 `filter` の `Cset`（指定がallなら該当する全セル）と天候 `w` で固定する。
// 骨組み：フィールドは実装時に使う
#[allow(dead_code)]
pub struct RiskView<'a> {
    model: &'a RiskModel,
    graph: &'a Graph,
    topology: &'a Topology,
    accidents: &'a EdgeAccidentIndex,
    snapshot: &'a AccidentSnapshot,
    mode: Mode,
    filter: ConditionFilter,
    edge_cache: HashMap<EdgeId, f64>,
    cluster_cache: HashMap<ClusterId, f64>,
}

impl<'a> RiskView<'a> {
    pub fn new(
        model: &'a RiskModel,
        graph: &'a Graph,
        topology: &'a Topology,
        accidents: &'a EdgeAccidentIndex,
        snapshot: &'a AccidentSnapshot,
        mode: Mode,
        filter: ConditionFilter,
    ) -> Self {
        Self {
            model,
            graph,
            topology,
            accidents,
            snapshot,
            mode,
            filter,
            edge_cache: HashMap::new(),
            cluster_cache: HashMap::new(),
        }
    }

    /// 辺eを全長通るときのリスク `risk_e`（形式書14.5節）。歩道（`EDGE_K_SIDEWALK`）は親の車道の
    /// リスク密度に歩道の長さを掛ける。`EDGE_K_NO_RISK` は0。
    pub fn edge_risk(&mut self, e: EdgeId) -> f64 {
        todo!("形式書14.5節：辺のリスク")
    }

    /// 辺eの始点側から `from_dm`〜`to_dm` の部分だけを通るときのリスク（出発・到着の辺、14.5節）。
    /// `(1 − W)·μ_e·(通った長さ/ℓ) + W·(通った部分に位置がある事故の重みの和)` に深刻度と天候の項を掛ける。
    /// 辺を2つに分けても合計が変わらない。
    pub fn partial_edge_risk(&mut self, e: EdgeId, from_dm: u32, to_dm: u32) -> f64 {
        todo!("形式書14.5節：辺の一部のリスク")
    }

    /// 交差点クラスタcの `risk_v`（形式書14.5節）。「外から入る有向辺」を通るときに1回だけ加える。
    pub fn cluster_risk(&mut self, c: ClusterId) -> f64 {
        todo!("形式書14.5節：交差点のリスク")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::routing::graph::HighwayClass;

    #[test]
    fn k_and_j_formulas() {
        assert_eq!(road_class_k(0, 0, false), 0);
        assert_eq!(road_class_k(6, 2, true), 53);
        assert_eq!(road_class_k(15, 3, true), 127);
        assert_eq!(cluster_class_j(0, 0, false), 0);
        assert_eq!(cluster_class_j(9, 2, true), 77);
        // EDGE_K_* の特別値と衝突しない
        assert!(road_class_k(15, 3, true) < EDGE_K_SIDEWALK);
        assert_eq!(
            (
                branch_band(0),
                branch_band(3),
                branch_band(4),
                branch_band(5),
                branch_band(255)
            ),
            (0, 0, 1, 2, 2)
        );
    }

    #[test]
    fn groups_follow_the_table() {
        let group = |c: HighwayClass| road_group(c.code());
        assert_eq!(group(HighwayClass::MotorwayLink), Some(0));
        assert_eq!(group(HighwayClass::TrunkLink), Some(1));
        assert_eq!(group(HighwayClass::Unclassified), Some(5));
        assert_eq!(group(HighwayClass::Residential), Some(6));
        assert_eq!(group(HighwayClass::Service), Some(8));
        assert_eq!(group(HighwayClass::Bridleway), Some(11));
        assert_eq!(group(HighwayClass::Steps), Some(15));
        assert_eq!(road_group(22), None);
        // 22種類すべてがgroup 0〜15に入る
        assert!(
            HighwayClass::ALL
                .iter()
                .all(|c| road_group(c.code()).is_some_and(|g| g < 16))
        );
    }

    #[test]
    fn table_indexing() {
        assert_eq!(table_index(Mode::Walk, 0, 0), 0);
        assert_eq!(table_index(Mode::Bike, 0, 0), 256 * 10);
        assert_eq!(table_index(Mode::Car, 255, 9), TABLE_LEN - 1);
        assert_eq!(cell_index(Period::Night, DayType::Holiday), 7);
        assert_eq!(cell_index(Period::EarlyMorning, DayType::Weekday), 0);
    }
}
