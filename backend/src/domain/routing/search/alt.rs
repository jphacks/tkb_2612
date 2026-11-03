//! ALTの下界（形式書15章）：ランドマークからの最短時間の表と、それによる下界。
//!
//! 距離は、形式書8章の「辺の時間だけ」の最短時間（ノードの加算と右左折コストを除き、通過不可の
//! ノードも無視する。どれも経路を増やす方向の緩和なので、下界として正しい）。
//! 表の計算（前処理P8）は [`farthest_landmark_table`]：ランドマークを最遠点法で選び、距離表を作る。

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::contract::Mode;
use crate::contract::geo::straight_distance_m;
use crate::domain::routing::graph::time::edge_time_ds;
use crate::domain::routing::graph::{Graph, NodeId};

use super::fastest::STRAIGHT_FACTOR;

/// 「到達できない、または表せない」（`FR_*`・`TO_*` の値、形式書15章）。そのランドマークは下界に使わない。
pub const UNREACHABLE_Q: u16 = 0xFFFF;

/// `alt.bin` の `PARAMS`（形式書15章）。手段のキーは `car`・`bike`・`walk`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AltParams {
    /// 距離表の単位（デシ秒、初期値40＝4秒）。
    pub unit_ds: u32,
    /// 手段ごとのランドマークの個数L。表を持たない手段は含まない。
    #[serde(default)]
    pub landmarks: BTreeMap<String, u32>,
    /// 手段ごとの方向（`["from"]` または `["from", "to"]`）。
    #[serde(default)]
    pub directions: BTreeMap<String, Vec<String>>,
}

/// 1つの手段の距離表。
#[derive(Debug, Clone, PartialEq)]
pub struct ModeLandmarks {
    /// `LM_<M>`：ランドマークのノード番号。長さL。
    pub nodes: Vec<NodeId>,
    /// `FR_<M>`：ランドマークからの最短時間 d(L→v) を `unit_ds` で割った値。ノードvの値は `[v×L .. v×L+L]`。
    pub from: Vec<u16>,
    /// `TO_<M>`：ノードからランドマークへの最短時間 d(v→L)。`directions` に `to` があるときだけ。
    pub to: Option<Vec<u16>>,
}

impl ModeLandmarks {
    /// ランドマークの個数L。
    pub fn count(&self) -> usize {
        self.nodes.len()
    }
}

/// `alt.bin` の中身。
#[derive(Debug, Clone, PartialEq)]
pub struct Landmarks {
    pub params: AltParams,
    /// `PARAMS` のJSON全体（`params` に入らないキーも保つ）。
    pub raw_params: Value,
    /// 添字は `risk::mode_index`（0：walk、1：bike、2：car）。表を持たない手段は `None`。
    pub tables: [Option<ModeLandmarks>; 3],
}

impl Landmarks {
    /// 表を持たない（直線距離÷最高速度の下界だけを使う）`Landmarks`。合成グラフのテストにも使う。
    pub fn empty() -> Self {
        Self {
            params: AltParams {
                unit_ds: 40,
                landmarks: BTreeMap::new(),
                directions: BTreeMap::new(),
            },
            raw_params: Value::Object(Default::default()),
            tables: [None, None, None],
        }
    }

    /// その手段の距離表。
    pub fn table(&self, mode: Mode) -> Option<&ModeLandmarks> {
        self.tables[crate::domain::routing::risk::mode_index(mode)].as_ref()
    }

    /// 手段ごとの距離表から作る。個数・方向の記録（`PARAMS`）は表から決める。
    pub fn from_tables(unit_ds: u32, tables: [Option<ModeLandmarks>; 3]) -> Self {
        let mut landmarks = BTreeMap::new();
        let mut directions = BTreeMap::new();
        for mode in Mode::ALL {
            let Some(table) = &tables[crate::domain::routing::risk::mode_index(mode)] else {
                continue;
            };
            let key = mode.as_str().to_owned();
            landmarks.insert(key.clone(), table.count() as u32);
            let mut dirs = vec!["from".to_owned()];
            if table.to.is_some() {
                dirs.push("to".to_owned());
            }
            directions.insert(key, dirs);
        }
        let params = AltParams {
            unit_ds,
            landmarks,
            directions,
        };
        let raw_params = serde_json::to_value(&params).unwrap_or(Value::Null);
        Self {
            params,
            raw_params,
            tables,
        }
    }

    /// ノードvから目的地tまでの最短時間（辺の時間だけ）の下界（デシ秒）。
    ///
    /// 表があれば `unit_ds × (q_from[L][t] − q_from[L][v] − 1)` と
    /// `unit_ds × (q_to[L][v] − q_to[L][t] − 1)` のうち最大のもの（負なら0）を、`UNREACHABLE_Q` でない
    /// ランドマークについて取る。表がない手段は、直線距離÷その手段の最高速度。
    pub fn lower_bound_ds(&self, graph: &Graph, mode: Mode, v: NodeId, t: NodeId) -> u32 {
        let Some(table) = self.table(mode) else {
            return straight_lower_bound_ds(graph, mode, v, t);
        };
        let count = table.count();
        let unit = i64::from(self.params.unit_ds);
        let (v, t) = (v as usize, t as usize);
        let mut best: i64 = 0;
        for i in 0..count {
            let (fv, ft) = (table.from[v * count + i], table.from[t * count + i]);
            if fv != UNREACHABLE_Q && ft != UNREACHABLE_Q {
                best = best.max(unit * (i64::from(ft) - i64::from(fv) - 1));
            }
            if let Some(to) = &table.to {
                let (tv, tt) = (to[v * count + i], to[t * count + i]);
                if tv != UNREACHABLE_Q && tt != UNREACHABLE_Q {
                    best = best.max(unit * (i64::from(tv) - i64::from(tt) - 1));
                }
            }
        }
        u32::try_from(best).unwrap_or(u32::MAX)
    }
}

/// 直線距離÷その手段の最高速度による、辺の時間だけの最短時間の下界（デシ秒）。
///
/// 最高速度は、そのモデルの種類ごとの既定速度の最大（carは `maxspeed` による上限の倍率も考える）。
/// 直線距離は `straight_distance_m`（平均緯度での局所近似）で、辺の長さの丸め（0.1m）や道のりの近似と
/// 少しずれるので、最短時間経路の下界と同じ安全率 [`STRAIGHT_FACTOR`] を掛ける（許容的）。
/// モデルに速度がなければ0。
pub fn straight_lower_bound_ds(graph: &Graph, mode: Mode, v: NodeId, t: NodeId) -> u32 {
    let model = graph.time_model();
    let mut fastest_kmh = model
        .speed_kmh
        .get(mode)
        .iter()
        .fold(0.0_f64, |a, &b| a.max(b));
    if mode == Mode::Car {
        fastest_kmh *= model.car_maxspeed_cap_ratio.max(1.0);
    }
    if fastest_kmh <= 0.0 {
        return 0;
    }
    let meters = straight_distance_m(
        graph.node_coord(v).to_latlon(),
        graph.node_coord(t).to_latlon(),
    );
    let seconds = meters / (fastest_kmh / 3.6);
    let ds = (seconds * 10.0 * STRAIGHT_FACTOR).floor();
    if ds >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        ds as u32
    }
}

/// 辺の時間だけの最短時間（デシ秒）を `root` から求める（ダイクストラ法）。`reverse` が真なら、
/// 各ノードから `root` へ向かう最短時間。通れない有向辺は使わず、到達できないノードは `u32::MAX`。
/// 値が `u32::MAX` に達する距離は `u32::MAX - 1` に丸める。
pub fn edge_only_distances(graph: &Graph, mode: Mode, root: NodeId, reverse: bool) -> Vec<u32> {
    const INF: u32 = u32::MAX;
    let mut dist = vec![INF; graph.node_count()];
    let mut heap: BinaryHeap<Reverse<(u32, NodeId)>> = BinaryHeap::new();
    dist[root as usize] = 0;
    heap.push(Reverse((0, root)));
    while let Some(Reverse((d, u))) = heap.pop() {
        if d > dist[u as usize] {
            continue;
        }
        for &out in graph.out_dedges(u) {
            // 順方向は out、逆方向は out の逆向き（その有向辺が u に入る）の時間を使う。
            let used = if reverse { out.reversed() } else { out };
            let Some(time) = edge_time_ds(graph, used, mode) else {
                continue;
            };
            let v = graph.dedge_to(out);
            let nd = d.saturating_add(time).min(INF - 1);
            if nd < dist[v as usize] {
                dist[v as usize] = nd;
                heap.push(Reverse((nd, v)));
            }
        }
    }
    dist
}

/// 距離を表の値にする：`floor(d / unit_ds)`。到達できない、または16ビットに収まらないなら `UNREACHABLE_Q`。
fn quantize(distance_ds: u32, unit_ds: u32) -> u16 {
    if distance_ds == u32::MAX {
        return UNREACHABLE_Q;
    }
    let q = distance_ds / unit_ds;
    if q >= u32::from(UNREACHABLE_Q) {
        UNREACHABLE_Q
    } else {
        q as u16
    }
}

/// ランドマークを最遠点法で選ぶ。`start` から（その手段で）到達できるノードだけが候補で、最初の
/// ランドマークは `start` から最も遠いノード、次からは、すでに選んだランドマークからの最短時間の
/// 最小が最大のノード（どれからも到達できないノードは最大とみなす）。同じ値なら番号の小さいノード。
/// 候補が尽きたら、`count` より少なく返す。ランドマークからの最短時間の列（ノード順）も返す。
fn choose_landmarks(
    graph: &Graph,
    mode: Mode,
    count: usize,
    start: NodeId,
) -> (Vec<NodeId>, Vec<Vec<u32>>) {
    let n = graph.node_count();
    let mut nodes: Vec<NodeId> = Vec::new();
    let mut columns: Vec<Vec<u32>> = Vec::new();
    if n == 0 || count == 0 {
        return (nodes, columns);
    }
    let from_start = edge_only_distances(graph, mode, start, false);
    let eligible = |v: usize| from_start[v] != u32::MAX;
    // 最初は start から最も遠いノード。
    let mut next = (0..n)
        .filter(|&v| eligible(v))
        .max_by_key(|&v| (from_start[v], Reverse(v)))
        .map(|v| v as NodeId);
    // 選んだランドマークのどれかからの最短時間の最小（まだどれからも届かなければ u32::MAX）。
    let mut nearest = vec![u32::MAX; n];
    while let Some(landmark) = next {
        if nodes.len() == count {
            break;
        }
        nodes.push(landmark);
        let column = edge_only_distances(graph, mode, landmark, false);
        for (slot, &d) in nearest.iter_mut().zip(&column) {
            *slot = (*slot).min(d);
        }
        columns.push(column);
        next = (0..n)
            .filter(|&v| eligible(v) && !nodes.contains(&(v as NodeId)))
            .max_by_key(|&v| (nearest[v], Reverse(v)))
            .map(|v| v as NodeId);
    }
    (nodes, columns)
}

/// ランドマークを最遠点法で選ぶ（`choose_landmarks` の規則）。
pub fn select_landmarks(graph: &Graph, mode: Mode, count: usize, start: NodeId) -> Vec<NodeId> {
    choose_landmarks(graph, mode, count, start).0
}

/// 1つの手段の距離表（`FR_<M>`、`with_to` なら `TO_<M>` も）を作る（前処理P8）。ランドマークは
/// [`select_landmarks`] の最遠点法で選ぶ。`unit_ds` は表の単位（デシ秒、形式書15章の初期値は40）。
/// 全国グラフでは `count` 回（`with_to` なら2倍）のダイクストラ法を行う。
pub fn farthest_landmark_table(
    graph: &Graph,
    mode: Mode,
    count: usize,
    start: NodeId,
    unit_ds: u32,
    with_to: bool,
) -> ModeLandmarks {
    assert!(unit_ds > 0, "unit_dsは1以上でなければなりません");
    let n = graph.node_count();
    let (nodes, columns) = choose_landmarks(graph, mode, count, start);
    let l = nodes.len();
    let mut from = vec![UNREACHABLE_Q; n * l];
    for (i, column) in columns.iter().enumerate() {
        for (v, &d) in column.iter().enumerate() {
            from[v * l + i] = quantize(d, unit_ds);
        }
    }
    drop(columns);
    let to = with_to.then(|| {
        let mut to = vec![UNREACHABLE_Q; n * l];
        for (i, &landmark) in nodes.iter().enumerate() {
            let column = edge_only_distances(graph, mode, landmark, true);
            for (v, &d) in column.iter().enumerate() {
                to[v * l + i] = quantize(d, unit_ds);
            }
        }
        to
    });
    ModeLandmarks { nodes, from, to }
}
