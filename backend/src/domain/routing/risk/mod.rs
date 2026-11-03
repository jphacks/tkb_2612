//! リスクモデル（形式書14章）：係数表、辺・交差点の種別、リクエストごとのリスク（`RiskView`）。
//!
//! 事故の観測件数 `n_e` はファイルに持たない。リクエストごとに、`assignment.bin` の索引と
//! 事故の列から、探索で触れた辺だけ計算する。
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::contract::{ConditionFilter, DayType, Mode, Period, Weather};
use crate::domain::AccidentSnapshot;
use crate::domain::routing::accident::EdgeAccidentIndex;
use crate::domain::routing::graph::time::PerMode;
use crate::domain::routing::graph::{EdgeId, Graph};
use crate::domain::routing::topology::{ClusterId, Topology};

pub mod arc_cost;

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
///
/// `beta_cluster` は古いJSONにないことがあるので、`beta` と同じ値で補う
/// （`#[serde(default)]` で0にしない）。
#[derive(Debug, Clone, PartialEq, Serialize)]
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
    /// 交差点の天候の縮小の母数β_cluster（手段ごと）。
    pub beta_cluster: PerMode<f64>,
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

/// `RiskParams` のJSONを読むための補助。`beta_cluster` の欠落を `beta` で補う。
#[derive(Deserialize)]
struct RiskParamsJson {
    train_source_years: Vec<u16>,
    years: u32,
    c: f64,
    c_groups: Vec<u8>,
    beta: PerMode<f64>,
    #[serde(default)]
    beta_cluster: Option<PerMode<f64>>,
    weather_multiplier: WeatherMultiplier,
    #[serde(default)]
    urban: Value,
    #[serde(default)]
    pooling: Value,
    #[serde(default)]
    fit: Value,
}

impl<'de> Deserialize<'de> for RiskParams {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RiskParamsJson::deserialize(deserializer)?;
        let beta_cluster = raw.beta_cluster.unwrap_or_else(|| raw.beta.clone());
        Ok(RiskParams {
            train_source_years: raw.train_source_years,
            years: raw.years,
            c: raw.c,
            c_groups: raw.c_groups,
            beta: raw.beta,
            beta_cluster,
            weather_multiplier: raw.weather_multiplier,
            urban: raw.urban,
            pooling: raw.pooling,
            fit: raw.fit,
        })
    }
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

/// `Cset` のセルについて、係数表の行を足したもの。
#[derive(Debug, Clone, Copy, Default)]
struct CoefSums {
    /// `R = Σ_C r[C]`。
    r: f64,
    /// `S = Σ_C s[C]·r[C] / Σ_C r[C]`（`R=0` なら0）。
    s: f64,
    /// `ρ = Σ_C r[C]·rho[C][w-1] / Σ_C r[C]`（`R=0` なら0）。
    rho: f64,
}

/// 位置 `pos_dm` が区間 `[from_dm, to_dm)` に入るか。`to_dm` が辺の長さなら、終端
/// `pos_dm == length_dm` も含める（辺を分けたときに境界の事故を二重に数えないため）。
fn position_in_range(pos_dm: u32, from_dm: u32, to_dm: u32, length_dm: u32) -> bool {
    (from_dm..to_dm).contains(&pos_dm) || (to_dm == length_dm && pos_dm == length_dm)
}

/// 1リクエスト分のリスクの計算器（形式書14.5節）。探索で触れた辺・クラスタだけを計算して覚える。
/// 手段 `mode`、条件 `filter` の `Cset`（指定がallなら該当する全セル）と天候 `w` で固定する。
pub struct RiskView<'a> {
    model: &'a RiskModel,
    graph: &'a Graph,
    topology: &'a Topology,
    accidents: &'a EdgeAccidentIndex,
    snapshot: &'a AccidentSnapshot,
    mode: Mode,
    /// `Cset` のセル番号（昇順）。
    cells: Vec<usize>,
    /// セルが `Cset` に入るか（添字はセル番号）。
    cell_included: [bool; CELL_COUNT],
    /// 学習に使ったsource_year。
    train_years: HashSet<u16>,
    /// 雨・霧・雪のときの（天候コード、倍率M）。それ以外は `None`。
    weather_term: Option<(u8, f64)>,
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
        let periods: Vec<Period> = match filter.period {
            Some(period) => vec![period],
            None => Period::ALL.to_vec(),
        };
        let day_types: Vec<DayType> = match filter.day_type {
            Some(day_type) => vec![day_type],
            None => DayType::ALL.to_vec(),
        };
        let mut cells = Vec::with_capacity(periods.len() * day_types.len());
        let mut cell_included = [false; CELL_COUNT];
        for &period in &periods {
            for &day_type in &day_types {
                let cell = cell_index(period, day_type);
                cell_included[cell] = true;
                cells.push(cell);
            }
        }
        let weather_term = match filter.weather {
            Some(Weather::Rain) => {
                Some((Weather::Rain.code(), model.params.weather_multiplier.rain))
            }
            Some(Weather::Fog) => Some((Weather::Fog.code(), model.params.weather_multiplier.fog)),
            Some(Weather::Snow) => {
                Some((Weather::Snow.code(), model.params.weather_multiplier.snow))
            }
            Some(Weather::Clear | Weather::Cloudy) | None => None,
        };
        Self {
            model,
            graph,
            topology,
            accidents,
            snapshot,
            mode,
            cells,
            cell_included,
            train_years: model.params.train_source_years.iter().copied().collect(),
            weather_term,
            edge_cache: HashMap::new(),
            cluster_cache: HashMap::new(),
        }
    }

    /// この `RiskView` が参照しているグラフ。
    pub fn graph(&self) -> &'a Graph {
        self.graph
    }

    /// この `RiskView` が参照している位相情報。
    pub fn topology(&self) -> &'a Topology {
        self.topology
    }

    /// 辺eを全長通るときのリスク `risk_e`（形式書14.5節）。歩道（`EDGE_K_SIDEWALK`）は親の車道の
    /// リスク密度に歩道の長さを掛ける。`EDGE_K_NO_RISK` は0。
    pub fn edge_risk(&mut self, e: EdgeId) -> f64 {
        if let Some(&risk) = self.edge_cache.get(&e) {
            return risk;
        }
        let risk = self.compute_edge_risk(e);
        self.edge_cache.insert(e, risk);
        risk
    }

    /// 辺eの始点側から `from_dm`〜`to_dm` の部分だけを通るときのリスク（出発・到着の辺、14.5節）。
    /// `(1 − W)·μ_e·(通った長さ/ℓ) + W·(通った部分に位置がある事故の重みの和)` に深刻度と天候の項を掛ける。
    /// 辺を2つに分けても合計が変わらない。
    pub fn partial_edge_risk(&mut self, e: EdgeId, from_dm: u32, to_dm: u32) -> f64 {
        let length = self.graph.edge(e).length_dm;
        let from_dm = from_dm.min(length);
        let to_dm = to_dm.min(length);
        if from_dm >= to_dm {
            return 0.0;
        }
        let k = self.model.edge_k[e as usize];
        match k {
            EDGE_K_NO_RISK => 0.0,
            EDGE_K_SIDEWALK => self.sidewalk_partial_risk(e, from_dm, to_dm),
            _ => {
                let (n_e, n_weather) = self.edge_weights(e, from_dm, to_dm);
                let sums = self.coef_sums(&self.model.k_table, k);
                let a = self.coefficient_a(&self.model.k_table, k);
                let beta = *self.model.params.beta.get(self.mode);
                let travelled_m = f64::from(to_dm - from_dm) / 10.0;
                let risk = self.risk_value(sums, travelled_m, n_e, n_weather, a, beta);
                self.c_factor(k) * risk
            }
        }
    }

    /// 交差点クラスタcの `risk_v`（形式書14.5節）。「外から入る有向辺」を通るときに1回だけ加える。
    pub fn cluster_risk(&mut self, c: ClusterId) -> f64 {
        if let Some(&risk) = self.cluster_cache.get(&c) {
            return risk;
        }
        let risk = self.compute_cluster_risk(c);
        self.cluster_cache.insert(c, risk);
        risk
    }

    /// 事故がこのリクエストの条件（手段・セル・source_year）に合うか。
    fn assignment_matches(&self, accident: usize) -> bool {
        let columns = self.snapshot.columns();
        if accident >= columns.len() {
            return false;
        }
        if columns.modes[accident] & self.mode.mask() == 0 {
            return false;
        }
        let cell =
            usize::from(columns.period[accident]) * 2 + usize::from(columns.day_type[accident]);
        if !self.cell_included.get(cell).copied().unwrap_or(false) {
            return false;
        }
        self.train_years.contains(&columns.source_year[accident])
    }

    /// 事故の天候が、天候の項の対象（雨・霧・雪）と一致するか。
    fn weather_matches(&self, accident: usize) -> bool {
        self.weather_term
            .is_some_and(|(weather, _)| self.snapshot.columns().weather[accident] == weather)
    }

    /// 辺eの `[from_dm, to_dm)` にある割り当ての重みの和（`n_e` と、天候が一致する `n^w_e`）。
    fn edge_weights(&self, e: EdgeId, from_dm: u32, to_dm: u32) -> (f64, f64) {
        let length = self.graph.edge(e).length_dm;
        let mut n_e = 0.0;
        let mut n_weather = 0.0;
        for entry in self.accidents.edge_entries(e) {
            let accident = entry.accident as usize;
            if !self.assignment_matches(accident) {
                continue;
            }
            if !position_in_range(entry.pos_dm, from_dm, to_dm, length) {
                continue;
            }
            let weight = f64::from(entry.weight);
            n_e += weight;
            if self.weather_matches(accident) {
                n_weather += weight;
            }
        }
        (n_e, n_weather)
    }

    /// クラスタcへの割り当ての重みの和（`n_v` と、天候が一致する `n^w_v`）。
    fn cluster_weights(&self, c: ClusterId) -> (f64, f64) {
        let mut n_e = 0.0;
        let mut n_weather = 0.0;
        for entry in self.accidents.cluster_entries(c) {
            let accident = entry.accident as usize;
            if !self.assignment_matches(accident) {
                continue;
            }
            let weight = f64::from(entry.weight);
            n_e += weight;
            if self.weather_matches(accident) {
                n_weather += weight;
            }
        }
        (n_e, n_weather)
    }

    /// `Cset` のセルについて係数表の行を足す。
    fn coef_sums(&self, table: &[CoefRecord], kind: u8) -> CoefSums {
        let mut r = 0.0;
        let mut s_r = 0.0;
        let mut rho_r = 0.0;
        for &cell in &self.cells {
            let coef = &table[table_index(self.mode, kind, cell)];
            r += coef.r;
            s_r += coef.s * coef.r;
            if let Some((weather, _)) = self.weather_term {
                rho_r += coef.r * f64::from(coef.rho[usize::from(weather) - 1]);
            }
        }
        CoefSums {
            r,
            s: if r == 0.0 { 0.0 } else { s_r / r },
            rho: if r == 0.0 { 0.0 } else { rho_r / r },
        }
    }

    /// 係数表の `a`（同じ (m,種別) の10セルで同じ値）。
    fn coefficient_a(&self, table: &[CoefRecord], kind: u8) -> f64 {
        let cell = self.cells.first().copied().unwrap_or(0);
        table[table_index(self.mode, kind, cell)].a
    }

    /// 縮小と天候の項を込めた `S·Y`。辺は `travelled_m`、交差点は `travelled_m = 1` を渡す。
    #[allow(clippy::too_many_arguments)]
    fn risk_value(
        &self,
        sums: CoefSums,
        travelled_m: f64,
        n_e: f64,
        n_weather: f64,
        a: f64,
        beta: f64,
    ) -> f64 {
        let t = f64::from(self.model.params.years);
        let t_r = t * sums.r;
        let mu = t * travelled_m * sums.r;
        let denominator = t_r + a;
        let w = if denominator == 0.0 {
            0.0
        } else {
            t_r / denominator
        };
        let mut y = (1.0 - w) * mu + w * n_e;
        if let Some((_, multiplier)) = self.weather_term {
            let t_r_rho = t_r * sums.rho;
            let denominator_weather = t_r_rho + beta;
            let w_weather = if denominator_weather == 0.0 {
                0.0
            } else {
                t_r_rho / denominator_weather
            };
            let y_weather = (1.0 - w_weather) * (mu * sums.rho) + w_weather * n_weather;
            y += (multiplier - 1.0) * y_weather;
        }
        sums.s * y
    }

    /// 自動車の生活道路の係数 `c_k`（形式書14.5節）。
    fn c_factor(&self, k: u8) -> f64 {
        if self.mode == Mode::Car
            && k != EDGE_K_NO_RISK
            && k != EDGE_K_SIDEWALK
            && self.model.params.c_groups.contains(&(k / 8))
        {
            self.model.params.c
        } else {
            1.0
        }
    }

    /// 辺eの全長のリスク（キャッシュへの出し入れは `edge_risk`）。
    fn compute_edge_risk(&mut self, e: EdgeId) -> f64 {
        let k = self.model.edge_k[e as usize];
        match k {
            EDGE_K_NO_RISK => 0.0,
            EDGE_K_SIDEWALK => self.sidewalk_risk(e),
            _ => {
                let length = self.graph.edge(e).length_dm;
                let (n_e, n_weather) = self.edge_weights(e, 0, length);
                let sums = self.coef_sums(&self.model.k_table, k);
                let a = self.coefficient_a(&self.model.k_table, k);
                let beta = *self.model.params.beta.get(self.mode);
                let risk = self.risk_value(sums, f64::from(length) / 10.0, n_e, n_weather, a, beta);
                self.c_factor(k) * risk
            }
        }
    }

    /// 歩道の全長のリスク：親の車道のリスク密度 × 歩道の長さ。親がなければ0。
    fn sidewalk_risk(&mut self, e: EdgeId) -> f64 {
        let Some(parent) = self.topology.sidewalk_parent(e) else {
            return 0.0;
        };
        let parent_k = self.model.edge_k[parent as usize];
        if parent_k == EDGE_K_NO_RISK || parent_k == EDGE_K_SIDEWALK {
            return 0.0;
        }
        let parent_risk = self.edge_risk(parent);
        let parent_m = f64::from(self.graph.edge(parent).length_dm) / 10.0;
        let edge_m = f64::from(self.graph.edge(e).length_dm) / 10.0;
        if parent_m == 0.0 {
            0.0
        } else {
            parent_risk / parent_m * edge_m
        }
    }

    /// 歩道の部分のリスク：親の車道のリスク密度 × 通った長さ。親がなければ0。
    fn sidewalk_partial_risk(&mut self, e: EdgeId, from_dm: u32, to_dm: u32) -> f64 {
        let Some(parent) = self.topology.sidewalk_parent(e) else {
            return 0.0;
        };
        let parent_k = self.model.edge_k[parent as usize];
        if parent_k == EDGE_K_NO_RISK || parent_k == EDGE_K_SIDEWALK {
            return 0.0;
        }
        let parent_risk = self.edge_risk(parent);
        let parent_m = f64::from(self.graph.edge(parent).length_dm) / 10.0;
        let travelled_m = f64::from(to_dm - from_dm) / 10.0;
        if parent_m == 0.0 {
            0.0
        } else {
            parent_risk / parent_m * travelled_m
        }
    }

    /// クラスタのリスク（キャッシュへの出し入れは `cluster_risk`）。
    fn compute_cluster_risk(&self, c: ClusterId) -> f64 {
        let j = self.model.cluster_j[c as usize];
        let (n_e, n_weather) = self.cluster_weights(c);
        let sums = self.coef_sums(&self.model.j_table, j);
        let a = self.coefficient_a(&self.model.j_table, j);
        let beta = *self.model.params.beta_cluster.get(self.mode);
        self.risk_value(sums, 1.0, n_e, n_weather, a, beta)
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
