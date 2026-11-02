//! 右左折禁止（形式書9章）。「続けて通ってはいけない有向辺の列」に統一して持つ。
// 骨組み：実装時に外す
#![allow(unused_variables)]

use serde_json::Value;

use crate::contract::Mode;

use crate::domain::routing::graph::{DirEdge, Graph};

/// 禁止の1件（`RESTR` 節のレコードを `SEQ` で展開したもの）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restriction {
    /// 禁止する有向辺の列（長さ2以上）。隣り合う有向辺がつながっている。
    pub seq: Vec<DirEdge>,
    /// 対象の手段のビット（walk=1、bike=2、car=4。`Mode::mask`）。0にならない。
    pub modes: u8,
    /// 元のrelation（0：via-node、1：via-way）。
    pub kind: u8,
}

impl Restriction {
    /// `RESTR` 節の `elem_size`（`{seq_start: u32, seq_len: u16, modes: u8, kind: u8}`）。
    pub const RECORD_SIZE: usize = 8;
}

/// `restrictions.bin` の中身。
#[derive(Debug, Clone, PartialEq)]
pub struct Restrictions {
    /// `PARAMS`（JSON）。
    pub params: Value,
    /// 列の辞書順（`SEQ` の値の並び）で並べ、同じ列・同じ手段は1つにまとめる。
    pub items: Vec<Restriction>,
}

impl Restrictions {
    /// 禁止がない（`restrictions.bin` が空）場合。合成グラフのテストにも使う。
    pub fn empty() -> Self {
        Self {
            params: Value::Object(Default::default()),
            items: Vec::new(),
        }
    }

    /// 読込時の検査（形式書9章）：列の有向辺がすべて2E未満、隣り合う有向辺がつながっていること、
    /// `modes ≠ 0`、`seq_len ≥ 2`、並び順。
    pub fn validate(&self, graph: &Graph) -> Result<(), RestrictionError> {
        todo!("形式書9章の読込時の検査")
    }
}

/// `Restrictions::validate` の失敗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestrictionError {
    /// 有向辺の番号が2E以上。
    DirEdgeOutOfRange { item: usize },
    /// 隣り合う有向辺がつながっていない。
    NotConnected { item: usize },
    /// 列が2未満、または `modes` が0。
    Malformed { item: usize },
    /// 並び順・重複の違反。
    NotSorted { item: usize },
}

impl std::fmt::Display for RestrictionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RestrictionError::DirEdgeOutOfRange { item } => {
                write!(f, "restrictions.binの{item}件目の有向辺が範囲外です")
            }
            RestrictionError::NotConnected { item } => {
                write!(f, "restrictions.binの{item}件目の列がつながっていません")
            }
            RestrictionError::Malformed { item } => {
                write!(f, "restrictions.binの{item}件目の列または手段が不正です")
            }
            RestrictionError::NotSorted { item } => {
                write!(
                    f,
                    "restrictions.binの{item}件目が並び順どおりではありません"
                )
            }
        }
    }
}

impl std::error::Error for RestrictionError {}

/// 手段ごとのAho–Corasick照合器（形式書9章）。探索の状態は「直前の有向辺＋照合器の状態」で、
/// 禁止の列の末尾に達する遷移は通れない。状態数は、列の接頭辞の数＋1以下。
#[derive(Debug, Clone)]
pub struct TurnAutomaton {
    /// 状態数（根を含む）。遷移表の持ち方は実装時に決める。
    states: u32,
}

impl TurnAutomaton {
    /// その手段（`modes` に `mode.mask()` を含む）の禁止だけから照合器を作る。
    pub fn build(restrictions: &Restrictions, mode: Mode) -> TurnAutomaton {
        todo!("形式書9章：Aho–Corasickの構築")
    }

    /// 出発直後の状態。
    pub fn start(&self) -> u32 {
        0
    }

    /// 状態数。
    pub fn state_count(&self) -> u32 {
        self.states
    }

    /// 有向辺 `d` を通った後の状態。禁止の列の末尾に達するなら通れないので `None`。
    pub fn step(&self, state: u32, d: DirEdge) -> Option<u32> {
        todo!("形式書9章：照合器の遷移")
    }
}
