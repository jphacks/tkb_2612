//! 緯度経度の格子（形式書11章）。吸着用の `odindex.bin` と、P3の近傍検索が使う。
//!
//! 格子の原点は日本矩形の南西端（20°・122°）。セルの番号は `row×cols + col`、
//! `row = floor((lat_e7 − lat0_e7) / cell_lat_e7)`、`col` も同様（`OdIndex::cell_key` と同じ）。

use crate::domain::routing::graph::Coord;

/// 日本矩形（形式書5章）。1e-7度。
pub const LAT_MIN_E7: i32 = 200_000_000;
pub const LAT_MAX_E7: i32 = 460_000_000;
pub const LON_MIN_E7: i32 = 1_220_000_000;
pub const LON_MAX_E7: i32 = 1_540_000_000;

/// 座標が日本矩形（両端を含む）に入っているか。
pub fn in_japan(c: Coord) -> bool {
    (LAT_MIN_E7..=LAT_MAX_E7).contains(&c.lat_e7) && (LON_MIN_E7..=LON_MAX_E7).contains(&c.lon_e7)
}

/// 格子の形。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridParams {
    pub lat0_e7: i32,
    pub lon0_e7: i32,
    pub cell_lat_e7: i32,
    pub cell_lon_e7: i32,
    pub rows: u32,
    pub cols: u32,
}

impl GridParams {
    /// 日本矩形の全体（北端・東端の座標も入る行数・列数）を覆う格子。
    pub fn japan(cell_lat_e7: i32, cell_lon_e7: i32) -> Self {
        assert!(cell_lat_e7 > 0 && cell_lon_e7 > 0);
        let rows = ((LAT_MAX_E7 - LAT_MIN_E7) / cell_lat_e7) as u32 + 1;
        let cols = ((LON_MAX_E7 - LON_MIN_E7) / cell_lon_e7) as u32 + 1;
        assert!(
            u64::from(rows) * u64::from(cols) <= u64::from(u32::MAX),
            "セルの番号がu32に収まらない"
        );
        Self {
            lat0_e7: LAT_MIN_E7,
            lon0_e7: LON_MIN_E7,
            cell_lat_e7,
            cell_lon_e7,
            rows,
            cols,
        }
    }

    /// 座標の（行、列）。格子の外は `None`。
    pub fn row_col(&self, c: Coord) -> Option<(u32, u32)> {
        let dlat = i64::from(c.lat_e7) - i64::from(self.lat0_e7);
        let dlon = i64::from(c.lon_e7) - i64::from(self.lon0_e7);
        if dlat < 0 || dlon < 0 {
            return None;
        }
        let row = dlat / i64::from(self.cell_lat_e7);
        let col = dlon / i64::from(self.cell_lon_e7);
        if row >= i64::from(self.rows) || col >= i64::from(self.cols) {
            return None;
        }
        Some((row as u32, col as u32))
    }

    /// 座標のセル番号。
    pub fn key(&self, c: Coord) -> Option<u32> {
        self.row_col(c).map(|(r, col)| self.key_of(r, col))
    }

    pub fn key_of(&self, row: u32, col: u32) -> u32 {
        row * self.cols + col
    }

    /// 線分 `a`-`b`（両端を含む）が通るセルの番号を `out` に足す。重複・順不同。
    ///
    /// 「線分上のいずれかの点が、`row_col` でそのセルに入る」セルをすべて足す。
    /// 辺の外接矩形ではなく線分で判定する（形式書11章）。ちょうどセルの境界に載る場合は、
    /// 隣のセルを余分に含めることがある（吸着の検索が漏れないための側の誤差）。
    /// 格子の外の部分は捨てる。
    pub fn segment_cells(&self, a: Coord, b: Coord, out: &mut Vec<u32>) {
        let lat0 = i64::from(self.lat0_e7);
        let lon0 = i64::from(self.lon0_e7);
        let (mut p0, mut p1) = (
            (i64::from(a.lat_e7) - lat0, i64::from(a.lon_e7) - lon0),
            (i64::from(b.lat_e7) - lat0, i64::from(b.lon_e7) - lon0),
        );
        if p0.0 > p1.0 {
            std::mem::swap(&mut p0, &mut p1);
        }
        let (y0, x0) = p0;
        let (y1, x1) = p1;
        let h = i64::from(self.cell_lat_e7);
        let w = i64::from(self.cell_lon_e7);
        let (rows, cols) = (i64::from(self.rows), i64::from(self.cols));

        let col_at = |y: i64| -> i64 {
            if y1 == y0 {
                // 水平な線分は呼び出し側で扱う
                return x0.div_euclid(w);
            }
            let dy = i128::from(y1 - y0);
            let num = i128::from(x0) * dy + i128::from(x1 - x0) * i128::from(y - y0);
            num.div_euclid(dy * i128::from(w)) as i64
        };

        let r_lo = y0.div_euclid(h);
        let r_hi = y1.div_euclid(h);
        for r in r_lo..=r_hi {
            if r < 0 || r >= rows {
                continue;
            }
            let ys = y0.max(r * h);
            let ye = y1.min((r + 1) * h);
            let (c_a, c_b) = if y0 == y1 {
                (x0.div_euclid(w), x1.div_euclid(w))
            } else {
                (col_at(ys), col_at(ye))
            };
            let (lo, hi) = (c_a.min(c_b), c_a.max(c_b));
            for c in lo.max(0)..=hi.min(cols - 1) {
                out.push((r * cols + c) as u32);
            }
        }
    }
}

/// セルごとの辺の表（CSR）。近傍の辺を引くのに使う。
#[derive(Debug, Clone)]
pub struct EdgeGrid {
    pub params: GridParams,
    keys: Vec<u32>,
    ofs: Vec<u32>,
    edges: Vec<u32>,
}

impl EdgeGrid {
    /// `(セル番号, 辺番号)` の組から作る。順不同・重複は許す。
    pub fn from_pairs(params: GridParams, mut pairs: Vec<(u32, u32)>) -> Self {
        use rayon::slice::ParallelSliceMut;
        pairs.par_sort_unstable();
        pairs.dedup();
        let mut keys = Vec::new();
        let mut ofs = Vec::new();
        let mut edges = Vec::with_capacity(pairs.len());
        for (key, edge) in pairs {
            if keys.last() != Some(&key) {
                keys.push(key);
                ofs.push(edges.len() as u32);
            }
            edges.push(edge);
        }
        ofs.push(edges.len() as u32);
        Self {
            params,
            keys,
            ofs,
            edges,
        }
    }

    /// セルを通る辺（昇順）。
    pub fn cell(&self, key: u32) -> &[u32] {
        match self.keys.binary_search(&key) {
            Ok(i) => &self.edges[self.ofs[i] as usize..self.ofs[i + 1] as usize],
            Err(_) => &[],
        }
    }

    /// 座標を含むセルと、その周囲8セルを通る辺を `out` に足し、昇順・重複なしにする。
    pub fn near(&self, c: Coord, out: &mut Vec<u32>) {
        out.clear();
        let Some((row, col)) = self.params.row_col(c) else {
            return;
        };
        for r in row.saturating_sub(1)..=(row + 1).min(self.params.rows - 1) {
            for cc in col.saturating_sub(1)..=(col + 1).min(self.params.cols - 1) {
                out.extend_from_slice(self.cell(self.params.key_of(r, cc)));
            }
        }
        out.sort_unstable();
        out.dedup();
    }

    /// 登録された（セル、辺）の組の数。
    pub fn entries(&self) -> usize {
        self.edges.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> GridParams {
        GridParams {
            lat0_e7: 200_000_000,
            lon0_e7: 1_220_000_000,
            cell_lat_e7: 25_000,
            cell_lon_e7: 25_000,
            rows: 10,
            cols: 10,
        }
    }

    fn cells(p: &GridParams, a: Coord, b: Coord) -> Vec<u32> {
        let mut out = Vec::new();
        p.segment_cells(a, b, &mut out);
        out.sort_unstable();
        out.dedup();
        out
    }

    fn at(row_frac: f64, col_frac: f64) -> Coord {
        // セル単位（小数）で指定した座標
        Coord::new(
            200_000_000 + (row_frac * 25_000.0) as i32,
            1_220_000_000 + (col_frac * 25_000.0) as i32,
        )
    }

    #[test]
    fn japan_grid_includes_the_northeast_corner() {
        let g = GridParams::japan(25_000, 25_000);
        assert_eq!((g.rows, g.cols), (10_401, 12_801));
        assert_eq!(
            g.row_col(Coord::new(LAT_MAX_E7, LON_MAX_E7)),
            Some((10_400, 12_800))
        );
        assert_eq!(g.row_col(Coord::new(LAT_MIN_E7, LON_MIN_E7)), Some((0, 0)));
        assert_eq!(g.row_col(Coord::new(LAT_MIN_E7 - 1, LON_MIN_E7)), None);
        assert!(in_japan(Coord::new(LAT_MAX_E7, LON_MIN_E7)));
        assert!(!in_japan(Coord::new(LAT_MAX_E7 + 1, LON_MIN_E7)));
    }

    #[test]
    fn segment_inside_one_cell() {
        let p = params();
        assert_eq!(cells(&p, at(2.2, 3.2), at(2.8, 3.9)), vec![2 * 10 + 3]);
        // 点（退化した線分）
        assert_eq!(cells(&p, at(2.5, 3.5), at(2.5, 3.5)), vec![23]);
    }

    #[test]
    fn horizontal_and_vertical_segments() {
        let p = params();
        assert_eq!(cells(&p, at(1.5, 1.5), at(1.5, 4.5)), vec![11, 12, 13, 14]);
        assert_eq!(cells(&p, at(4.5, 1.5), at(1.5, 1.5)), vec![11, 21, 31, 41]);
    }

    #[test]
    fn diagonal_segment_visits_only_the_cells_it_crosses() {
        let p = params();
        // (0.5,0.5) から (2.5,1.5)：row0 は col0 から、row1 は col0..1、row2 は col1
        let got = cells(&p, at(0.5, 0.5), at(2.5, 1.5));
        assert_eq!(got, vec![0, 10, 11, 21]);
        // 向きを逆にしても同じ
        assert_eq!(cells(&p, at(2.5, 1.5), at(0.5, 0.5)), got);
        // 外接矩形（2行×2列=6セル）ではなく、通るセルだけ
        let bbox_cells = 3 * 2;
        assert!(got.len() < bbox_cells);
        // 逆向きの斜め
        let got = cells(&p, at(0.5, 2.5), at(2.5, 1.5));
        assert_eq!(got, vec![2, 11, 12, 21]);
    }

    #[test]
    fn every_endpoint_cell_is_included_and_outside_is_dropped() {
        let p = params();
        for (a, b) in [
            (at(0.0, 0.0), at(9.99, 9.99)),
            (at(3.0, 3.0), at(3.0, 4.0)), // 境界上
            (at(5.0, 5.0), at(6.0, 5.0)),
        ] {
            let got = cells(&p, a, b);
            assert!(got.contains(&p.key(a).unwrap()), "{a:?}");
            assert!(got.contains(&p.key(b).unwrap()), "{b:?}");
        }
        // 格子の外にはみ出す部分は捨てる
        let got = cells(&p, at(9.5, 9.5), at(11.5, 11.5));
        assert_eq!(got, vec![99]);
    }

    #[test]
    fn edge_grid_lookup() {
        let p = params();
        let grid = EdgeGrid::from_pairs(p, vec![(11, 7), (11, 3), (12, 3), (11, 7), (55, 9)]);
        assert_eq!(grid.cell(11), &[3, 7]);
        assert_eq!(grid.cell(12), &[3]);
        assert!(grid.cell(13).is_empty());
        assert_eq!(grid.entries(), 4);
        let mut out = Vec::new();
        grid.near(at(1.5, 1.5), &mut out);
        assert_eq!(out, vec![3, 7]);
        grid.near(at(5.5, 5.5), &mut out);
        assert_eq!(out, vec![9]);
        grid.near(at(8.5, 8.5), &mut out);
        assert!(out.is_empty());
    }
}
