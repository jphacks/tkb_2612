//! ALTの下界（形式書15章）：ランドマークからの最短時間の表と、それによる下界。
// 骨組み：実装時に外す
#![allow(unused_variables)]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::contract::Mode;
use crate::domain::routing::graph::{Graph, NodeId};

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

    /// ノードvから目的地tまでの最短時間（辺の時間だけ）の下界（デシ秒）。
    ///
    /// 表があれば `unit_ds × (q_from[L][t] − q_from[L][v] − 1)` と
    /// `unit_ds × (q_to[L][v] − q_to[L][t] − 1)` のうち最大のもの（負なら0）を、`UNREACHABLE_Q` でない
    /// ランドマークについて取る。表がない手段は、直線距離÷その手段の最高速度。
    pub fn lower_bound_ds(&self, graph: &Graph, mode: Mode, v: NodeId, t: NodeId) -> u32 {
        todo!("形式書15章：ALTの下界")
    }
}
