//! 出発地・目的地を辺に吸着する（形式書11章）。
// 骨組み：実装時に外す
#![allow(unused_variables)]

use serde::{Deserialize, Serialize};

use crate::contract::{LatLon, Mode};
use crate::domain::routing::graph::{Coord, EdgeId, Graph};
use crate::domain::routing::topology::Topology;

/// `odindex.bin` の `PARAMS`（形式書11章）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OdIndexParams {
    /// 格子の原点（日本矩形の南西端 20°・122°）。
    pub lat0_e7: i32,
    pub lon0_e7: i32,
    /// セルの大きさ（初期値はどちらも25,000＝0.0025度）。
    pub cell_lat_e7: i32,
    pub cell_lon_e7: i32,
    pub rows: u32,
    pub cols: u32,
}

/// `odindex.bin` の中身：吸着のための格子。
#[derive(Debug, Clone, PartialEq)]
pub struct OdIndex {
    pub params: OdIndexParams,
    /// `PARAMS` のJSON全体（`params` に入らないキーも保つ）。
    pub raw_params: serde_json::Value,
    /// `CELLKEY`：空でないセルの番号 `row×cols + col` の昇順。長さK。
    pub cell_keys: Vec<u32>,
    /// `CELLOFS`：各セルの辺の開始位置。長さK+1。
    pub cell_ofs: Vec<u32>,
    /// `CELLEDG`：セルを通る辺の番号（セル内は昇順）。辺の折れ線のいずれかの線分が通るセルすべて。
    pub cell_edges: Vec<EdgeId>,
}

impl OdIndex {
    /// 座標のセル番号 `row×cols + col`。格子の外は `None`。
    /// `row = floor((lat_e7 − lat0_e7) / cell_lat_e7)`、`col` も同様。
    pub fn cell_key(&self, coord: Coord) -> Option<u32> {
        let p = &self.params;
        let dlat = i64::from(coord.lat_e7) - i64::from(p.lat0_e7);
        let dlon = i64::from(coord.lon_e7) - i64::from(p.lon0_e7);
        if dlat < 0 || dlon < 0 {
            return None;
        }
        let row = dlat / i64::from(p.cell_lat_e7);
        let col = dlon / i64::from(p.cell_lon_e7);
        if row >= i64::from(p.rows) || col >= i64::from(p.cols) {
            return None;
        }
        Some((row * i64::from(p.cols) + col) as u32)
    }

    /// セルを通る辺（昇順）。空のセルは空のスライス。
    pub fn edges_in_cell(&self, key: u32) -> &[EdgeId] {
        match self.cell_keys.binary_search(&key) {
            Ok(i) => &self.cell_edges[self.cell_ofs[i] as usize..self.cell_ofs[i + 1] as usize],
            Err(_) => &[],
        }
    }
}

/// 吸着した点（形式書11章）。辺番号と始点からの距離で表す仮想ノードとして探索に入れる。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnapPoint {
    pub edge: EdgeId,
    /// 辺の始点からの距離（デシメートル、0〜`length_dm`）。
    pub offset_dm: u32,
    /// 吸着した辺上の点。
    pub position: Coord,
    /// 元の点からの距離（m、`contract::geo` と同じ局所平面近似）。
    pub distance_m: f64,
}

/// 点を、その手段で通れる辺に吸着する（形式書11章）。
///
/// 対象は、その手段で少なくとも一方向に通れて、`Topology::snappable` が真の辺。距離が最小の辺の
/// 最も近い点に吸着し、同じ距離なら辺番号が小さいものを選ぶ。距離が `SNAP_MAX_DISTANCE_M`
/// を超える、または辺がなければ `None`（呼び出し側が `NoRoute` にする）。
pub fn snap(
    graph: &Graph,
    topology: &Topology,
    index: &OdIndex,
    point: LatLon,
    mode: Mode,
) -> Option<SnapPoint> {
    todo!("形式書11章：吸着")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> OdIndex {
        OdIndex {
            params: OdIndexParams {
                lat0_e7: 200_000_000,
                lon0_e7: 1_220_000_000,
                cell_lat_e7: 25_000,
                cell_lon_e7: 25_000,
                rows: 10,
                cols: 10,
            },
            raw_params: serde_json::Value::Null,
            cell_keys: vec![0, 11],
            cell_ofs: vec![0, 1, 3],
            cell_edges: vec![4, 5, 9],
        }
    }

    #[test]
    fn cell_lookup() {
        let i = index();
        assert_eq!(i.cell_key(Coord::new(200_000_000, 1_220_000_000)), Some(0));
        assert_eq!(i.cell_key(Coord::new(200_025_000, 1_220_025_000)), Some(11));
        assert_eq!(i.cell_key(Coord::new(199_999_999, 1_220_000_000)), None);
        assert_eq!(i.cell_key(Coord::new(200_250_000, 1_220_000_000)), None);
        assert_eq!(i.edges_in_cell(0), &[4]);
        assert_eq!(i.edges_in_cell(11), &[5, 9]);
        assert!(i.edges_in_cell(5).is_empty());
    }
}
