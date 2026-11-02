//! 所要時間のモデル（形式書8章）。
//!
//! 係数は `graph.bin` の `TIMEMODL`（JSON）にデータとして持つ。時間の単位はデシ秒（0.1秒）。
//! ノードの加算と右左折コストは0以上なので、辺の時間だけの最短時間は本当の最短時間の下界になる
//! （楕円の切り出しとALTの距離表が使う）。

use std::f64::consts::PI;

use serde::{Deserialize, Serialize};

use crate::contract::Mode;

use crate::domain::routing::graph::{
    Coord, DirEdge, EdgeId, Graph, HighwayClass, NodeId, node_flags,
};

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

impl TimeModel {
    /// 値の検査（`Graph::from_parts` が呼ぶ）。速度・加算・右左折コストは有限で0以上、
    /// `car_maxspeed_*` は正、`steps_factor_walk` は1以上、角度のしきい値は
    /// `0 ≤ straight_max_deg < u_turn_min_deg ≤ 180`。
    /// 加算とターンコストが0以上であることは、辺の時間だけの最短時間が下界になる前提（形式書8章）。
    pub fn validate(&self) -> Result<(), String> {
        for mode in Mode::ALL {
            let name = mode.as_str();
            for (class, &v) in self.speed_kmh.get(mode).iter().enumerate() {
                if !(v.is_finite() && v >= 0.0) {
                    return Err(format!(
                        "speed_kmh.{name}[{class}]={v}が有限の0以上ではありません"
                    ));
                }
            }
            let penalties = [
                ("signal_penalty_s", *self.signal_penalty_s.get(mode)),
                ("stop_penalty_s", *self.stop_penalty_s.get(mode)),
                (
                    "level_crossing_penalty_s",
                    *self.level_crossing_penalty_s.get(mode),
                ),
            ];
            for (key, v) in penalties {
                if !(v.is_finite() && v >= 0.0) {
                    return Err(format!("{key}.{name}={v}が有限の0以上ではありません"));
                }
            }
            let t = self.turn.get(mode);
            for (key, v) in [
                ("right_s", t.right_s),
                ("left_s", t.left_s),
                ("u_turn_s", t.u_turn_s),
            ] {
                if !(v.is_finite() && v >= 0.0) {
                    return Err(format!("turn.{name}.{key}={v}が有限の0以上ではありません"));
                }
            }
            let angles_ok = t.straight_max_deg.is_finite()
                && t.u_turn_min_deg.is_finite()
                && t.straight_max_deg >= 0.0
                && t.straight_max_deg < t.u_turn_min_deg
                && t.u_turn_min_deg <= 180.0;
            if !angles_ok {
                return Err(format!(
                    "turn.{name}の角度（straight_max_deg={}、u_turn_min_deg={}）が0 ≤ 直進 < Uターン ≤ 180ではありません",
                    t.straight_max_deg, t.u_turn_min_deg
                ));
            }
        }
        for (key, v) in [
            ("car_maxspeed_factor", self.car_maxspeed_factor),
            ("car_maxspeed_cap_ratio", self.car_maxspeed_cap_ratio),
        ] {
            if !(v.is_finite() && v > 0.0) {
                return Err(format!("{key}={v}が有限の正の数ではありません"));
            }
        }
        if !(self.steps_factor_walk.is_finite() && self.steps_factor_walk >= 1.0) {
            return Err(format!(
                "steps_factor_walk={}が1以上の有限の数ではありません",
                self.steps_factor_walk
            ));
        }
        Ok(())
    }
}

/// 速度（km/h）を1/1000 km/h単位の整数にする。0以下・非有限は `None`（通れない）。
/// 速度の浮動小数点の誤差（`maxspeed × 係数` など）が `ceil` の結果を1ずらさないように、
/// 時間の計算は整数で行う。
fn speed_milli(kmh: f64) -> Option<u64> {
    if !kmh.is_finite() {
        return None;
    }
    let milli = (kmh * 1000.0).round();
    (milli >= 1.0).then_some(milli as u64)
}

/// 手段・道路の種類・`maxspeed` から決まる速度（1/1000 km/h）。モデルが0とする組は `None`。
fn edge_speed_milli(graph: &Graph, d: DirEdge, mode: Mode) -> Option<u64> {
    let record = graph.edge(d.edge());
    let model = graph.time_model();
    let base = *model.speed_kmh.get(mode).get(usize::from(record.class))?;
    let kmh = if mode == Mode::Car && record.maxspeed_kmh > 0 {
        (f64::from(record.maxspeed_kmh) * model.car_maxspeed_factor)
            .min(base * model.car_maxspeed_cap_ratio)
    } else {
        base
    };
    speed_milli(kmh)
}

/// 距離（デシメートル）を通るときの時間（デシ秒、切り上げ）。
/// `dist_dm / 10 ÷ (speed / 3.6) × 10 × factor = dist_dm × 3.6 × factor / speed`。
fn travel_ds(dist_dm: u32, speed_milli: u64, factor: f64) -> u32 {
    let factor_milli = (factor * 1000.0).round().max(1.0) as u128;
    let numerator = u128::from(dist_dm) * 3600 * factor_milli;
    let denominator = u128::from(speed_milli) * 1000;
    u32::try_from(numerator.div_ceil(denominator)).unwrap_or(u32::MAX)
}

/// walkの階段の辺の倍率。それ以外は1。
fn time_factor(graph: &Graph, d: DirEdge, mode: Mode) -> f64 {
    let is_steps = graph.edge(d.edge()).class == HighwayClass::Steps.code();
    if mode == Mode::Walk && is_steps {
        graph.time_model().steps_factor_walk
    } else {
        1.0
    }
}

/// 辺の時間（デシ秒、形式書8章）：`length_dm / 10 ÷ (速度km/h ÷ 3.6)` 秒を10倍して切り上げた整数。
/// walkの階段は `steps_factor_walk` 倍。通れない有向辺（`Graph::passable` が偽）は `None`。
///
/// 速度はcarで `maxspeed_kmh > 0` なら `min(maxspeed × car_maxspeed_factor, 既定速度 × car_maxspeed_cap_ratio)`。
/// モデルの速度が0（通行不可の種類）の組も `None` にする（`access` が通すと言っていても、時間が決まらない）。
/// 速度は1/1000 km/h単位に丸め、時間は整数で計算する（浮動小数点の誤差で切り上げが1ずれない）。
/// 結果が `u32` に収まらないときは `u32::MAX`。
pub fn edge_time_ds(graph: &Graph, d: DirEdge, mode: Mode) -> Option<u32> {
    if !graph.passable(d, mode) {
        return None;
    }
    let speed = edge_speed_milli(graph, d, mode)?;
    let length_dm = graph.edge(d.edge()).length_dm;
    Some(travel_ds(length_dm, speed, time_factor(graph, d, mode)))
}

/// 辺の一部（始点側から `travelled_dm` の長さ）だけを通るときの時間。出発・到着の辺で使う。
/// 通った割合を掛けて切り上げる（形式書8章）：`travelled_dm` を辺の長さとして `edge_time_ds` と
/// 同じ式で計算するので、全長なら `edge_time_ds` と等しく、0なら0。辺の長さを超える分は辺の長さに切る。
/// 通れない有向辺は `None`。
pub fn partial_edge_time_ds(
    graph: &Graph,
    d: DirEdge,
    mode: Mode,
    travelled_dm: u32,
) -> Option<u32> {
    if !graph.passable(d, mode) {
        return None;
    }
    let speed = edge_speed_milli(graph, d, mode)?;
    let travelled = travelled_dm.min(graph.edge(d.edge()).length_dm);
    Some(travel_ds(travelled, speed, time_factor(graph, d, mode)))
}

/// 秒をデシ秒（四捨五入）にする。
fn seconds_to_ds(seconds: f64) -> u32 {
    (seconds * 10.0).round() as u32
}

/// ノードの加算（デシ秒、形式書8章）。経路の途中のノードを通過するときに、信号・一時停止・踏切の
/// 加算の最大値を足す。出発・到着の辺の端点には呼び出し側が加えない。
pub fn node_penalty_ds(graph: &Graph, node: NodeId, mode: Mode) -> u32 {
    let flags = graph.node_flags(node);
    let model = graph.time_model();
    let mut seconds = 0.0_f64;
    if flags & node_flags::SIGNAL != 0 {
        seconds = seconds.max(*model.signal_penalty_s.get(mode));
    }
    if flags & node_flags::STOP != 0 {
        seconds = seconds.max(*model.stop_penalty_s.get(mode));
    }
    if flags & node_flags::LEVEL_CROSSING != 0 {
        seconds = seconds.max(*model.level_crossing_penalty_s.get(mode));
    }
    seconds_to_ds(seconds)
}

/// 辺の折れ線を始点→終点の順にたどる（割り当てなし）。
fn polyline_points(graph: &Graph, e: EdgeId) -> impl Iterator<Item = Coord> + '_ {
    let parts = graph.parts();
    let record = &parts.edges[e as usize];
    let start = parts.shape_ofs[e as usize] as usize;
    let end = parts.shape_ofs[e as usize + 1] as usize;
    let first = parts.node_ll[record.tail as usize];
    let head = parts.node_ll[record.head as usize];
    let mut current = first;
    std::iter::once(first)
        .chain(parts.shape[start..end].iter().map(move |p| {
            current = Coord::new(
                current.lat_e7 + i32::from(p.dlat_e7),
                current.lon_e7 + i32::from(p.dlon_e7),
            );
            current
        }))
        .chain(std::iter::once(head))
}

/// 辺の始点→終点の向きで見た、最初の長さのある線分。
fn first_segment(graph: &Graph, e: EdgeId) -> Option<(Coord, Coord)> {
    let mut points = polyline_points(graph, e);
    let mut prev = points.next()?;
    for p in points {
        if p != prev {
            return Some((prev, p));
        }
        prev = p;
    }
    None
}

/// 辺の始点→終点の向きで見た、最後の長さのある線分。
fn last_segment(graph: &Graph, e: EdgeId) -> Option<(Coord, Coord)> {
    let mut points = polyline_points(graph, e);
    let mut prev = points.next()?;
    let mut found = None;
    for p in points {
        if p != prev {
            found = Some((prev, p));
        }
        prev = p;
    }
    found
}

/// 有向辺の進行方向で見た最初の線分（長さ0の線分は飛ばす）。
fn dedge_first_segment(graph: &Graph, d: DirEdge) -> Option<(Coord, Coord)> {
    if d.is_reverse() {
        last_segment(graph, d.edge()).map(|(a, b)| (b, a))
    } else {
        first_segment(graph, d.edge())
    }
}

/// 有向辺の進行方向で見た最後の線分（長さ0の線分は飛ばす）。
fn dedge_last_segment(graph: &Graph, d: DirEdge) -> Option<(Coord, Coord)> {
    if d.is_reverse() {
        first_segment(graph, d.edge()).map(|(a, b)| (b, a))
    } else {
        last_segment(graph, d.edge())
    }
}

/// 入る有向辺の最後の線分から、出る有向辺の最初の線分への進行方向の変化角（度、-180より大きく180以下）。
/// 時計回り（右折）が正。局所平面近似（経度の差に、入る有向辺の到着ノードの緯度の余弦を掛ける。
/// `contract::geo` と同じ近似）で計算する。どちらかの辺が長さ0の線分しか持たず向きが決まらないときは `None`。
pub fn turn_angle_deg(graph: &Graph, in_dedge: DirEdge, out_dedge: DirEdge) -> Option<f64> {
    let (a0, a1) = dedge_last_segment(graph, in_dedge)?;
    let (b0, b1) = dedge_first_segment(graph, out_dedge)?;
    let node = graph.node_coord(graph.dedge_to(in_dedge));
    let cos_lat = (f64::from(node.lat_e7) / 1e7 * PI / 180.0).cos();
    let vector = |from: Coord, to: Coord| {
        (
            (f64::from(to.lon_e7) - f64::from(from.lon_e7)) * cos_lat,
            f64::from(to.lat_e7) - f64::from(from.lat_e7),
        )
    };
    let (x1, y1) = vector(a0, a1);
    let (x2, y2) = vector(b0, b1);
    // 東をx、北をyとして、入る向きから出る向きへの時計回りの角度。
    let clockwise = (y1 * x2 - x1 * y2).atan2(x1 * x2 + y1 * y2);
    Some(clockwise * 180.0 / PI)
}

/// 角度のしきい値の比較の許容（度）。ちょうど境界の角度が浮動小数点の誤差で振り分けを変えない。
const ANGLE_EPS_DEG: f64 = 1e-9;

/// 右左折コスト（デシ秒、形式書8章）。入る有向辺の最後の線分と出る有向辺の最初の線分の方位の差から決める。
///
/// 変化角の絶対値が `straight_max_deg` 以下なら直進（0）、`u_turn_min_deg` 以上ならUターン（`u_turn_s`）、
/// それ以外は時計回りなら `right_s`、反時計回りなら `left_s`。同じ辺を引き返す（`out == in.reversed()`）
/// のは、角度によらずUターン。向きが決まらない（長さ0の線分だけの辺）ときは0。
pub fn turn_cost_ds(graph: &Graph, in_dedge: DirEdge, out_dedge: DirEdge, mode: Mode) -> u32 {
    let params = graph.time_model().turn.get(mode);
    let seconds = if out_dedge == in_dedge.reversed() {
        params.u_turn_s
    } else {
        match turn_angle_deg(graph, in_dedge, out_dedge) {
            None => 0.0,
            Some(angle) => {
                let magnitude = angle.abs();
                if magnitude <= params.straight_max_deg + ANGLE_EPS_DEG {
                    0.0
                } else if magnitude >= params.u_turn_min_deg - ANGLE_EPS_DEG {
                    params.u_turn_s
                } else if angle > 0.0 {
                    params.right_s
                } else {
                    params.left_s
                }
            }
        }
    };
    seconds_to_ds(seconds)
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

    // ---- 時間の関数（形式書8章） ----

    use crate::domain::routing::graph::synthetic::{EdgeSpec, GraphBuilder, grid};
    use crate::domain::routing::graph::node_flags;

    const ALL_CLASSES: usize = HighwayClass::COUNT;

    /// 100 m（1000 dm）の辺が1本の、2ノードのグラフ。
    fn single_edge(spec: impl FnOnce(EdgeSpec) -> EdgeSpec, model: TimeModel) -> (Graph, DirEdge) {
        let mut b = GraphBuilder::new();
        let a = b.add_node_deg(35.0, 139.0);
        let c = b.add_node_deg(35.0, 139.001);
        b.add_edge(spec(EdgeSpec::new(a, c).length_dm(1000)));
        b.set_time_model(model);
        let graph = b.build();
        (graph, DirEdge::new(0, false))
    }

    #[test]
    fn edge_times_match_hand_computed_values() {
        // 100 m を 30km/h（8.333 m/s）で12秒、15km/hで24秒、4.8km/h（80 m/分）で75秒。
        let (g, d) = single_edge(|e| e, TimeModel::synthetic());
        assert_eq!(edge_time_ds(&g, d, Mode::Car), Some(120));
        assert_eq!(edge_time_ds(&g, d, Mode::Bike), Some(240));
        assert_eq!(edge_time_ds(&g, d, Mode::Walk), Some(750));
        assert_eq!(edge_time_ds(&g, d.reversed(), Mode::Car), Some(120));
    }

    #[test]
    fn edge_times_are_rounded_up_without_float_noise() {
        let cases = [
            // (length_dm, mode, 期待値)
            (1000, Mode::Car, 120),
            (1001, Mode::Car, 121), // 120.12
            (1, Mode::Car, 1),      // 0.12
            (25, Mode::Car, 3),     // 25 × 3.6 / 30 = 3 ちょうど
            (40, Mode::Walk, 30), // 40 × 3.6 / 4.8 = 30 ちょうど（浮動小数点では30.000000000000004）
            (1000, Mode::Bike, 240),
            (7, Mode::Bike, 2), // 1.68
        ];
        for (length_dm, mode, want) in cases {
            let (g, d) = single_edge(|e| e.length_dm(length_dm), TimeModel::synthetic());
            assert_eq!(
                edge_time_ds(&g, d, mode),
                Some(want),
                "{length_dm} {mode:?}"
            );
        }
    }

    #[test]
    fn dedges_that_cannot_be_traversed_have_no_time() {
        let (g, fwd) = single_edge(|e| e.oneway().deny(Mode::Walk), TimeModel::synthetic());
        let bwd = fwd.reversed();
        assert_eq!(edge_time_ds(&g, fwd, Mode::Car), Some(120));
        assert_eq!(edge_time_ds(&g, bwd, Mode::Car), None);
        assert_eq!(edge_time_ds(&g, bwd, Mode::Bike), None);
        assert_eq!(edge_time_ds(&g, fwd, Mode::Walk), None);
        assert_eq!(edge_time_ds(&g, bwd, Mode::Walk), None);
        for mode in Mode::ALL {
            assert_eq!(
                partial_edge_time_ds(&g, bwd, mode, 10).is_some(),
                g.passable(bwd, mode) && edge_time_ds(&g, bwd, mode).is_some()
            );
        }
        assert_eq!(partial_edge_time_ds(&g, bwd, Mode::Car, 10), None);
    }

    #[test]
    fn a_zero_speed_class_is_not_routable_even_if_access_allows_it() {
        let mut model = TimeModel::synthetic();
        model.speed_kmh.car[HighwayClass::Track.code() as usize] = 0.0;
        let (g, d) = single_edge(|e| e.class(HighwayClass::Track), model);
        assert!(g.passable(d, Mode::Car));
        assert_eq!(edge_time_ds(&g, d, Mode::Car), None);
        assert_eq!(partial_edge_time_ds(&g, d, Mode::Car, 500), None);
        assert_eq!(edge_time_ds(&g, d, Mode::Bike), Some(240));
    }

    #[test]
    fn car_maxspeed_rule() {
        let mut model = TimeModel::synthetic();
        model.car_maxspeed_factor = 0.9;
        model.car_maxspeed_cap_ratio = 1.2; // 上限 30 × 1.2 = 36 km/h
        let cases = [
            (0, 120),   // maxspeedなし：既定の30km/h
            (40, 100),  // min(36, 36)。0.9 × 40 は浮動小数点で36ちょうどでなくてもよい
            (30, 134),  // 27km/h → 133.33
            (20, 200),  // 18km/h
            (255, 100), // 上限36km/h
            (1, 4000),  // 0.9km/h は 1000×3.6/0.9
        ];
        for (maxspeed, want) in cases {
            let (g, d) = single_edge(|e| e.maxspeed(maxspeed), model.clone());
            assert_eq!(
                edge_time_ds(&g, d, Mode::Car),
                Some(want),
                "maxspeed={maxspeed}"
            );
            // bikeとwalkは maxspeed を使わない。
            assert_eq!(edge_time_ds(&g, d, Mode::Bike), Some(240));
            assert_eq!(edge_time_ds(&g, d, Mode::Walk), Some(750));
        }
    }

    #[test]
    fn walking_up_steps_is_multiplied() {
        let mut model = TimeModel::synthetic();
        model.steps_factor_walk = 2.5;
        let (g, d) = single_edge(|e| e.class(HighwayClass::Steps), model.clone());
        assert_eq!(edge_time_ds(&g, d, Mode::Walk), Some(1875)); // 750 × 2.5
        assert_eq!(edge_time_ds(&g, d, Mode::Bike), Some(240));
        assert_eq!(edge_time_ds(&g, d, Mode::Car), Some(120));
        // 階段でない種類は倍率なし。
        let (g, d) = single_edge(|e| e.class(HighwayClass::Footway), model.clone());
        assert_eq!(edge_time_ds(&g, d, Mode::Walk), Some(750));
        // 倍率をかけてから切り上げる：1 dm = 0.75秒 × 2.5 = 1.875 → 2
        let (g, d) = single_edge(|e| e.class(HighwayClass::Steps).length_dm(1), model);
        assert_eq!(edge_time_ds(&g, d, Mode::Walk), Some(2));
    }

    #[test]
    fn partial_edge_time_scales_by_the_travelled_length() {
        let (g, d) = single_edge(|e| e, TimeModel::synthetic());
        // 250 dm（25 m）：3秒
        assert_eq!(partial_edge_time_ds(&g, d, Mode::Car, 250), Some(30));
        assert_eq!(partial_edge_time_ds(&g, d, Mode::Car, 0), Some(0));
        // 全長ならedge_time_dsと等しい。超える分は辺の長さに切る。
        assert_eq!(partial_edge_time_ds(&g, d, Mode::Car, 1000), Some(120));
        assert_eq!(partial_edge_time_ds(&g, d, Mode::Car, 5000), Some(120));
        // 切り上げ：1 dm は0.12デシ秒ぶんだが1。
        assert_eq!(partial_edge_time_ds(&g, d, Mode::Car, 1), Some(1));
        assert_eq!(partial_edge_time_ds(&g, d, Mode::Walk, 500), Some(375));
        // 向きによらない。
        assert_eq!(
            partial_edge_time_ds(&g, d.reversed(), Mode::Car, 250),
            Some(30)
        );
        // 階段の倍率も掛かる。
        let mut model = TimeModel::synthetic();
        model.steps_factor_walk = 3.0;
        let (g, d) = single_edge(|e| e.class(HighwayClass::Steps), model);
        assert_eq!(partial_edge_time_ds(&g, d, Mode::Walk, 500), Some(1125));
        // 2つに分けた時間の和は、全体以上（切り上げの分）。
        let whole = edge_time_ds(&g, d, Mode::Walk).unwrap();
        let split = partial_edge_time_ds(&g, d, Mode::Walk, 333).unwrap()
            + partial_edge_time_ds(&g, d, Mode::Walk, 667).unwrap();
        assert!(split >= whole && split <= whole + 1, "{split} {whole}");
    }

    #[test]
    fn node_penalty_adds_the_largest_applicable_penalty() {
        let mut model = TimeModel::synthetic();
        model.signal_penalty_s = PerMode {
            walk: 7.5,
            bike: 10.0,
            car: 20.0,
        };
        model.stop_penalty_s = PerMode {
            walk: 1.0,
            bike: 5.0,
            car: 5.0,
        };
        model.level_crossing_penalty_s = PerMode {
            walk: 2.0,
            bike: 2.0,
            car: 30.0,
        };
        let mut b = GraphBuilder::new();
        let flags = [
            0,
            node_flags::SIGNAL,
            node_flags::STOP,
            node_flags::LEVEL_CROSSING,
            node_flags::SIGNAL | node_flags::STOP,
            node_flags::SIGNAL | node_flags::STOP | node_flags::LEVEL_CROSSING,
            node_flags::CROSSING | node_flags::BLOCK_CAR, // 加算に関係しない
        ];
        let nodes: Vec<NodeId> = flags
            .iter()
            .map(|&f| {
                let n = b.add_node_deg(35.0, 139.0 + 0.001 * f64::from(f));
                b.add_node_flags(n, f);
                n
            })
            .collect();
        b.set_time_model(model);
        let g = b.build();
        let at = |i: usize, mode| node_penalty_ds(&g, nodes[i], mode);
        assert_eq!([at(0, Mode::Car), at(0, Mode::Walk)], [0, 0]);
        assert_eq!(
            [at(1, Mode::Walk), at(1, Mode::Bike), at(1, Mode::Car)],
            [75, 100, 200]
        );
        assert_eq!(
            [at(2, Mode::Walk), at(2, Mode::Bike), at(2, Mode::Car)],
            [10, 50, 50]
        );
        assert_eq!(at(3, Mode::Car), 300);
        assert_eq!(at(4, Mode::Car), 200); // 加算ではなく最大
        assert_eq!(at(4, Mode::Walk), 75);
        assert_eq!(at(5, Mode::Car), 300);
        assert_eq!(at(5, Mode::Bike), 100);
        assert_eq!(at(6, Mode::Car), 0);
    }

    /// 右・左・Uターンのコストが手段ごとに違うモデル。
    fn turn_model() -> TimeModel {
        let mut model = TimeModel::synthetic();
        model.turn = PerMode {
            walk: TurnParams {
                right_s: 1.0,
                left_s: 1.0,
                u_turn_s: 2.0,
                straight_max_deg: 30.0,
                u_turn_min_deg: 150.0,
            },
            bike: TurnParams {
                right_s: 3.0,
                left_s: 2.0,
                u_turn_s: 8.0,
                straight_max_deg: 30.0,
                u_turn_min_deg: 150.0,
            },
            car: TurnParams {
                right_s: 10.0,
                left_s: 4.0,
                u_turn_s: 30.0,
                straight_max_deg: 30.0,
                u_turn_min_deg: 150.0,
            },
        };
        model
    }

    #[test]
    fn turn_costs_distinguish_straight_right_left_and_u_turn() {
        let (mut b, s) = grid(3, 3, 100.0);
        b.set_time_model(turn_model());
        let g = b.build();
        let n = |r, c| s.node(r, c);
        let d = |from: NodeId, to: NodeId| {
            let e = (0..g.edge_count() as u32)
                .find(|&e| {
                    let r = g.edge(e);
                    (r.tail, r.head) == (from, to) || (r.tail, r.head) == (to, from)
                })
                .unwrap();
            DirEdge::new(e, g.edge(e).tail != from)
        };
        // 東向きに (1,0)→(1,1) へ入り、中央のノードから出る。
        let east_in = d(n(1, 0), n(1, 1));
        let cost = |out: DirEdge, mode| turn_cost_ds(&g, east_in, out, mode);
        let straight = d(n(1, 1), n(1, 2));
        let left = d(n(1, 1), n(2, 1)); // 北へ（東向きから反時計回り）
        let right = d(n(1, 1), n(0, 1)); // 南へ（時計回り）
        let back = d(n(1, 1), n(1, 0));
        assert_eq!(back, east_in.reversed());
        assert_eq!(cost(straight, Mode::Car), 0);
        assert_eq!(cost(left, Mode::Car), 40);
        assert_eq!(cost(right, Mode::Car), 100);
        assert_eq!(cost(back, Mode::Car), 300);
        assert_eq!(cost(left, Mode::Bike), 20);
        assert_eq!(cost(right, Mode::Bike), 30);
        assert_eq!(cost(back, Mode::Bike), 80);
        assert_eq!(cost(right, Mode::Walk), 10);
        assert_eq!(cost(back, Mode::Walk), 20);
        // 左側通行：右折は左折より重い（car・bike）。
        assert!(cost(right, Mode::Car) > cost(left, Mode::Car));

        // ほかの向きから入っても同じ関係。北向き（(0,1)→(1,1)）では、東が右、西が左。
        let north_in = d(n(0, 1), n(1, 1));
        let turn = |out: DirEdge| turn_cost_ds(&g, north_in, out, Mode::Car);
        assert_eq!(turn(d(n(1, 1), n(2, 1))), 0);
        assert_eq!(turn(d(n(1, 1), n(1, 2))), 100); // 東：右
        assert_eq!(turn(d(n(1, 1), n(1, 0))), 40); // 西：左
        assert_eq!(turn(d(n(1, 1), n(0, 1))), 300);
        // 西向き（(1,2)→(1,1)）：北が右、南が左。
        let west_in = d(n(1, 2), n(1, 1));
        assert_eq!(
            turn_cost_ds(&g, west_in, d(n(1, 1), n(2, 1)), Mode::Car),
            100
        );
        assert_eq!(
            turn_cost_ds(&g, west_in, d(n(1, 1), n(0, 1)), Mode::Car),
            40
        );
        // 南向き（(2,1)→(1,1)）：西が右、東が左。
        let south_in = d(n(2, 1), n(1, 1));
        assert_eq!(
            turn_cost_ds(&g, south_in, d(n(1, 1), n(1, 0)), Mode::Car),
            100
        );
        assert_eq!(
            turn_cost_ds(&g, south_in, d(n(1, 1), n(1, 2)), Mode::Car),
            40
        );
    }

    #[test]
    fn turn_angles_are_signed_clockwise() {
        let (b, s) = grid(3, 3, 100.0);
        let g = b.build();
        let n = |r, c| s.node(r, c);
        let find = |from: NodeId, to: NodeId| {
            let e = (0..g.edge_count() as u32)
                .find(|&e| {
                    let r = g.edge(e);
                    (r.tail, r.head) == (from, to) || (r.tail, r.head) == (to, from)
                })
                .unwrap();
            DirEdge::new(e, g.edge(e).tail != from)
        };
        let east_in = find(n(1, 0), n(1, 1));
        let angle = |out| turn_angle_deg(&g, east_in, out).unwrap();
        assert!(angle(find(n(1, 1), n(1, 2))).abs() < 1e-9);
        assert!((angle(find(n(1, 1), n(0, 1))) - 90.0).abs() < 1e-9);
        assert!((angle(find(n(1, 1), n(2, 1))) + 90.0).abs() < 1e-9);
        assert!((angle(find(n(1, 1), n(1, 0))).abs() - 180.0).abs() < 1e-9);
    }

    #[test]
    fn thresholds_decide_straight_and_u_turn_by_angle() {
        // 中央のノード M の周りに、東への入り口と、いろいろな角度の出口を置く。
        let mut b = GraphBuilder::new();
        let m = b.add_node_deg(35.0, 139.0);
        let west = b.add_node_deg(35.0, 138.999);
        b.add_edge(EdgeSpec::new(west, m));
        // 出口は東から時計回りに +a 度（a は偏角）。局所平面で長さ 0.001 度ぶん。
        let cos_lat = 35.0_f64.to_radians().cos();
        let exits: Vec<(f64, NodeId)> = [
            0.0, 25.0, 31.0, 90.0, 149.0, 151.0, 179.0, -31.0, -149.0, -151.0,
        ]
        .iter()
        .map(|&a: &f64| {
            let (dx, dy) = (a.to_radians().cos() * 0.001, -a.to_radians().sin() * 0.001);
            (a, b.add_node_deg(35.0 + dy, 139.0 + dx / cos_lat))
        })
        .collect();
        for &(_, n) in &exits {
            b.add_edge(EdgeSpec::new(m, n));
        }
        b.set_time_model(turn_model());
        let g = b.build();
        let east_in = DirEdge::new(0, false);
        for (i, (a, _)) in exits.iter().enumerate() {
            let out = DirEdge::new(1 + i as u32, false);
            let want = if a.abs() <= 30.0 {
                0
            } else if a.abs() >= 150.0 {
                300
            } else if *a > 0.0 {
                100
            } else {
                40
            };
            assert_eq!(turn_cost_ds(&g, east_in, out, Mode::Car), want, "{a}度");
        }
    }

    #[test]
    fn turn_cost_uses_the_segments_at_the_junction_not_the_chord() {
        // A→B は B の手前で南東へ曲がる形状点Pを持つ。弦A→Bは東向きだが、最後の線分P→Bは南東向き。
        let mut b = GraphBuilder::new();
        let a = b.add_node_deg(35.0, 139.0);
        let node_b = b.add_node_deg(35.0, 139.001);
        let c = b.add_node_deg(35.0, 139.002);
        let p = Coord::from_latlon(crate::contract::LatLon::new(35.0005, 139.0005));
        let bent = b.add_edge(EdgeSpec::new(a, node_b).shape(&[p]));
        let out = b.add_edge(EdgeSpec::new(node_b, c));
        b.set_time_model(turn_model());
        let g = b.build();
        let cos_lat = 35.0_f64.to_radians().cos();
        // 入る向き P→B：北が正、東が正で (0.0005 × cos, -0.0005)。出る向きは東。
        let in_bearing = (0.0005 * cos_lat).atan2(-0.0005).to_degrees();
        let want = 90.0 - in_bearing; // 時計回り正の変化角
        let angle =
            turn_angle_deg(&g, DirEdge::new(bent, false), DirEdge::new(out, false)).unwrap();
        assert!((angle - want).abs() < 1e-6, "{angle} {want}");
        assert!(angle < -30.0 && angle > -150.0);
        assert_eq!(
            turn_cost_ds(
                &g,
                DirEdge::new(bent, false),
                DirEdge::new(out, false),
                Mode::Car
            ),
            40
        ); // 反時計回りなので左折

        // 逆向き（C→B→A）。入る向きは西、出る辺は B→P の向き（北西）。
        let back = turn_angle_deg(&g, DirEdge::new(out, true), DirEdge::new(bent, true)).unwrap();
        let out_bearing = (-0.0005 * cos_lat).atan2(0.0005).to_degrees(); // P−B の向き（北西）
        let want_back = {
            let d = out_bearing - 270.0;
            if d < -180.0 { d + 360.0 } else { d }
        };
        assert!((back - want_back).abs() < 1e-6, "{back} {want_back}");
        assert!(back > 30.0 && back < 150.0);
        assert_eq!(
            turn_cost_ds(
                &g,
                DirEdge::new(out, true),
                DirEdge::new(bent, true),
                Mode::Car
            ),
            100
        );
    }

    #[test]
    fn zero_length_segments_are_skipped_and_degenerate_edges_cost_nothing() {
        let mut b = GraphBuilder::new();
        let a = b.add_node_deg(35.0, 139.0);
        let node_b = b.add_node_deg(35.0, 139.001);
        let c = b.add_node_deg(35.0, 139.002);
        let head = b.node_coord(node_b);
        let p = Coord::from_latlon(crate::contract::LatLon::new(35.0005, 139.0005));
        // 形状点に終点と同じ座標が続く（最後の線分は長さ0）。
        let bent = b.add_edge(EdgeSpec::new(a, node_b).shape(&[p, head, head]));
        let plain = b.add_edge(EdgeSpec::new(a, node_b).shape(&[p]));
        let out = b.add_edge(EdgeSpec::new(node_b, c));
        // 2点が同じ座標で、形状点もない辺（向きが決まらない）。
        let twin = b.add_node_deg(35.0, 139.002);
        let stub = b.add_edge(EdgeSpec::new(c, twin));
        b.set_time_model(turn_model());
        let g = b.build();
        let out_d = DirEdge::new(out, false);
        assert_eq!(
            turn_angle_deg(&g, DirEdge::new(bent, false), out_d),
            turn_angle_deg(&g, DirEdge::new(plain, false), out_d)
        );
        assert_eq!(
            turn_angle_deg(&g, DirEdge::new(out, false), DirEdge::new(stub, false)),
            None
        );
        assert_eq!(
            turn_cost_ds(
                &g,
                DirEdge::new(out, false),
                DirEdge::new(stub, false),
                Mode::Car
            ),
            0
        );
        // 同じ辺の引き返しは角度によらずUターン。
        assert_eq!(
            turn_cost_ds(
                &g,
                DirEdge::new(stub, false),
                DirEdge::new(stub, true),
                Mode::Car
            ),
            300
        );
    }

    #[test]
    fn model_validation_rejects_unusable_values() {
        assert_eq!(TimeModel::synthetic().validate(), Ok(()));
        let mutations: [fn(&mut TimeModel); 12] = [
            |m| m.speed_kmh.car[0] = -1.0,
            |m| m.speed_kmh.walk[3] = f64::NAN,
            |m| m.speed_kmh.bike[21] = f64::INFINITY,
            |m| m.signal_penalty_s.car = -0.1,
            |m| m.stop_penalty_s.walk = f64::NAN,
            |m| m.level_crossing_penalty_s.bike = -5.0,
            |m| m.car_maxspeed_factor = 0.0,
            |m| m.car_maxspeed_cap_ratio = -1.0,
            |m| m.steps_factor_walk = 0.5,
            |m| m.turn.car.right_s = -1.0,
            |m| m.turn.bike.straight_max_deg = 150.0, // u_turn_min_deg と同じ
            |m| m.turn.walk.u_turn_min_deg = 181.0,
        ];
        for (i, mutate) in mutations.iter().enumerate() {
            let mut model = TimeModel::synthetic();
            mutate(&mut model);
            assert!(model.validate().is_err(), "{i}");
        }
        // 境界は受け付ける（速度0は「通行不可の種類」）。
        let mut model = TimeModel::synthetic();
        model.speed_kmh.car = [0.0; ALL_CLASSES];
        model.steps_factor_walk = 1.0;
        model.turn.car.straight_max_deg = 0.0;
        model.turn.car.u_turn_min_deg = 180.0;
        assert_eq!(model.validate(), Ok(()));
    }
}
