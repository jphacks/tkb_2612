//! P5：事故を道路の辺と交差点クラスタに割り当てる（形式書13章、計画書4.2〜4.4節、作業書M2b）。
//!
//! 入力は P1 の `graph.bin`（`GraphParts`）と `graph_attrs.bin`、P3 の `topology.bin`、P4 の事故の属性と
//! 事故の座標。出力は `assignment.bin` の中身（`Assignment`）。ファイルの読み書きは `data` が行う。
//!
//! # 規則（形式書13章が「P5で決める」とした部分。値は `AssignParams` と `PARAMS` に記録する）
//!
//! 1. **候補の道路**：`class` が motorway〜track（0〜15）で、歩道・横断歩道の印がない辺（車道の辺）。
//!    事故の座標から折れ線までの距離が35m以内のものを候補にする。
//! 2. **高速・自専道の一致**：辺が「高速・自専道の辺」（`class` が motorway・motorway_link、または
//!    `motorroad=yes`）であることと、事故が「高速・自専道のコードの事故」（`route_class` が5〜7）であることが
//!    一致する候補だけを残す（両方向）。路線コードが欠損・未知の事故は、高速・自専道のコードでない扱い。
//!    クラスタの候補も、接する車道の辺を見て同じ規則で残す（高速・自専道のコードの事故は、高速・自専道の辺が
//!    1本以上接するクラスタ。それ以外の事故は、高速・自専道でない車道の辺が1本以上接するクラスタ）。
//! 3. **道路形状**：0（一般交通の場所）は割り当てず、理由4で帰属不明にする。1・31（交差点）は、30m以内の
//!    クラスタの候補（距離は、属するノードまでの最短距離）を先に調べ、残る候補があればクラスタだけで
//!    配分する。なければ、辺の候補に進む（交差点のそばに交差点ノードがない場合の保険）。それ以外の道路形状
//!    （交差点付近・単路・踏切）は辺の候補で配分する。
//! 4. **スコア**（対数で足す）：距離のガウス `exp(-d²/(2σ²))`（σ² = σ0² + 半幅²。辺の半幅は `width`・`lanes` タグ、
//!    なければ種別の既定値の半分。クラスタは半幅の代わりに `cluster_half_extent_m`）× 尤度表（`assign_tables`）。
//!    辺の尤度表は、路線区分×OSM種別、幅員の区分、橋・トンネル・layerと道路形状、国道の番号と
//!    `ref`・`name`（「国道N号」）の照合。主要地方道・一般都道府県道の路線コードは県道番号ではない
//!    通し番号なので、番号の照合はしない。
//! 5. **双子**：上下線の双子の辺は、候補の中で1つの組にまとめる。組のスコアは2本のスコアの和、
//!    組の位置は距離の近い方の辺（同じなら辺番号が小さい方）。重みは組の重みで、位置の近い方に載せ、もう一方
//!    （候補に入っていなくてもよい。高速・自専道の規則を満たすものだけ）に `MIRROR` として同じ重みで写す。
//!    写しの位置は、事故の座標を双子の辺の折れ線に投影した位置。
//! 6. **配分**：スコアの和で正規化し、重みが0.1以上の組を重みの大きい順（同じなら辺番号の小さい順）に
//!    最大3つ残す。残した重みの合計は1以下で、再正規化はしない。残る組がなければ理由3で帰属不明。
//! 7. **理由**：道路形状0→4。候補（距離だけで見た、35m以内の辺と交差点の事故での30m以内のクラスタ）が
//!    ひとつもない→1。候補はあるが規則2ですべて外れた→2。配分で残る組がない→3。
//!    255（その他）は、事故の座標が日本矩形の外にあるなど、上のどれにも当たらない場合に使う。
//!
//! 尤度表は、規則1〜3の候補が1本（双子の組なら1組、クラスタなら1つ）だけで、その距離が
//! `table_max_distance_m` 以下の事故から数える（`assign_tables`）。

use rayon::prelude::*;
use serde_json::{Value, json};

use crate::contract::LatLon;
use crate::contract::geo::METERS_PER_DEGREE;
use crate::domain::routing::accident::{
    AccidentAttr, AssignTarget, Assignment, CLUSTER_RADIUS_M, EDGE_RADIUS_M, MAX_TARGETS,
    MIN_WEIGHT, MISSING_U8, SIGMA0_M, TARGET_MIRROR, TargetKind, UnassignedReason,
    UnassignedRecord,
};
use crate::domain::routing::graph::{
    Coord, EdgeId, EdgeRecord, GraphParts, HighwayClass, edge_flags,
};
use crate::domain::routing::prep::assign_tables::{
    EDGE_KINDS, REF_CLASSES, REF_MATCH, REF_MISMATCH, REF_NEUTRAL, TableCounts, Tables, ref_group,
    shape_kind,
};
use crate::domain::routing::prep::attrs::GraphAttrs;
use crate::domain::routing::prep::geom::{LocalFrame, point_segment_distance};
use crate::domain::routing::prep::grid::{EdgeGrid, GridParams};
use crate::domain::routing::topology::{ClusterId, Topology};

/// 規則の版。変えたら `PARAMS` に記録される。
pub const ASSIGN_RULES_VERSION: &str = "jp-assign-1";

/// 車道の辺を種別ごとに決める1車道あたりの既定の幅員（m）。`width`・`lanes` タグがないときの半幅の元。
/// 添字は `class`（0〜15）。
pub const DEFAULT_CARRIAGEWAY_M: [f64; EDGE_KINDS] = [
    7.0, 4.0, 7.0, 4.0, 6.0, 4.0, 5.5, 4.0, 5.0, 3.5, 4.0, 4.0, 3.5, 3.0, 4.0, 3.0,
];

/// 割り当てのパラメータ。
#[derive(Debug, Clone, PartialEq)]
pub struct AssignParams {
    /// 距離の分散の基準 σ0（m）。
    pub sigma0_m: f64,
    /// 辺の候補の距離の上限（m）。
    pub edge_radius_m: f64,
    /// クラスタの候補の距離の上限（m）。
    pub cluster_radius_m: f64,
    /// 重みの下限。
    pub min_weight: f32,
    /// `MIRROR` でない候補の上限。
    pub max_targets: usize,
    /// クラスタの候補の距離の分散に足す、交差点の広がり（m）。
    pub cluster_half_extent_m: f64,
    /// 尤度表の推定に使う「唯一の候補」の距離の上限（m）。
    pub table_max_distance_m: f64,
    /// 尤度表の事前分布の重みα。
    pub table_prior_weight: f64,
    /// `lanes` タグから幅員を見積もるときの1車線の幅（m）。
    pub lane_width_m: f64,
    /// 候補検索の格子のセルの大きさ（1e-7度）。35m以上であること。
    pub grid_cell_e7: i32,
    /// 並列処理の1つの塊の事故数。
    pub chunk: usize,
}

impl Default for AssignParams {
    fn default() -> Self {
        Self {
            sigma0_m: SIGMA0_M,
            edge_radius_m: EDGE_RADIUS_M,
            cluster_radius_m: CLUSTER_RADIUS_M,
            min_weight: MIN_WEIGHT,
            max_targets: MAX_TARGETS,
            cluster_half_extent_m: 10.0,
            table_max_distance_m: 20.0,
            table_prior_weight: 10.0,
            lane_width_m: 3.25,
            grid_cell_e7: 5_000,
            chunk: 2_048,
        }
    }
}

impl AssignParams {
    /// `PARAMS` に書くJSON。
    pub fn to_json(&self) -> Value {
        json!({
            "rules_version": ASSIGN_RULES_VERSION,
            "sigma0_m": self.sigma0_m,
            "edge_radius_m": self.edge_radius_m,
            "cluster_radius_m": self.cluster_radius_m,
            "min_weight": self.min_weight,
            "max_targets": self.max_targets,
            "cluster_half_extent_m": self.cluster_half_extent_m,
            "table_max_distance_m": self.table_max_distance_m,
            "table_prior_weight": self.table_prior_weight,
            "lane_width_m": self.lane_width_m,
            "grid_cell_e7": self.grid_cell_e7,
            "default_carriageway_m": DEFAULT_CARRIAGEWAY_M,
            "distance_kernel": "exp(-d^2/(2*(sigma0^2+half_width^2)))（正規化の係数なし）",
            "candidate_edges": "class 0..=15（motorway..track）、歩道・横断歩道の印がない辺",
            "motorway_rule": "高速・自専道の辺（class 0・1またはmotorroad）とroute_class 5..=7が両方向で一致",
            "intersection_shapes": [1, 31],
            "general_traffic_place_shape": 0,
            "renormalize_after_cut": false,
        })
    }
}

// ---------------------------------------------------------------------------
// 検索の準備
// ---------------------------------------------------------------------------

/// 車道の辺か（形式書6.5節の `class` が motorway〜track で、歩道・横断歩道の印がない）。
pub fn is_road_edge(record: &EdgeRecord) -> bool {
    record.class <= HighwayClass::Track.code()
        && record.flags & (edge_flags::SIDEWALK | edge_flags::CROSSING) == 0
}

/// 高速・自専道の辺か（`class` が motorway・motorway_link、または `motorroad=yes`）。
pub fn is_motorway_edge(record: &EdgeRecord) -> bool {
    record.class <= HighwayClass::MotorwayLink.code() || record.flags & edge_flags::MOTORROAD != 0
}

/// クラスタに接する車道の辺の種類（`cluster_kinds` のビット）。
const CLUSTER_HAS_MOTORWAY: u8 = 0x01;
const CLUSTER_HAS_OTHER: u8 = 0x02;

/// 辺の候補。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeCand {
    pub edge: EdgeId,
    /// 事故の座標から折れ線までの距離（m）。
    pub dist_m: f64,
    /// 辺の始点からの位置（デシメートル）。
    pub pos_dm: u32,
}

/// クラスタの候補。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClusterCand {
    pub cluster: ClusterId,
    /// 事故の座標から、属するノードまでの最短距離（m）。
    pub dist_m: f64,
}

/// 作業用のバッファ（スレッドごとに1つ）。
#[derive(Debug, Default)]
pub struct Scratch {
    ids: Vec<u32>,
    xy: Vec<(f64, f64)>,
}

/// 候補を探す索引。
pub struct RoadIndex<'a> {
    parts: &'a GraphParts,
    attrs: &'a GraphAttrs,
    topology: &'a Topology,
    pub params: AssignParams,
    edge_grid: EdgeGrid,
    node_grid: EdgeGrid,
    cluster_kinds: Vec<u8>,
}

/// 緯度経度の範囲（1e-7度）に半径 `radius_m` の円が収まるセルの範囲 `(行の下端, 行の上端, 列の下端, 列の上端)`。
fn cell_box(grid: &GridParams, p: Coord, radius_m: f64) -> Option<(u32, u32, u32, u32)> {
    let cos_lat = (f64::from(p.lat_e7) * 1e-7).to_radians().cos().max(0.2);
    let dlat = (radius_m / METERS_PER_DEGREE * 1e7).ceil() as i64 + 1;
    let dlon = (radius_m / (METERS_PER_DEGREE * cos_lat) * 1e7).ceil() as i64 + 1;
    let lat = i64::from(p.lat_e7) - i64::from(grid.lat0_e7);
    let lon = i64::from(p.lon_e7) - i64::from(grid.lon0_e7);
    let (h, w) = (i64::from(grid.cell_lat_e7), i64::from(grid.cell_lon_e7));
    let r0 = (lat - dlat).div_euclid(h).max(0);
    let r1 = (lat + dlat).div_euclid(h).min(i64::from(grid.rows) - 1);
    let c0 = (lon - dlon).div_euclid(w).max(0);
    let c1 = (lon + dlon).div_euclid(w).min(i64::from(grid.cols) - 1);
    (r0 <= r1 && c0 <= c1).then_some((r0 as u32, r1 as u32, c0 as u32, c1 as u32))
}

/// 座標のまわりの半径 `radius_m` を覆うセルの要素を、昇順・重複なしで `ids` に入れる。
fn collect_ids(grid: &EdgeGrid, p: Coord, radius_m: f64, ids: &mut Vec<u32>) {
    ids.clear();
    let Some((r0, r1, c0, c1)) = cell_box(&grid.params, p, radius_m) else {
        return;
    };
    for r in r0..=r1 {
        for c in c0..=c1 {
            ids.extend_from_slice(grid.cell(grid.params.key_of(r, c)));
        }
    }
    ids.sort_unstable();
    ids.dedup();
}

/// 辺の折れ線の頂点を順に返す（始点ノード、中間形状点、終点ノード）。
fn polyline_points(parts: &GraphParts, e: EdgeId) -> impl Iterator<Item = Coord> + '_ {
    let record = &parts.edges[e as usize];
    let start = parts.shape_ofs[e as usize] as usize;
    let end = parts.shape_ofs[e as usize + 1] as usize;
    let first = parts.node_ll[record.tail as usize];
    let mut current = first;
    std::iter::once(first)
        .chain(parts.shape[start..end].iter().map(move |p| {
            current = Coord::new(
                current.lat_e7 + i32::from(p.dlat_e7),
                current.lon_e7 + i32::from(p.dlon_e7),
            );
            current
        }))
        .chain(std::iter::once(parts.node_ll[record.head as usize]))
}

/// 原点（事故の座標）から折れ線への最短距離（m）と、始点からの位置の割合（0〜1）。
pub fn project_origin(points: &[(f64, f64)]) -> (f64, f64) {
    let mut best = (f64::INFINITY, 0.0);
    let mut arc = 0.0;
    for w in points.windows(2) {
        let (d, t) = point_segment_distance((0.0, 0.0), w[0], w[1]);
        let len = (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1);
        if d < best.0 {
            best = (d, arc + t * len);
        }
        arc += len;
    }
    if points.len() == 1 {
        return (points[0].0.hypot(points[0].1), 0.0);
    }
    (best.0, if arc > 0.0 { best.1 / arc } else { 0.0 })
}

impl<'a> RoadIndex<'a> {
    /// 辺の格子、クラスタのノードの格子、クラスタの種類を作る。
    pub fn build(
        parts: &'a GraphParts,
        attrs: &'a GraphAttrs,
        topology: &'a Topology,
        params: AssignParams,
    ) -> Self {
        assert!(
            params.grid_cell_e7 >= 5_000,
            "セルは約35m四方より大きくする（候補の検索が漏れない）"
        );
        let grid = GridParams::japan(params.grid_cell_e7, params.grid_cell_e7);
        let edge_count = parts.edges.len();
        const BLOCK: usize = 1 << 16;
        let blocks: Vec<Vec<(u32, u32)>> = (0..edge_count.div_ceil(BLOCK))
            .into_par_iter()
            .map(|block| {
                let mut pairs = Vec::new();
                let mut cells = Vec::new();
                for e in block * BLOCK..((block + 1) * BLOCK).min(edge_count) {
                    if !is_road_edge(&parts.edges[e]) {
                        continue;
                    }
                    cells.clear();
                    let mut previous: Option<Coord> = None;
                    for c in polyline_points(parts, e as u32) {
                        if let Some(a) = previous {
                            grid.segment_cells(a, c, &mut cells);
                        }
                        previous = Some(c);
                    }
                    cells.sort_unstable();
                    cells.dedup();
                    pairs.extend(cells.iter().map(|&cell| (cell, e as u32)));
                }
                pairs
            })
            .collect();
        let mut edge_pairs = Vec::with_capacity(blocks.iter().map(Vec::len).sum());
        for mut block in blocks {
            edge_pairs.append(&mut block);
        }
        let edge_grid = EdgeGrid::from_pairs(grid, edge_pairs);

        let node_pairs: Vec<(u32, u32)> = (0..parts.node_ll.len())
            .into_par_iter()
            .filter(|&v| topology.node_cluster[v] != crate::domain::routing::NONE)
            .filter_map(|v| grid.key(parts.node_ll[v]).map(|k| (k, v as u32)))
            .collect();
        let node_grid = EdgeGrid::from_pairs(grid, node_pairs);

        let cluster_kinds: Vec<u8> = (0..topology.clusters.len() as u32)
            .into_par_iter()
            .map(|c| {
                let mut kinds = 0_u8;
                for &v in topology.nodes_of_cluster(c) {
                    let from = parts.node_ofs[v as usize] as usize;
                    let to = parts.node_ofs[v as usize + 1] as usize;
                    for d in &parts.adj[from..to] {
                        let record = &parts.edges[d.edge() as usize];
                        if is_road_edge(record) {
                            kinds |= if is_motorway_edge(record) {
                                CLUSTER_HAS_MOTORWAY
                            } else {
                                CLUSTER_HAS_OTHER
                            };
                        }
                    }
                }
                kinds
            })
            .collect();

        RoadIndex {
            parts,
            attrs,
            topology,
            params,
            edge_grid,
            node_grid,
            cluster_kinds,
        }
    }

    pub fn parts(&self) -> &GraphParts {
        self.parts
    }

    pub fn graph_attrs(&self) -> &GraphAttrs {
        self.attrs
    }

    pub fn topology(&self) -> &Topology {
        self.topology
    }

    /// 辺の格子に登録した（セル、辺）の組の数。
    pub fn edge_grid_entries(&self) -> usize {
        self.edge_grid.entries()
    }

    /// 辺の折れ線を、`frame` の原点を中心とする局所平面にして `xy` に入れる。
    fn fill_xy(&self, e: EdgeId, frame: &LocalFrame, xy: &mut Vec<(f64, f64)>) {
        xy.clear();
        xy.extend(polyline_points(self.parts, e).map(|c| frame.xy(c)));
    }

    /// 辺までの距離（m）と位置（デシメートル）。`frame` の原点が事故の座標。
    fn project_edge(&self, e: EdgeId, frame: &LocalFrame, xy: &mut Vec<(f64, f64)>) -> (f64, u32) {
        self.fill_xy(e, frame, xy);
        let (dist, ratio) = project_origin(xy);
        let length = self.parts.edges[e as usize].length_dm;
        let pos = (ratio * f64::from(length))
            .round()
            .clamp(0.0, f64::from(length));
        (dist, pos as u32)
    }

    /// 35m以内の車道の辺（高速・自専道の規則の前）。辺番号の昇順。
    pub fn edge_candidates(
        &self,
        p: Coord,
        frame: &LocalFrame,
        scratch: &mut Scratch,
    ) -> Vec<EdgeCand> {
        let Scratch { ids, xy } = scratch;
        collect_ids(&self.edge_grid, p, self.params.edge_radius_m, ids);
        let mut out = Vec::new();
        for &e in ids.iter() {
            let (dist, pos) = self.project_edge(e, frame, xy);
            if dist <= self.params.edge_radius_m {
                out.push(EdgeCand {
                    edge: e,
                    dist_m: dist,
                    pos_dm: pos,
                });
            }
        }
        out
    }

    /// 30m以内の交差点クラスタ（高速・自専道の規則の前）。クラスタ番号の昇順。
    pub fn cluster_candidates(
        &self,
        p: Coord,
        frame: &LocalFrame,
        scratch: &mut Scratch,
    ) -> Vec<ClusterCand> {
        collect_ids(
            &self.node_grid,
            p,
            self.params.cluster_radius_m,
            &mut scratch.ids,
        );
        let mut found: Vec<ClusterCand> = Vec::new();
        for &v in &scratch.ids {
            let (x, y) = frame.xy(self.parts.node_ll[v as usize]);
            let dist = x.hypot(y);
            if dist <= self.params.cluster_radius_m {
                found.push(ClusterCand {
                    cluster: self.topology.node_cluster[v as usize],
                    dist_m: dist,
                });
            }
        }
        // ノード番号の昇順に走査したので、同じクラスタは距離の小さい方を残す
        found.sort_by(|a, b| {
            a.cluster
                .cmp(&b.cluster)
                .then(a.dist_m.total_cmp(&b.dist_m))
        });
        found.dedup_by_key(|c| c.cluster);
        found
    }

    fn cluster_eligible(&self, cluster: ClusterId, motorway_coded: bool) -> bool {
        let kinds = self.cluster_kinds[cluster as usize];
        if motorway_coded {
            kinds & CLUSTER_HAS_MOTORWAY != 0
        } else {
            kinds & CLUSTER_HAS_OTHER != 0 || kinds == 0
        }
    }

    fn edge_eligible(&self, e: EdgeId, motorway_coded: bool) -> bool {
        is_motorway_edge(&self.parts.edges[e as usize]) == motorway_coded
    }

    // --- 辺の特徴 ---

    /// `width`・`lanes` タグから見積もった、その辺1本の車道の幅員（m）。
    fn own_width_m(&self, e: EdgeId) -> Option<f64> {
        let i = e as usize;
        let width_dm = self.attrs.width_dm.get(i).copied().unwrap_or(0);
        if width_dm > 0 {
            return Some(f64::from(width_dm) / 10.0);
        }
        let lanes = self.attrs.lanes.get(i).copied().unwrap_or(0);
        (lanes > 0).then(|| f64::from(lanes) * self.params.lane_width_m)
    }

    /// 双子なら2本の和（片方が不明なら、分かる方の2倍）。警察の車道幅員は上下線の合計なので。
    fn total_width_m(&self, e: EdgeId) -> Option<f64> {
        let own = self.own_width_m(e);
        match self.topology.twin_of(e) {
            None => own,
            Some(twin) => match (own, self.own_width_m(twin)) {
                (Some(a), Some(b)) => Some(a + b),
                (Some(a), None) | (None, Some(a)) => Some(2.0 * a),
                (None, None) => None,
            },
        }
    }

    /// 距離のガウスの半幅（m）。
    fn half_width_m(&self, e: EdgeId) -> f64 {
        let class = usize::from(self.parts.edges[e as usize].class).min(EDGE_KINDS - 1);
        self.own_width_m(e).unwrap_or(DEFAULT_CARRIAGEWAY_M[class]) / 2.0
    }

    /// 構造：0＝なし、1＝橋かlayer>0、2＝トンネルかlayer<0。
    fn struct_kind(&self, e: EdgeId) -> usize {
        let flags = self.parts.edges[e as usize].flags;
        let layer = self.attrs.layer.get(e as usize).copied().unwrap_or(0);
        if flags & edge_flags::TUNNEL != 0 || layer < 0 {
            2
        } else if flags & edge_flags::BRIDGE != 0 || layer > 0 {
            1
        } else {
            0
        }
    }

    /// `ref`・`name` の番号と、事故の路線の番号の照合。
    fn ref_state(&self, e: EdgeId, expected: u32) -> usize {
        let i = e as usize;
        let (mut matched, mut any) = (false, false);
        let mut note = |n: u32| {
            any = true;
            matched |= n == expected;
        };
        if let Some(&id) = self.attrs.ref_idx.get(i) {
            numeric_tokens(self.attrs.string(id), &mut note);
        }
        if let Some(&id) = self.attrs.name_idx.get(i) {
            named_numbers(self.attrs.string(id), &["国道"], &mut note);
        }
        if matched {
            REF_MATCH
        } else if any {
            REF_MISMATCH
        } else {
            REF_NEUTRAL
        }
    }

    /// 辺の特徴から、距離以外の因子の対数尤度。
    fn edge_evidence(&self, e: EdgeId, attr: &AccidentAttr, tables: &Tables) -> f64 {
        let record = &self.parts.edges[e as usize];
        let route_class = attr.route_class_kind().map(|c| c.code());
        let width_bin = self.total_width_m(e).map_or(0, width_bin);
        let width_code = (attr.carriageway_width != MISSING_U8).then_some(attr.carriageway_width);
        let road_shape = (attr.road_shape != MISSING_U8).then_some(attr.road_shape);
        let ref_state = match (route_class, expected_number(attr)) {
            (Some(c), Some(n)) if usize::from(c) < REF_CLASSES => self.ref_state(e, n),
            _ => REF_NEUTRAL,
        };
        tables.edge_log_likelihood(
            record.class,
            route_class,
            width_bin,
            width_code,
            self.struct_kind(e),
            road_shape,
            ref_state,
        )
    }

    fn edge_log_score(&self, cand: &EdgeCand, attr: &AccidentAttr, tables: &Tables) -> f64 {
        let half = self.half_width_m(cand.edge);
        let sigma2 = self.params.sigma0_m * self.params.sigma0_m + half * half;
        -(cand.dist_m * cand.dist_m) / (2.0 * sigma2) + self.edge_evidence(cand.edge, attr, tables)
    }

    fn cluster_log_score(&self, cand: &ClusterCand, attr: &AccidentAttr, tables: &Tables) -> f64 {
        let h = self.params.cluster_half_extent_m;
        let sigma2 = self.params.sigma0_m * self.params.sigma0_m + h * h;
        let top_class = self.topology.clusters[cand.cluster as usize].top_class;
        let route_class = attr.route_class_kind().map(|c| c.code());
        -(cand.dist_m * cand.dist_m) / (2.0 * sigma2)
            + tables.cluster_log_likelihood(top_class, route_class)
    }

    // --- 候補を集める（尤度表の推定と配分で共通） ---

    /// 規則1〜3で、事故ごとの候補を集める。
    fn gather(&self, p: Coord, attr: &AccidentAttr, scratch: &mut Scratch) -> Gathered {
        if attr.road_shape == 0 {
            return Gathered::Skip(UnassignedReason::GeneralTrafficPlace);
        }
        let frame = LocalFrame::new(p);
        let motorway_coded = attr.is_motorway_code();
        let mut raw_any = false;
        let mut excluded = Excluded::default();
        if is_intersection_shape(attr.road_shape) {
            let all = self.cluster_candidates(p, &frame, scratch);
            raw_any |= !all.is_empty();
            let (eligible, dropped): (Vec<_>, Vec<_>) = all
                .into_iter()
                .partition(|c| self.cluster_eligible(c.cluster, motorway_coded));
            excluded.clusters = dropped;
            if !eligible.is_empty() {
                return Gathered::Clusters {
                    cands: eligible,
                    frame,
                    excluded,
                };
            }
        }
        let all = self.edge_candidates(p, &frame, scratch);
        raw_any |= !all.is_empty();
        let (eligible, dropped): (Vec<_>, Vec<_>) = all
            .into_iter()
            .partition(|c| self.edge_eligible(c.edge, motorway_coded));
        excluded.edges = dropped;
        if eligible.is_empty() {
            return Gathered::Skip(if raw_any {
                UnassignedReason::MotorwayCodeMismatch
            } else {
                UnassignedReason::NoCandidate
            });
        }
        Gathered::Edges {
            cands: eligible,
            frame,
            excluded,
        }
    }

    /// 双子の組の鍵（組の小さい方の辺番号。双子がなければ自分）。
    fn twin_key(&self, e: EdgeId) -> EdgeId {
        self.topology.twin_of(e).map_or(e, |t| t.min(e))
    }

    /// 尤度表の推定のために、唯一の候補の事故の観測を数える。
    pub fn observe(
        &self,
        p: Coord,
        attr: &AccidentAttr,
        scratch: &mut Scratch,
        counts: &mut TableCounts,
    ) {
        let route_class = attr.route_class_kind().map(|c| c.code());
        match self.gather(p, attr, scratch) {
            Gathered::Skip(_) => {}
            Gathered::Clusters { cands, .. } => {
                let [only] = cands.as_slice() else { return };
                let (Some(c), true) =
                    (route_class, only.dist_m <= self.params.table_max_distance_m)
                else {
                    return;
                };
                let top_class = self.topology.clusters[only.cluster as usize].top_class;
                counts.route_by_cluster[usize::from(top_class).min(EDGE_KINDS - 1)]
                    [usize::from(c)] += 1;
                counts.unique_cluster_accidents += 1;
            }
            Gathered::Edges { cands, .. } => {
                let first_key = self.twin_key(cands[0].edge);
                if cands.iter().any(|c| self.twin_key(c.edge) != first_key) {
                    return;
                }
                // 近い方の辺で代表させる（同じ距離なら辺番号が小さい方）
                let best = cands
                    .iter()
                    .min_by(|a, b| a.dist_m.total_cmp(&b.dist_m).then(a.edge.cmp(&b.edge)))
                    .expect("候補は空でない");
                if best.dist_m > self.params.table_max_distance_m {
                    return;
                }
                let e = best.edge;
                let class = usize::from(self.parts.edges[e as usize].class).min(EDGE_KINDS - 1);
                counts.unique_edge_accidents += 1;
                if let Some(c) = route_class {
                    counts.route_by_kind[class][usize::from(c)] += 1;
                    if let (true, Some(n)) = (usize::from(c) < REF_CLASSES, expected_number(attr)) {
                        let state = self.ref_state(e, n);
                        counts.ref_states[usize::from(c)]
                            [ref_group(self.parts.edges[e as usize].class)][state] += 1;
                    }
                }
                if (1..=6).contains(&attr.carriageway_width) {
                    let bin = self.total_width_m(e).map_or(0, width_bin);
                    counts.width[bin][usize::from(attr.carriageway_width) - 1] += 1;
                }
                if matches!(attr.road_shape, 7 | 37 | 11 | 12 | 13 | 14 | 21 | 22 | 23) {
                    counts.shape_by_struct[self.struct_kind(e)][shape_kind(attr.road_shape)] += 1;
                }
            }
        }
    }

    /// 1件の事故を割り当てる。`detail` が真なら、確認用に候補の一覧も返す。
    pub fn assign_one(
        &self,
        tables: &Tables,
        p: Coord,
        attr: &AccidentAttr,
        scratch: &mut Scratch,
        detail: bool,
    ) -> Outcome {
        match self.gather(p, attr, scratch) {
            Gathered::Skip(reason) => Outcome::unassigned(reason),
            Gathered::Clusters {
                cands,
                excluded,
                frame: _,
            } => self.assign_clusters(tables, attr, &cands, &excluded, detail),
            Gathered::Edges {
                cands,
                frame,
                excluded,
            } => self.assign_edges(tables, attr, &cands, &frame, &excluded, scratch, detail),
        }
    }

    fn assign_clusters(
        &self,
        tables: &Tables,
        attr: &AccidentAttr,
        cands: &[ClusterCand],
        excluded: &Excluded,
        want_detail: bool,
    ) -> Outcome {
        let ln: Vec<f64> = cands
            .iter()
            .map(|c| self.cluster_log_score(c, attr, tables))
            .collect();
        let shares = normalized_shares(&ln);
        let mut order: Vec<usize> = (0..cands.len()).collect();
        order.sort_by(|&a, &b| {
            shares[b]
                .total_cmp(&shares[a])
                .then(cands[a].cluster.cmp(&cands[b].cluster))
        });
        let mut targets = Vec::new();
        for &i in &order {
            let w = shares[i] as f32;
            if w < self.params.min_weight || targets.len() >= self.params.max_targets {
                break;
            }
            targets.push(AssignTarget {
                target: cands[i].cluster,
                pos_dm: u32::MAX,
                weight: w,
                kind: TargetKind::Cluster.code(),
                flags: 0,
                reserved: 0,
            });
        }
        targets.sort_by_key(|t| (t.kind, t.target, t.pos_dm));
        let detail = want_detail.then(|| Detail {
            clusters: cands
                .iter()
                .zip(&shares)
                .map(|(c, &s)| DetailCluster {
                    cluster: c.cluster,
                    dist_m: c.dist_m,
                    share: s,
                    assigned: targets
                        .iter()
                        .find(|t| t.target == c.cluster)
                        .map(|t| t.weight),
                })
                .collect(),
            edges: Vec::new(),
            excluded_edges: excluded.edges.clone(),
            excluded_clusters: excluded.clusters.clone(),
        });
        let nearest = order.first().map_or(0.0, |&i| cands[i].dist_m);
        Outcome::finish(targets, nearest, detail)
    }

    #[allow(clippy::too_many_arguments)]
    fn assign_edges(
        &self,
        tables: &Tables,
        attr: &AccidentAttr,
        cands: &[EdgeCand],
        frame: &LocalFrame,
        excluded: &Excluded,
        scratch: &mut Scratch,
        want_detail: bool,
    ) -> Outcome {
        let motorway_coded = attr.is_motorway_code();
        let ln: Vec<f64> = cands
            .iter()
            .map(|c| self.edge_log_score(c, attr, tables))
            .collect();
        let shares = normalized_shares(&ln);

        // 双子の組にまとめる。鍵（組の小さい方の辺番号）の昇順
        let mut by_key: Vec<usize> = (0..cands.len()).collect();
        by_key.sort_by_key(|&i| (self.twin_key(cands[i].edge), cands[i].edge));
        struct Group {
            primary: usize,
            share: f64,
        }
        let mut groups: Vec<Group> = Vec::new();
        let mut last_key = None;
        for &i in &by_key {
            let key = self.twin_key(cands[i].edge);
            if last_key == Some(key) {
                let g = groups.last_mut().expect("同じ鍵の組がある");
                g.share += shares[i];
                let best = &cands[g.primary];
                let better = cands[i]
                    .dist_m
                    .total_cmp(&best.dist_m)
                    .then(cands[i].edge.cmp(&best.edge))
                    .is_lt();
                if better {
                    g.primary = i;
                }
            } else {
                groups.push(Group {
                    primary: i,
                    share: shares[i],
                });
                last_key = Some(key);
            }
        }
        let total: f64 = groups.iter().map(|g| g.share).sum();
        let mut order: Vec<usize> = (0..groups.len()).collect();
        let weight_of = |g: &Group| (g.share / total) as f32;
        order.sort_by(|&a, &b| {
            weight_of(&groups[b])
                .total_cmp(&weight_of(&groups[a]))
                .then(
                    cands[groups[a].primary]
                        .edge
                        .cmp(&cands[groups[b].primary].edge),
                )
        });

        let mut targets: Vec<AssignTarget> = Vec::new();
        for (direct, &gi) in order.iter().enumerate() {
            let w = weight_of(&groups[gi]);
            if w < self.params.min_weight || direct >= self.params.max_targets {
                break;
            }
            let primary = &cands[groups[gi].primary];
            targets.push(AssignTarget {
                target: primary.edge,
                pos_dm: primary.pos_dm,
                weight: w,
                kind: TargetKind::Edge.code(),
                flags: 0,
                reserved: 0,
            });
        }
        // 双子への写し
        let direct_targets = targets.clone();
        for t in &direct_targets {
            let Some(twin) = self.topology.twin_of(t.target) else {
                continue;
            };
            let record = &self.parts.edges[twin as usize];
            if !is_road_edge(record)
                || !self.edge_eligible(twin, motorway_coded)
                || targets
                    .iter()
                    .any(|x| x.kind == TargetKind::Edge.code() && x.target == twin)
            {
                continue;
            }
            let (_, pos) = self.project_edge(twin, frame, &mut scratch.xy);
            targets.push(AssignTarget {
                target: twin,
                pos_dm: pos,
                weight: t.weight,
                kind: TargetKind::Edge.code(),
                flags: TARGET_MIRROR,
                reserved: 0,
            });
        }
        targets.sort_by_key(|t| (t.kind, t.target, t.pos_dm));

        let detail = want_detail.then(|| Detail {
            edges: cands
                .iter()
                .zip(&shares)
                .map(|(c, &s)| DetailEdge {
                    edge: c.edge,
                    dist_m: c.dist_m,
                    share: s,
                    assigned: targets
                        .iter()
                        .find(|t| t.kind == TargetKind::Edge.code() && t.target == c.edge)
                        .map(|t| (t.weight, t.is_mirror())),
                })
                .collect(),
            clusters: Vec::new(),
            excluded_edges: excluded.edges.clone(),
            excluded_clusters: excluded.clusters.clone(),
        });
        let nearest = order
            .first()
            .map_or(0.0, |&gi| cands[groups[gi].primary].dist_m);
        Outcome::finish(targets, nearest, detail)
    }
}

/// 道路形状が交差点（1・31）か。
pub fn is_intersection_shape(road_shape: u8) -> bool {
    matches!(road_shape, 1 | 31)
}

/// 事故の路線コードから、OSMの `ref` と照合する番号。国道（上4桁が国道番号）だけ。
/// 主要地方道・一般都道府県道の上4桁は県道番号ではなく通し番号なので `None`。
pub fn expected_number(attr: &AccidentAttr) -> Option<u32> {
    use crate::domain::routing::accident::RouteClass::NationalRoute;
    (attr.route_class_kind()? == NationalRoute && attr.route_upper > 0)
        .then_some(u32::from(attr.route_upper))
}

/// 警察の車道幅員の単路コード1〜6に当たる、幅員（m）の区分。
pub fn width_bin(width_m: f64) -> usize {
    match width_m {
        w if w < 3.5 => 1,
        w if w < 5.5 => 2,
        w if w < 9.0 => 3,
        w if w < 13.0 => 4,
        w if w < 19.5 => 5,
        _ => 6,
    }
}

/// `ref` の値（`;`・`,` 区切り）のうち、数字だけの要素を `f` に渡す。`E4`・`C1` などは無視する。
pub fn numeric_tokens(value: &str, f: &mut impl FnMut(u32)) {
    for token in value.split([';', ',']) {
        let token = token.trim();
        if !token.is_empty()
            && token.bytes().all(|b| b.is_ascii_digit())
            && let Ok(n) = token.parse::<u32>()
        {
            f(n);
        }
    }
}

/// `name` の中の「<接頭辞><数字>号」（全角の数字も可）の番号を `f` に渡す。
pub fn named_numbers(name: &str, prefixes: &[&str], f: &mut impl FnMut(u32)) {
    for prefix in prefixes {
        for (at, _) in name.match_indices(prefix) {
            let rest = &name[at + prefix.len()..];
            let mut value: u32 = 0;
            let mut digits = 0;
            let mut end = 0;
            for (i, c) in rest.char_indices() {
                let d = match c {
                    '0'..='9' => c as u32 - '0' as u32,
                    '０'..='９' => c as u32 - '０' as u32,
                    _ => break,
                };
                value = value.saturating_mul(10).saturating_add(d);
                digits += 1;
                end = i + c.len_utf8();
            }
            if digits > 0 && rest[end..].starts_with('号') {
                f(value);
            }
        }
    }
}

/// 対数スコアから、合計が1の割合にする（最大値を引いてから指数を取る）。
fn normalized_shares(ln: &[f64]) -> Vec<f64> {
    let max = ln.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let raw: Vec<f64> = ln.iter().map(|&x| (x - max).exp()).collect();
    let total: f64 = raw.iter().sum();
    raw.into_iter().map(|x| x / total).collect()
}

/// 候補を集めた結果。
enum Gathered {
    /// 候補を調べずに、または調べた結果、帰属不明。
    Skip(UnassignedReason),
    Clusters {
        cands: Vec<ClusterCand>,
        #[allow(dead_code)]
        frame: LocalFrame,
        excluded: Excluded,
    },
    Edges {
        cands: Vec<EdgeCand>,
        frame: LocalFrame,
        excluded: Excluded,
    },
}

/// 高速・自専道の規則で外れた候補（確認用）。
#[derive(Debug, Default, Clone)]
struct Excluded {
    edges: Vec<EdgeCand>,
    clusters: Vec<ClusterCand>,
}

/// 確認用：辺の候補。
#[derive(Debug, Clone, PartialEq)]
pub struct DetailEdge {
    pub edge: EdgeId,
    pub dist_m: f64,
    /// 候補どうしの正規化した割合（双子の組にまとめる前）。
    pub share: f64,
    /// 割り当てた重みと、写しかどうか。
    pub assigned: Option<(f32, bool)>,
}

/// 確認用：クラスタの候補。
#[derive(Debug, Clone, PartialEq)]
pub struct DetailCluster {
    pub cluster: ClusterId,
    pub dist_m: f64,
    pub share: f64,
    pub assigned: Option<f32>,
}

/// 確認用：候補の一覧。
#[derive(Debug, Clone)]
pub struct Detail {
    pub edges: Vec<DetailEdge>,
    pub clusters: Vec<DetailCluster>,
    pub excluded_edges: Vec<EdgeCand>,
    pub excluded_clusters: Vec<ClusterCand>,
}

/// 1件の事故の割り当て。
#[derive(Debug, Clone)]
pub struct Outcome {
    /// `(kind, target, pos_dm)` の昇順。
    pub targets: Vec<AssignTarget>,
    pub unassigned: Option<UnassignedReason>,
    /// 最大の重みの候補までの距離（m）。割り当てがなければ `None`。
    pub nearest_m: Option<f64>,
    pub detail: Option<Detail>,
}

impl Outcome {
    fn unassigned(reason: UnassignedReason) -> Outcome {
        Outcome {
            targets: Vec::new(),
            unassigned: Some(reason),
            nearest_m: None,
            detail: None,
        }
    }

    fn finish(targets: Vec<AssignTarget>, nearest_m: f64, detail: Option<Detail>) -> Outcome {
        let unassigned = targets
            .is_empty()
            .then_some(UnassignedReason::InsufficientWeight);
        Outcome {
            nearest_m: (!targets.is_empty()).then_some(nearest_m),
            targets,
            unassigned,
            detail,
        }
    }
}

/// 割り当てた事故の、最大の重みの候補までの距離の度数分布の区切り（m）。
pub const DISTANCE_BINS_M: [f64; 7] = [5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0];

// ---------------------------------------------------------------------------
// 全件の処理
// ---------------------------------------------------------------------------

/// 入力一式。事故の座標と属性は同じ長さ（事故IDの順）。
pub struct SnapInput<'a> {
    pub graph: &'a GraphParts,
    pub graph_attrs: &'a GraphAttrs,
    pub topology: &'a Topology,
    pub lat: &'a [f64],
    pub lon: &'a [f64],
    pub accident_attrs: &'a [AccidentAttr],
}

/// 全件の処理の結果。
pub struct SnapResult {
    /// `PARAMS` に規則・尤度表・件数を入れた割り当て。
    pub assignment: Assignment,
    pub tables: Tables,
}

/// 事故の座標（十進度）を1e-7度の整数にする。
pub fn accident_coord(lat: f64, lon: f64) -> Coord {
    Coord::from_latlon(LatLon::new(lat, lon))
}

struct ChunkOut {
    nearest_hist: [u64; 8],
    per_accident: Vec<u32>,
    targets: Vec<AssignTarget>,
    unassigned: Vec<UnassignedRecord>,
}

/// すべての事故を割り当てる。同じ入力から同じ結果になる（並列処理でも）。
pub fn snap_accidents(
    input: &SnapInput<'_>,
    params: &AssignParams,
    progress: &dyn Fn(&str),
) -> Result<SnapResult, String> {
    let count = input.accident_attrs.len();
    if input.lat.len() != count || input.lon.len() != count {
        return Err(format!(
            "事故の座標（{}・{}件）と属性（{count}件）の件数が違います",
            input.lat.len(),
            input.lon.len()
        ));
    }
    if input.graph_attrs.ref_idx.len() != input.graph.edges.len()
        || input.graph_attrs.name_idx.len() != input.graph.edges.len()
        || input.graph_attrs.lanes.len() != input.graph.edges.len()
        || input.graph_attrs.width_dm.len() != input.graph.edges.len()
        || input.graph_attrs.layer.len() != input.graph.edges.len()
    {
        return Err("graph_attrs.bin の辺の数が graph.bin と違います".to_owned());
    }
    if input.topology.node_cluster.len() != input.graph.node_ll.len() {
        return Err("topology.bin のノードの数が graph.bin と違います".to_owned());
    }
    if count >= u32::MAX as usize {
        return Err("事故の数がu32に収まりません".to_owned());
    }

    progress("候補検索の索引を作る");
    let index = RoadIndex::build(
        input.graph,
        input.graph_attrs,
        input.topology,
        params.clone(),
    );
    progress(&format!(
        "  辺の格子 {}組、クラスタ {}",
        index.edge_grid_entries(),
        input.topology.clusters.len()
    ));

    let chunk = params.chunk.max(1);
    let chunks = count.div_ceil(chunk);
    let coord_of = |i: usize| accident_coord(input.lat[i], input.lon[i]);

    progress("尤度表のために、唯一の候補の事故を数える");
    let counts = (0..chunks)
        .into_par_iter()
        .map_init(Scratch::default, |scratch, ci| {
            let mut counts = TableCounts::default();
            for i in ci * chunk..((ci + 1) * chunk).min(count) {
                index.observe(coord_of(i), &input.accident_attrs[i], scratch, &mut counts);
            }
            counts
        })
        .reduce(TableCounts::default, |mut a, b| {
            a.merge(&b);
            a
        });
    let tables = Tables::from_counts(counts, params.table_prior_weight);
    progress(&format!(
        "  辺 {}件、クラスタ {}件",
        tables.counts.unique_edge_accidents, tables.counts.unique_cluster_accidents
    ));

    progress("割り当てる");
    let outs: Vec<ChunkOut> = (0..chunks)
        .into_par_iter()
        .map_init(Scratch::default, |scratch, ci| {
            let range = ci * chunk..((ci + 1) * chunk).min(count);
            let mut out = ChunkOut {
                nearest_hist: [0; 8],
                per_accident: Vec::with_capacity(range.len()),
                targets: Vec::new(),
                unassigned: Vec::new(),
            };
            for i in range {
                let outcome = index.assign_one(
                    &tables,
                    coord_of(i),
                    &input.accident_attrs[i],
                    scratch,
                    false,
                );
                out.per_accident.push(outcome.targets.len() as u32);
                if let Some(d) = outcome.nearest_m {
                    let bin = DISTANCE_BINS_M.iter().position(|&hi| d <= hi).unwrap_or(7);
                    out.nearest_hist[bin] += 1;
                }
                if let Some(reason) = outcome.unassigned {
                    out.unassigned.push(UnassignedRecord {
                        accident: i as u32,
                        reason,
                    });
                }
                out.targets.extend(outcome.targets);
            }
            out
        })
        .collect();

    let total_targets: usize = outs.iter().map(|o| o.targets.len()).sum();
    if total_targets >= u32::MAX as usize {
        return Err("割り当ての数がu32に収まりません".to_owned());
    }
    let mut offsets = Vec::with_capacity(count + 1);
    offsets.push(0_u32);
    let mut targets = Vec::with_capacity(total_targets);
    let mut unassigned = Vec::new();
    let mut nearest_hist = [0_u64; 8];
    for mut out in outs {
        for (total, n) in nearest_hist.iter_mut().zip(out.nearest_hist) {
            *total += n;
        }
        for n in &out.per_accident {
            offsets.push(offsets[offsets.len() - 1] + n);
        }
        targets.append(&mut out.targets);
        unassigned.append(&mut out.unassigned);
    }

    let mut by_reason = std::collections::BTreeMap::new();
    for r in &unassigned {
        *by_reason
            .entry(r.reason.code().to_string())
            .or_insert(0_u64) += 1;
    }
    let mirrors = targets.iter().filter(|t| t.is_mirror()).count();
    let mut params_json = params.to_json();
    let object = params_json.as_object_mut().expect("JSONオブジェクト");
    object.insert("accidents".to_owned(), json!(count));
    object.insert("targets".to_owned(), json!(targets.len()));
    object.insert("mirror_targets".to_owned(), json!(mirrors));
    object.insert("unassigned".to_owned(), json!(unassigned.len()));
    object.insert("unassigned_by_reason".to_owned(), json!(by_reason));
    object.insert("edges".to_owned(), json!(input.graph.edges.len()));
    object.insert("clusters".to_owned(), json!(input.topology.clusters.len()));
    object.insert("tables".to_owned(), tables.to_json());
    let mut labels: Vec<String> = Vec::new();
    let mut low = 0.0;
    for hi in DISTANCE_BINS_M {
        labels.push(format!("{low}-{hi}"));
        low = hi;
    }
    labels.push(format!("{low}-"));
    object.insert(
        "assigned_nearest_distance_histogram_m".to_owned(),
        json!(
            labels
                .into_iter()
                .zip(nearest_hist)
                .collect::<std::collections::BTreeMap<_, _>>()
        ),
    );
    Ok(SnapResult {
        assignment: Assignment {
            params: params_json,
            offsets,
            targets,
            unassigned,
        },
        tables,
    })
}

#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod tests;
