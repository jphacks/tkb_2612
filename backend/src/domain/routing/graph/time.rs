//! 所要時間のモデル（形式書8章）。
//!
//! 係数は `graph.bin` の `TIMEMODL`（JSON）にデータとして持つ。時間の単位はデシ秒（0.1秒）。
//! ノードの加算と右左折コストは0以上なので、辺の時間だけの最短時間は本当の最短時間の下界になる
//! （楕円の切り出しとALTの距離表が使う）。
// 骨組み：実装時に外す
#![allow(unused_variables)]

use serde::{Deserialize, Serialize};

use crate::contract::Mode;

use crate::domain::routing::graph::{DirEdge, Graph, HighwayClass, NodeId};

/// 手段ごとに同じ型の値を持つ組（`{"walk": .., "bike": .., "car": ..}`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerMode<T> {
    pub walk: T,
    pub bike: T,
    pub car: T,
}

impl<T> PerMode<T> {
    pub fn get(&self, mode: Mode) -> &T {
        match mode {
            Mode::Walk => &self.walk,
            Mode::Bike => &self.bike,
            Mode::Car => &self.car,
        }
    }
}

/// 角度による右左折コスト（秒、形式書8章の `turn`）。
///
/// 進行方向の変化角が `straight_max_deg` 以下なら直進（0秒）、`u_turn_min_deg` 以上ならUターン、
/// それ以外は右（時計回り）か左。左側通行なので `right_s ≥ left_s`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnParams {
    pub right_s: f64,
    pub left_s: f64,
    pub u_turn_s: f64,
    pub straight_max_deg: f64,
    pub u_turn_min_deg: f64,
}

/// `TIMEMODL` 節（形式書8章）。未知のキーは読込時に失敗させる。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimeModel {
    /// モデルの版（例 `jp-time-1`）。
    pub version: String,
    /// 種類（`HighwayClass::code` を添字）ごとの既定速度（km/h）。通行不可の種類は0。
    pub speed_kmh: PerMode<[f64; HighwayClass::COUNT]>,
    /// carで `maxspeed_kmh` があるときの速度 = `min(maxspeed_kmh × factor, speed_kmh.car[class] × cap_ratio)`。
    pub car_maxspeed_factor: f64,
    /// 上の上限の倍率。
    pub car_maxspeed_cap_ratio: f64,
    /// 信号のノードを通過するときの加算（秒）。
    pub signal_penalty_s: PerMode<f64>,
    /// 一時停止のノードの加算（秒）。
    pub stop_penalty_s: PerMode<f64>,
    /// 踏切のノードの加算（秒）。
    pub level_crossing_penalty_s: PerMode<f64>,
    /// walkで階段の辺の時間に掛ける倍率（1以上）。
    pub steps_factor_walk: f64,
    /// 角度による右左折コスト。
    pub turn: PerMode<TurnParams>,
}

impl TimeModel {
    /// JSONから読む。未知のキーや欠けたキーは失敗する。
    pub fn from_json(value: &serde_json::Value) -> Result<TimeModel, serde_json::Error> {
        serde_json::from_value(value.clone())
    }

    /// 合成グラフのテスト用の単純なモデル（全種類で walk 4.8km/h・bike 15km/h・car 30km/h、
    /// 加算とターンコストは0）。releaseの値（形式書8章の初版の値）ではない。
    pub fn synthetic() -> TimeModel {
        let zero = PerMode {
            walk: 0.0,
            bike: 0.0,
            car: 0.0,
        };
        let turn = TurnParams {
            right_s: 0.0,
            left_s: 0.0,
            u_turn_s: 0.0,
            straight_max_deg: 30.0,
            u_turn_min_deg: 150.0,
        };
        TimeModel {
            version: "synthetic".to_owned(),
            speed_kmh: PerMode {
                walk: [4.8; HighwayClass::COUNT],
                bike: [15.0; HighwayClass::COUNT],
                car: [30.0; HighwayClass::COUNT],
            },
            car_maxspeed_factor: 1.0,
            car_maxspeed_cap_ratio: 1.0,
            signal_penalty_s: zero.clone(),
            stop_penalty_s: zero.clone(),
            level_crossing_penalty_s: zero,
            steps_factor_walk: 1.0,
            turn: PerMode {
                walk: turn.clone(),
                bike: turn.clone(),
                car: turn,
            },
        }
    }
}

/// 辺の時間（デシ秒、形式書8章）：`length_dm / 10 ÷ (速度km/h ÷ 3.6)` 秒を10倍して切り上げた整数。
/// walkの階段は `steps_factor_walk` 倍。通れない有向辺（`Graph::passable` が偽）は `None`。
pub fn edge_time_ds(graph: &Graph, d: DirEdge, mode: Mode) -> Option<u32> {
    todo!("形式書8章：辺の時間")
}

/// 辺の一部（始点側から `travelled_dm` の長さ）だけを通るときの時間。出発・到着の辺で使う。
/// 通った割合を掛けて切り上げる（形式書8章）。
pub fn partial_edge_time_ds(
    graph: &Graph,
    d: DirEdge,
    mode: Mode,
    travelled_dm: u32,
) -> Option<u32> {
    todo!("形式書8章：出発・到着の辺の時間")
}

/// ノードの加算（デシ秒、形式書8章）。経路の途中のノードを通過するときに、信号・一時停止・踏切の
/// 加算の最大値を足す。出発・到着の辺の端点には呼び出し側が加えない。
pub fn node_penalty_ds(graph: &Graph, node: NodeId, mode: Mode) -> u32 {
    todo!("形式書8章：ノードの加算")
}

/// 右左折コスト（デシ秒、形式書8章）。入る有向辺の最後の線分と出る有向辺の最初の線分の方位の差から決める。
pub fn turn_cost_ds(graph: &Graph, in_dedge: DirEdge, out_dedge: DirEdge, mode: Mode) -> u32 {
    todo!("形式書8章：右左折コスト")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_model_round_trips_through_json() {
        let model = TimeModel::synthetic();
        let json = serde_json::to_value(&model).unwrap();
        assert_eq!(TimeModel::from_json(&json).unwrap(), model);
    }

    #[test]
    fn unknown_or_missing_keys_are_rejected() {
        let mut json = serde_json::to_value(TimeModel::synthetic()).unwrap();
        json["unexpected"] = 1.into();
        assert!(TimeModel::from_json(&json).is_err());

        let mut json = serde_json::to_value(TimeModel::synthetic()).unwrap();
        json["turn"]["car"]["extra"] = 1.into();
        assert!(TimeModel::from_json(&json).is_err());

        let mut json = serde_json::to_value(TimeModel::synthetic()).unwrap();
        json.as_object_mut().unwrap().remove("steps_factor_walk");
        assert!(TimeModel::from_json(&json).is_err());

        // speed_kmh の配列は22要素
        let mut json = serde_json::to_value(TimeModel::synthetic()).unwrap();
        json["speed_kmh"]["car"] = serde_json::json!([1.0, 2.0]);
        assert!(TimeModel::from_json(&json).is_err());
    }
}
