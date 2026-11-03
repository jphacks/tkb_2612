//! P5：尤度表（路線区分×OSM種別、幅員、構造、路線番号の混同表）の推定（形式書13章、計画書4.3節、作業書M2b.2）。
//!
//! 候補が1本しかない事故（唯一の候補）の観測を数え（[`TableCounts`]）、事前分布へ縮めた確率にする
//! （[`Tables`]）。スコアは対数で足し合わせる。表の添字・確率の意味は次のとおり（すべて
//! 「その候補が本当の道路であるとき、その事故の観測が起こる確率」＝尤度）。
//!
//! | 表 | 行（候補の特徴） | 列（事故の観測） | 使う候補 |
//! |---|---|---|---|
//! | `route_by_kind` | OSM種別（`class` 0〜15） | 路線区分 `route_class` 0〜14 | 辺 |
//! | `width` | OSMから見積もった幅員区分（0＝不明、1〜6＝警察の単路の区分） | 警察の車道幅員の単路コード1〜6 | 辺 |
//! | `shape_by_struct` | 構造（0＝なし、1＝橋・layer>0、2＝トンネル・layer<0） | 道路形状の種類（0＝その他、1＝橋、2＝トンネル） | 辺 |
//! | `ref_states` | 路線区分0（国道）× 道路種別の群 | `ref`・`name` の番号の照合（0＝中立、1＝一致、2＝不一致） | 辺 |
//! | `route_by_cluster` | クラスタの最上位の `class` | 路線区分 | クラスタ |
//!
//! 推定は、各行について「数えた件数 + α × 列の周辺の割合」を「行の件数 + α」で割る（ディリクレ事前分布）。
//! 件数が少ない行は周辺の割合に近づく。確率の下限は [`MIN_PROBABILITY`]。

use serde_json::{Value, json};

/// 路線区分の数（`RouteClass`）。
pub const ROUTE_CLASSES: usize = 15;
/// OSM種別の数（`class` 0〜15。`track` まで）。
pub const EDGE_KINDS: usize = 16;
/// OSMから見積もった幅員区分の数（0＝不明、1〜6）。
pub const WIDTH_BINS: usize = 7;
/// 警察の車道幅員の単路コードの数（1〜6）。
pub const WIDTH_CODES: usize = 6;
/// 構造の種類の数。
pub const STRUCT_KINDS: usize = 3;
/// 事故の道路形状の種類の数。
pub const SHAPE_KINDS: usize = 3;
/// `ref` の番号の照合の状態の数。
pub const REF_STATES: usize = 3;
/// 番号の照合をする路線区分の数（国道だけ。主要地方道・一般都道府県道の路線コードは通し番号で、
/// 県道の番号ではない。実データで `ref` との一致が4割にとどまることを確かめた）。
pub const REF_CLASSES: usize = 1;
/// 番号の照合で分ける道路種別の群の数。
pub const REF_GROUPS: usize = 6;
/// 確率の下限。
pub const MIN_PROBABILITY: f64 = 1e-6;

/// 番号の照合の状態。
pub const REF_NEUTRAL: usize = 0;
pub const REF_MATCH: usize = 1;
pub const REF_MISMATCH: usize = 2;

/// 道路形状のコードから、構造との照合に使う種類にする（11：トンネル→2、12：橋→1、それ以外→0）。
pub fn shape_kind(road_shape: u8) -> usize {
    match road_shape {
        11 => 2,
        12 => 1,
        _ => 0,
    }
}

/// OSM種別（`class`）から、番号の照合の群にする。
pub fn ref_group(class: u8) -> usize {
    match class {
        0 | 1 => 0,
        2 | 3 => 1,
        4 | 5 => 2,
        6 | 7 => 3,
        8 | 9 => 4,
        _ => 5,
    }
}

/// 唯一の候補から数えた件数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableCounts {
    /// `[OSM種別][路線区分]`。
    pub route_by_kind: [[u64; ROUTE_CLASSES]; EDGE_KINDS],
    /// `[幅員区分][警察の単路コード - 1]`。
    pub width: [[u64; WIDTH_CODES]; WIDTH_BINS],
    /// `[構造][道路形状の種類]`。
    pub shape_by_struct: [[u64; SHAPE_KINDS]; STRUCT_KINDS],
    /// `[路線区分][道路種別の群][照合の状態]`。
    pub ref_states: [[[u64; REF_STATES]; REF_GROUPS]; REF_CLASSES],
    /// `[クラスタの最上位のclass][路線区分]`。
    pub route_by_cluster: [[u64; ROUTE_CLASSES]; EDGE_KINDS],
    /// 辺の唯一の候補の事故の数。
    pub unique_edge_accidents: u64,
    /// クラスタの唯一の候補の事故の数。
    pub unique_cluster_accidents: u64,
}

impl Default for TableCounts {
    fn default() -> Self {
        Self {
            route_by_kind: [[0; ROUTE_CLASSES]; EDGE_KINDS],
            width: [[0; WIDTH_CODES]; WIDTH_BINS],
            shape_by_struct: [[0; SHAPE_KINDS]; STRUCT_KINDS],
            ref_states: [[[0; REF_STATES]; REF_GROUPS]; REF_CLASSES],
            route_by_cluster: [[0; ROUTE_CLASSES]; EDGE_KINDS],
            unique_edge_accidents: 0,
            unique_cluster_accidents: 0,
        }
    }
}

impl TableCounts {
    /// 別の集計を足す（整数の和なので、足す順によらず同じ結果になる）。
    pub fn merge(&mut self, other: &TableCounts) {
        fn add<const N: usize>(a: &mut [u64; N], b: &[u64; N]) {
            for (x, y) in a.iter_mut().zip(b) {
                *x += y;
            }
        }
        for k in 0..EDGE_KINDS {
            add(&mut self.route_by_kind[k], &other.route_by_kind[k]);
            add(&mut self.route_by_cluster[k], &other.route_by_cluster[k]);
        }
        for b in 0..WIDTH_BINS {
            add(&mut self.width[b], &other.width[b]);
        }
        for s in 0..STRUCT_KINDS {
            add(&mut self.shape_by_struct[s], &other.shape_by_struct[s]);
        }
        for c in 0..REF_CLASSES {
            for g in 0..REF_GROUPS {
                add(&mut self.ref_states[c][g], &other.ref_states[c][g]);
            }
        }
        self.unique_edge_accidents += other.unique_edge_accidents;
        self.unique_cluster_accidents += other.unique_cluster_accidents;
    }
}

/// 件数から確率の行を作る。`prior_weight` は事前分布（列の周辺の割合）の重みα。
fn smoothed_rows<const R: usize, const C: usize>(
    counts: &[[u64; C]; R],
    prior_weight: f64,
) -> [[f64; C]; R] {
    let mut column_total = [0_u64; C];
    for row in counts {
        for (t, n) in column_total.iter_mut().zip(row) {
            *t += n;
        }
    }
    let grand: u64 = column_total.iter().sum();
    let mut prior = [0.0_f64; C];
    for (p, &t) in prior.iter_mut().zip(&column_total) {
        // 全体が空でも一様になる（+1の加算）
        *p = (t as f64 + 1.0) / (grand as f64 + C as f64);
    }
    let mut out = [[0.0_f64; C]; R];
    for (r, row) in counts.iter().enumerate() {
        let n: u64 = row.iter().sum();
        for c in 0..C {
            let p = (row[c] as f64 + prior_weight * prior[c]) / (n as f64 + prior_weight);
            out[r][c] = p.max(MIN_PROBABILITY);
        }
    }
    out
}

/// 有効数字9桁に丸める。
fn round_sig(x: f64) -> f64 {
    format!("{x:.8e}").parse().unwrap_or(x)
}

fn rounded<const R: usize, const C: usize>(rows: &[[f64; C]; R]) -> Value {
    Value::Array(
        rows.iter()
            .map(|row| Value::Array(row.iter().map(|&x| json!(round_sig(x))).collect()))
            .collect(),
    )
}

/// 確率の表。スコアには対数 `ln_*` を使う。
#[derive(Debug, Clone, PartialEq)]
pub struct Tables {
    pub prior_weight: f64,
    pub counts: TableCounts,
    /// `[OSM種別][路線区分]`。
    pub route_by_kind: [[f64; ROUTE_CLASSES]; EDGE_KINDS],
    pub width: [[f64; WIDTH_CODES]; WIDTH_BINS],
    pub shape_by_struct: [[f64; SHAPE_KINDS]; STRUCT_KINDS],
    pub ref_states: [[[f64; REF_STATES]; REF_GROUPS]; REF_CLASSES],
    pub route_by_cluster: [[f64; ROUTE_CLASSES]; EDGE_KINDS],
    /// 表を使わない（すべての因子が1）か。距離だけで決める試験用。
    neutral: bool,
}

impl Tables {
    /// 件数から表を作る。
    pub fn from_counts(counts: TableCounts, prior_weight: f64) -> Tables {
        let route_by_kind = smoothed_rows(&counts.route_by_kind, prior_weight);
        let width = smoothed_rows(&counts.width, prior_weight);
        let shape_by_struct = smoothed_rows(&counts.shape_by_struct, prior_weight);
        let route_by_cluster = smoothed_rows(&counts.route_by_cluster, prior_weight);
        // 番号の照合は、路線区分ごとに「群×状態」の周辺の割合を事前分布にする
        let ref_states = counts.ref_states.map(|c| smoothed_rows(&c, prior_weight));
        Tables {
            prior_weight,
            counts,
            route_by_kind,
            width,
            shape_by_struct,
            ref_states,
            route_by_cluster,
            neutral: false,
        }
    }

    /// すべての因子を1にする表（距離だけで決める試験用）。
    pub fn neutral() -> Tables {
        let mut tables = Tables::from_counts(TableCounts::default(), 1.0);
        tables.neutral = true;
        tables
    }

    pub fn is_neutral(&self) -> bool {
        self.neutral
    }

    /// 辺の候補の対数尤度（距離以外の因子）。`route_class` などが欠損・未知（`None`）の因子は0。
    #[allow(clippy::too_many_arguments)]
    pub fn edge_log_likelihood(
        &self,
        class: u8,
        route_class: Option<u8>,
        width_bin: usize,
        width_code: Option<u8>,
        struct_kind: usize,
        road_shape: Option<u8>,
        ref_state: usize,
    ) -> f64 {
        if self.neutral {
            return 0.0;
        }
        let mut ln = 0.0;
        let kind = usize::from(class).min(EDGE_KINDS - 1);
        if let Some(c) = route_class {
            ln += self.route_by_kind[kind][usize::from(c)].ln();
            if usize::from(c) < REF_CLASSES {
                ln += self.ref_states[usize::from(c)][ref_group(class)][ref_state].ln();
            }
        }
        if let Some(code @ 1..=6) = width_code {
            ln += self.width[width_bin][usize::from(code) - 1].ln();
        }
        if let Some(shape) = road_shape {
            ln += self.shape_by_struct[struct_kind][shape_kind(shape)].ln();
        }
        ln
    }

    /// クラスタの候補の対数尤度（距離以外の因子）。
    pub fn cluster_log_likelihood(&self, top_class: u8, route_class: Option<u8>) -> f64 {
        if self.neutral {
            return 0.0;
        }
        match route_class {
            Some(c) => {
                let kind = usize::from(top_class).min(EDGE_KINDS - 1);
                self.route_by_cluster[kind][usize::from(c)].ln()
            }
            None => 0.0,
        }
    }

    /// `assignment.bin` の `PARAMS.tables` に書くJSON。件数と確率の両方を持つ。
    ///
    /// 確率は有効数字9桁に丸めて書く（`serde_json` の既定の読み取りは、17桁の値を1ulpずらすことがあり、
    /// 書いて読み直した `PARAMS` が元と一致しなくなるため。スコアの計算には丸めない値を使う）。
    pub fn to_json(&self) -> Value {
        let route_by_kind = rounded(&self.route_by_kind);
        let width = rounded(&self.width);
        let shape_by_struct = rounded(&self.shape_by_struct);
        let route_by_cluster = rounded(&self.route_by_cluster);
        let ref_states: Vec<Value> = self.ref_states.iter().map(rounded).collect();
        json!({
            "prior_weight": self.prior_weight,
            "min_probability": MIN_PROBABILITY,
            "unique_edge_accidents": self.counts.unique_edge_accidents,
            "unique_cluster_accidents": self.counts.unique_cluster_accidents,
            "route_by_kind": {
                "rows": "OSM class 0..=15（motorway..track）",
                "cols": "route_class 0..=14",
                "counts": self.counts.route_by_kind,
                "probabilities": route_by_kind,
            },
            "width": {
                "rows": "OSMから見積もった幅員区分 0=不明,1=3.5m未満,2=5.5m未満,3=9m未満,4=13m未満,5=19.5m未満,6=19.5m以上",
                "cols": "警察の単路の車道幅員コード 1..=6",
                "counts": self.counts.width,
                "probabilities": width,
            },
            "shape_by_struct": {
                "rows": "構造 0=なし,1=橋かlayer>0,2=トンネルかlayer<0",
                "cols": "道路形状の種類 0=その他,1=橋(12),2=トンネル(11)",
                "counts": self.counts.shape_by_struct,
                "probabilities": shape_by_struct,
            },
            "ref_states": {
                "rows": "[route_class 0=国道][群 0=motorway系,1=trunk系,2=primary系,3=secondary系,4=tertiary系,5=その他]",
                "cols": "0=中立(番号なし),1=一致,2=不一致",
                "counts": self.counts.ref_states,
                "probabilities": ref_states,
            },
            "route_by_cluster": {
                "rows": "クラスタのtop_class 0..=15",
                "cols": "route_class 0..=14",
                "counts": self.counts.route_by_cluster,
                "probabilities": route_by_cluster,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_probability_distributions_shrunk_toward_the_marginal() {
        let mut counts = TableCounts::default();
        // OSM種別2（trunk）では路線区分0（国道）が95件、4が5件。種別11（residential）は4が100件
        counts.route_by_kind[2][0] = 95;
        counts.route_by_kind[2][4] = 5;
        counts.route_by_kind[11][4] = 100;
        let tables = Tables::from_counts(counts, 10.0);
        for row in &tables.route_by_kind {
            let sum: f64 = row.iter().sum();
            assert!((sum - 1.0).abs() < 1e-9, "{sum}");
            assert!(row.iter().all(|&p| p >= MIN_PROBABILITY));
        }
        let trunk = &tables.route_by_kind[2];
        let residential = &tables.route_by_kind[11];
        assert!(trunk[0] > 0.8 && trunk[0] < 0.95, "{}", trunk[0]);
        assert!(residential[4] > 0.85);
        assert!(trunk[0] / residential[0] > 20.0);
        // 件数のない行は周辺の割合（全体の分布）になる
        let empty = &tables.route_by_kind[7];
        let marginal_zero = (95.0 + 1.0) / (200.0 + 15.0);
        assert!((empty[0] - marginal_zero).abs() < 1e-9);
    }

    #[test]
    fn empty_counts_give_uniform_rows() {
        let tables = Tables::from_counts(TableCounts::default(), 5.0);
        for row in &tables.width {
            for &p in row {
                assert!((p - 1.0 / 6.0).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn merge_is_a_plain_sum_and_order_free() {
        let mut a = TableCounts::default();
        a.route_by_kind[1][2] = 3;
        a.ref_states[0][1][1] = 4;
        a.unique_edge_accidents = 7;
        let mut b = TableCounts::default();
        b.route_by_kind[1][2] = 5;
        b.width[3][2] = 9;
        b.unique_cluster_accidents = 2;
        let mut ab = a.clone();
        ab.merge(&b);
        let mut ba = b.clone();
        ba.merge(&a);
        assert_eq!(ab, ba);
        assert_eq!(ab.route_by_kind[1][2], 8);
        assert_eq!(
            (ab.unique_edge_accidents, ab.unique_cluster_accidents),
            (7, 2)
        );
    }

    #[test]
    fn neutral_tables_ignore_every_factor() {
        let t = Tables::neutral();
        assert!(t.is_neutral());
        assert_eq!(
            t.edge_log_likelihood(2, Some(0), 3, Some(3), 1, Some(12), REF_MATCH),
            0.0
        );
        assert_eq!(t.cluster_log_likelihood(2, Some(0)), 0.0);
    }

    #[test]
    fn unknown_observations_contribute_nothing() {
        let tables = Tables::from_counts(TableCounts::default(), 1.0);
        let ln = tables.edge_log_likelihood(4, None, 0, None, 0, None, REF_NEUTRAL);
        assert_eq!(ln, 0.0);
        // 路線区分が1以上は番号の照合の因子がない
        let a = tables.edge_log_likelihood(4, Some(4), 0, None, 0, None, REF_MATCH);
        let b = tables.edge_log_likelihood(4, Some(4), 0, None, 0, None, REF_MISMATCH);
        assert_eq!(a, b);
        // 国道には番号の照合の因子がある（空の件数では3状態が同じ確率なので同じ値になる）
        let c = tables.edge_log_likelihood(2, Some(0), 0, None, 0, None, REF_MATCH);
        assert!(c.is_finite());
    }

    #[test]
    fn shape_and_group_mappings() {
        assert_eq!((shape_kind(11), shape_kind(12), shape_kind(14)), (2, 1, 0));
        assert_eq!(
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 15].map(ref_group),
            [0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5]
        );
    }
}
