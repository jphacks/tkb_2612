//! P7：出発地・目的地の吸着用の格子 `odindex.bin` を作る（形式書11章）。構築だけで、
//! 吸着の関数（`snap::snap`）は別に実装する。
//!
//! 格子の原点は日本矩形の南西端（20°・122°）、セルは緯度・経度とも0.0025度（25,000）。
//! 辺の折れ線のいずれかの線分が通るセルすべてに、辺を登録する（外接矩形ではなく線分で判定）。
//! 吸着の対象かどうかは手段ごとに違うので、グラフのすべての辺を入れる。

use rayon::prelude::*;
use serde_json::{Value, json};

use crate::domain::routing::graph::{EdgeId, GraphParts};
use crate::domain::routing::prep::grid::GridParams;
use crate::domain::routing::prep::view::GraphView;
use crate::domain::routing::snap::{OdIndex, OdIndexParams};

/// セルの大きさの初期値（1e-7度、0.0025度）。
pub const CELL_E7: i32 = 25_000;

/// 辺 `e` の折れ線が通るセル番号（昇順・重複なし）を `out` に書く。
pub fn edge_cells(view: &GraphView<'_>, grid: &GridParams, e: EdgeId, out: &mut Vec<u32>) {
    out.clear();
    let line = view.polyline(e);
    if line.len() == 1 {
        out.extend(grid.key(line[0]));
    }
    for w in line.windows(2) {
        grid.segment_cells(w[0], w[1], out);
    }
    out.sort_unstable();
    out.dedup();
}

/// 吸着用の格子を作る。同じ入力から同じ結果になる。
pub fn build_odindex(parts: &GraphParts) -> OdIndex {
    let view = GraphView::new(parts);
    let grid = GridParams::japan(CELL_E7, CELL_E7);
    let edges = view.edge_count();

    // 辺を固定の大きさのかたまりに分けて並列に処理する。かたまりの結果は順に連結し、最後に並べ直す
    const CHUNK: usize = 1 << 16;
    let chunks: Vec<Vec<u64>> = (0..edges.div_ceil(CHUNK))
        .into_par_iter()
        .map(|c| {
            let mut pairs = Vec::new();
            let mut cells = Vec::new();
            for e in c * CHUNK..((c + 1) * CHUNK).min(edges) {
                edge_cells(&view, &grid, e as u32, &mut cells);
                pairs.extend(cells.iter().map(|&k| (u64::from(k) << 32) | e as u64));
            }
            pairs
        })
        .collect();
    let mut pairs: Vec<u64> = Vec::with_capacity(chunks.iter().map(Vec::len).sum());
    for chunk in chunks {
        pairs.extend(chunk);
    }
    pairs.par_sort_unstable();

    let mut cell_keys: Vec<u32> = Vec::new();
    let mut cell_ofs: Vec<u32> = Vec::new();
    let mut cell_edges: Vec<u32> = Vec::with_capacity(pairs.len());
    for p in pairs {
        let key = (p >> 32) as u32;
        if cell_keys.last() != Some(&key) {
            cell_keys.push(key);
            cell_ofs.push(cell_edges.len() as u32);
        }
        cell_edges.push(p as u32);
    }
    cell_ofs.push(cell_edges.len() as u32);

    let params = OdIndexParams {
        lat0_e7: grid.lat0_e7,
        lon0_e7: grid.lon0_e7,
        cell_lat_e7: grid.cell_lat_e7,
        cell_lon_e7: grid.cell_lon_e7,
        rows: grid.rows,
        cols: grid.cols,
    };
    let mut raw = match serde_json::to_value(&params) {
        Ok(Value::Object(map)) => map,
        _ => unreachable!("OdIndexParamsはJSONのオブジェクトになる"),
    };
    raw.insert("edges".to_owned(), json!(edges));
    raw.insert("cells".to_owned(), json!(cell_keys.len()));
    raw.insert("entries".to_owned(), json!(cell_edges.len()));
    OdIndex {
        params,
        raw_params: Value::Object(raw),
        cell_keys,
        cell_ofs,
        cell_edges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::routing::graph::time::TimeModel;
    use crate::domain::routing::graph::{Coord, DirEdge, EdgeRecord, ShapePoint, access};

    /// 辺ごとの折れ線（始点、中間点…、終点）から、直接グラフを組む。
    fn graph(lines: &[Vec<Coord>]) -> GraphParts {
        let mut node_ll: Vec<Coord> = Vec::new();
        let mut edges = Vec::new();
        let mut shape_ofs = vec![0u32];
        let mut shape = Vec::new();
        for line in lines {
            let tail = node_ll.len() as u32;
            node_ll.push(line[0]);
            let head = node_ll.len() as u32;
            node_ll.push(*line.last().unwrap());
            let mut cur = line[0];
            for &p in &line[1..line.len() - 1] {
                shape.push(ShapePoint {
                    dlat_e7: (p.lat_e7 - cur.lat_e7) as i16,
                    dlon_e7: (p.lon_e7 - cur.lon_e7) as i16,
                });
                cur = p;
            }
            shape_ofs.push(shape.len() as u32);
            edges.push(EdgeRecord {
                tail,
                head,
                length_dm: 10,
                class: 11,
                access: access::CAR_FWD | access::CAR_BWD,
                flags: 0,
                maxspeed_kmh: 0,
            });
        }
        let n = node_ll.len();
        let mut node_ofs = vec![0u32];
        let mut adj = Vec::new();
        for v in 0..n {
            for (e, edge) in edges.iter().enumerate() {
                if edge.tail as usize == v {
                    adj.push(DirEdge::new(e as u32, false));
                }
                if edge.head as usize == v {
                    adj.push(DirEdge::new(e as u32, true));
                }
            }
            node_ofs.push(adj.len() as u32);
        }
        GraphParts {
            params: Value::Null,
            time_model: TimeModel::synthetic(),
            node_ofs,
            adj,
            node_ll,
            node_flags: vec![0; n],
            edges,
            shape_ofs,
            shape,
        }
    }

    fn c(lat_cells: f64, lon_cells: f64) -> Coord {
        Coord::new(
            200_000_000 + (lat_cells * 25_000.0) as i32,
            1_220_000_000 + (lon_cells * 25_000.0) as i32,
        )
    }

    #[test]
    fn registers_an_edge_in_every_cell_its_polyline_crosses() {
        let parts = graph(&[
            // 辺0：セル(100,200) の中だけ
            vec![c(100.2, 200.2), c(100.8, 200.8)],
            // 辺1：東へ3セル（水平）
            vec![c(100.5, 200.5), c(100.5, 203.5)],
            // 辺2：L字（形状点で北へ折れる）。外接矩形ならセル(101,201)も入るが、線分は通らない
            vec![c(100.5, 201.5), c(100.5, 202.5), c(102.5, 202.5)],
        ]);
        let index = build_odindex(&parts);
        let p = &index.params;
        assert_eq!((p.rows, p.cols), (10_401, 12_801));
        let key = |row: u32, col: u32| row * p.cols + col;
        let edges_in = |row, col| index.edges_in_cell(key(row, col)).to_vec();
        assert_eq!(edges_in(100, 200), vec![0, 1]);
        assert_eq!(edges_in(100, 201), vec![1, 2]);
        assert_eq!(edges_in(100, 202), vec![1, 2]);
        assert_eq!(edges_in(100, 203), vec![1]);
        assert_eq!(edges_in(101, 202), vec![2]);
        assert_eq!(edges_in(102, 202), vec![2]);
        // L字の外接矩形の内側だが、線分が通らないセル
        assert!(edges_in(101, 201).is_empty());
        assert!(edges_in(102, 201).is_empty());
        // cell_key と整合
        assert_eq!(index.cell_key(c(100.5, 201.5)), Some(key(100, 201)));
        assert_eq!(index.raw_params["edges"], 3);
        assert_eq!(index.raw_params["entries"], index.cell_edges.len());
    }

    #[test]
    fn structure_is_sorted_and_deterministic() {
        let lines: Vec<Vec<Coord>> = (0..50)
            .map(|i| {
                let f = f64::from(i);
                vec![
                    c(10.0 + f * 0.37, 20.0 + f * 0.11),
                    c(12.0 + f * 0.2, 25.0 + f * 0.3),
                ]
            })
            .collect();
        let parts = graph(&lines);
        let a = build_odindex(&parts);
        let b = build_odindex(&parts);
        assert_eq!(a, b);
        assert!(a.cell_keys.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(a.cell_ofs.len(), a.cell_keys.len() + 1);
        assert_eq!(*a.cell_ofs.last().unwrap() as usize, a.cell_edges.len());
        for w in a.cell_ofs.windows(2) {
            let cell = &a.cell_edges[w[0] as usize..w[1] as usize];
            assert!(!cell.is_empty() && cell.windows(2).all(|e| e[0] < e[1]));
        }
        // 辺の端点を含むセルには必ず登録されている
        for (e, line) in lines.iter().enumerate() {
            for p in [line[0], line[1]] {
                let key = a.cell_key(p).unwrap();
                assert!(a.edges_in_cell(key).contains(&(e as u32)), "{e}");
            }
        }
    }
}
