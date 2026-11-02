//! P3：吸着可能な辺、交差点クラスタ、上下線の双子、歩道と親の車道の対応（形式書10章、計画書4章）。
//!
//! 入力は P1 の `graph.bin`（`GraphParts`）と `graph_attrs.bin`（`ref`・`name`）。
//!
//! # 判定の規則（形式書10章が「P3で決める」とした部分）
//!
//! - 吸着可能な辺：手段ごとの有向グラフ（`access` のビットの向き。ノードの通過不可は無視する）の
//!   強連結成分のうち、ノード数が `scc_min_nodes`（200）以上のものに、辺の両端が入る辺。
//! - 交差点クラスタ：carで通れる辺の次数（接する辺の数。ループは2）が3以上のノードを交差点ノードとし、
//!   長さが `cluster_edge_max_dm`（20m）以下のcarの辺でつながる交差点ノードを1つにまとめる。
//!   横断歩道のノード（`NODEFLG` のビット1、または `CROSSING` の辺の端点）で、carの辺をたどって
//!   最寄りの交差点ノードから20m以内にあるものは、その交差点のクラスタに入れる。
//! - 双子：どちらもcarが一方通行（片方向だけ通れる）の辺 `a`・`b` で、同じ `ref`、または同じ `name`
//!   （`ref` が両方にあって違うものは除く）を持つもの。`a` の進行方向に沿って20mおきに取った点が、
//!   `b` の折れ線から60m以内にあり、その点での `b` の進行方向が `a` の進行方向と逆（角度の差が
//!   150°以上）である割合（`b` から `a` も同様）の小さい方に長さを掛けた「重なり」が、短い方の辺の
//!   長さの6割以上。1つの辺に双子は最大1つで、お互いが最良の相手であるものだけを組にする。
//! - 歩道の親：`SIDEWALK` の辺について、10mおき（長い辺は最大40点）に取った点ごとに、15m以内にあり、
//!   方位の差（向きを問わない）が20°未満の、carで通れる車道の辺（motorway系と `motorroad` を除く）の
//!   うち最寄りのものに1票を入れ、票が点の数の半分以上で最多の辺を親にする（同数は距離の和が小さい方、
//!   さらに同数なら辺番号が小さい方）。

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use rayon::prelude::*;
use serde_json::{Value, json};

use crate::contract::Mode;
use crate::domain::routing::NONE;
use crate::domain::routing::graph::{
    EdgeId, GraphParts, HighwayClass, NodeId, access, edge_flags, node_flags,
};
use crate::domain::routing::prep::attrs::GraphAttrs;
use crate::domain::routing::prep::geom::{LocalFrame, XyLine};
use crate::domain::routing::prep::grid::{EdgeGrid, GridParams};
use crate::domain::routing::prep::view::GraphView;
use crate::domain::routing::topology::{
    ClusterRecord, SidewalkParent, Topology, TwinPair, cluster_flags,
};

/// P3のパラメータ。値は `topology.bin` の `PARAMS` に記録する。
#[derive(Debug, Clone, PartialEq)]
pub struct TopologyConfig {
    /// 吸着先にする強連結成分の最小のノード数（形式書10章の初期値200）。
    pub scc_min_nodes: u32,
    /// 交差点クラスタをつなぐ辺の長さの上限（デシメートル、初期値20m）。
    pub cluster_edge_max_dm: u32,
    /// 双子：2本の辺の間隔の上限（m）。
    pub twin_max_gap_m: f64,
    /// 双子：進行方向が逆向きと認める角度の許容（度）。
    pub twin_max_angle_deg: f64,
    /// 双子：重なりの下限（短い方の辺の長さに対する割合）。
    pub twin_min_overlap: f64,
    /// 双子：点を取る間隔（m）。
    pub twin_sample_step_m: f64,
    /// 歩道：親の車道までの距離の上限（m）。
    pub sidewalk_max_distance_m: f64,
    /// 歩道：親の車道との方位の差の上限（度、向きを問わない）。
    pub sidewalk_max_angle_deg: f64,
    /// 歩道：票を得る点の割合の下限。
    pub sidewalk_min_match: f64,
    /// 歩道：点を取る間隔（m）。
    pub sidewalk_sample_step_m: f64,
}

impl Default for TopologyConfig {
    fn default() -> Self {
        Self {
            scc_min_nodes: 200,
            cluster_edge_max_dm: 200,
            twin_max_gap_m: 60.0,
            twin_max_angle_deg: 30.0,
            twin_min_overlap: 0.6,
            twin_sample_step_m: 20.0,
            sidewalk_max_distance_m: 15.0,
            sidewalk_max_angle_deg: 20.0,
            sidewalk_min_match: 0.5,
            sidewalk_sample_step_m: 10.0,
        }
    }
}

impl TopologyConfig {
    fn to_json(&self) -> Value {
        json!({
            "scc_min_nodes": self.scc_min_nodes,
            "cluster_edge_max_dm": self.cluster_edge_max_dm,
            "twin_max_gap_m": self.twin_max_gap_m,
            "twin_max_angle_deg": self.twin_max_angle_deg,
            "twin_min_overlap": self.twin_min_overlap,
            "twin_sample_step_m": self.twin_sample_step_m,
            "sidewalk_max_distance_m": self.sidewalk_max_distance_m,
            "sidewalk_max_angle_deg": self.sidewalk_max_angle_deg,
            "sidewalk_min_match": self.sidewalk_min_match,
            "sidewalk_sample_step_m": self.sidewalk_sample_step_m,
        })
    }
}

const CAR_ANY: u8 = access::CAR_FWD | access::CAR_BWD;

// ---------------------------------------------------------------------------
// 吸着可能な辺（強連結成分）
// ---------------------------------------------------------------------------

/// 手段ごとの有向グラフの強連結成分を求め、ノードごとに「属する成分のノード数」を返す。
/// 反復版のTarjan法。ノードの通過不可（`NODEFLG`）は無視する。
pub fn component_sizes(view: &GraphView<'_>, mode: Mode) -> Vec<u32> {
    const UNVISITED: u32 = u32::MAX;
    let n = view.node_count();
    let ofs = &view.parts.node_ofs;
    let mut index = vec![UNVISITED; n];
    let mut low = vec![0u32; n];
    let mut comp = vec![UNVISITED; n];
    let mut sizes: Vec<u32> = Vec::new();
    let mut stack: Vec<u32> = Vec::new();
    let mut call: Vec<(u32, u32)> = Vec::new();
    let mut counter = 0u32;

    for root in 0..n as u32 {
        if index[root as usize] != UNVISITED {
            continue;
        }
        index[root as usize] = counter;
        low[root as usize] = counter;
        counter += 1;
        stack.push(root);
        call.push((root, ofs[root as usize]));
        while let Some(&(v, pos)) = call.last() {
            let top = call.len() - 1;
            if pos < ofs[v as usize + 1] {
                call[top].1 += 1;
                let d = view.parts.adj[pos as usize];
                if !view.passable(d, mode) {
                    continue;
                }
                let w = view.dedge_to(d);
                if index[w as usize] == UNVISITED {
                    index[w as usize] = counter;
                    low[w as usize] = counter;
                    counter += 1;
                    stack.push(w);
                    call.push((w, ofs[w as usize]));
                } else if comp[w as usize] == UNVISITED {
                    // スタックに残っているノード
                    low[v as usize] = low[v as usize].min(index[w as usize]);
                }
            } else {
                call.pop();
                if low[v as usize] == index[v as usize] {
                    let id = sizes.len() as u32;
                    let mut size = 0u32;
                    loop {
                        let w = stack.pop().expect("成分の根までスタックにある");
                        comp[w as usize] = id;
                        size += 1;
                        if w == v {
                            break;
                        }
                    }
                    sizes.push(size);
                }
                if let Some(&(parent, _)) = call.last() {
                    low[parent as usize] = low[parent as usize].min(low[v as usize]);
                }
            }
        }
    }
    comp.iter().map(|&c| sizes[c as usize]).collect()
}

struct SnapResult {
    bits: Vec<u64>,
    audit: Value,
}

fn snap_bits(view: &GraphView<'_>, mode: Mode, min_nodes: u32) -> SnapResult {
    let sizes = component_sizes(view, mode);
    let edges = view.edge_count();
    let mut bits = vec![0u64; edges.div_ceil(64)];
    let (mut passable, mut snappable) = (0u64, 0u64);
    for e in 0..edges {
        if !view.edge_has_mode(e as u32, mode) {
            continue;
        }
        passable += 1;
        let record = view.edge(e as u32);
        if sizes[record.tail as usize] >= min_nodes && sizes[record.head as usize] >= min_nodes {
            bits[e / 64] |= 1 << (e % 64);
            snappable += 1;
        }
    }
    let nodes_in_large = sizes.iter().filter(|&&s| s >= min_nodes).count();
    // 成分の数は、各成分の代表（サイズの逆数の和）で数える。サイズ1は孤立ノード
    let mut largest = 0u32;
    let mut large_components = 0.0f64;
    let mut small_components = 0.0f64;
    for &s in &sizes {
        largest = largest.max(s);
        if s >= min_nodes {
            large_components += 1.0 / f64::from(s);
        } else if s >= 2 {
            small_components += 1.0 / f64::from(s);
        }
    }
    let audit = json!({
        "edges_passable": passable,
        "edges_snappable": snappable,
        "snappable_ratio": if passable == 0 { 0.0 } else { snappable as f64 / passable as f64 },
        "nodes_in_large_components": nodes_in_large,
        "large_components": large_components.round() as u64,
        "small_components_size_2_or_more": small_components.round() as u64,
        "largest_component_nodes": largest,
    });
    SnapResult { bits, audit }
}

// ---------------------------------------------------------------------------
// 交差点クラスタ
// ---------------------------------------------------------------------------

struct Clusters {
    node_cluster: Vec<u32>,
    records: Vec<ClusterRecord>,
    nodes: Vec<NodeId>,
    audit: Value,
}

fn find(parent: &mut [u32], mut v: u32) -> u32 {
    while parent[v as usize] != v {
        parent[v as usize] = parent[parent[v as usize] as usize];
        v = parent[v as usize];
    }
    v
}

fn build_clusters(view: &GraphView<'_>, cfg: &TopologyConfig) -> Result<Clusters, String> {
    let n = view.node_count();
    let car_edge = |e: EdgeId| view.edge(e).access & CAR_ANY != 0;

    // 交差点ノード：carで通れる辺の次数が3以上
    let is_intersection: Vec<bool> = (0..n as u32)
        .into_par_iter()
        .map(|v| {
            view.out_dedges(v)
                .iter()
                .filter(|d| car_edge(d.edge()))
                .count()
                >= 3
        })
        .collect();

    // 短いcarの辺でつながる交差点ノードをまとめる（代表は最小のノード番号）
    let mut parent: Vec<u32> = (0..n as u32).collect();
    for e in 0..view.edge_count() as u32 {
        let r = view.edge(e);
        if r.access & CAR_ANY == 0
            || r.length_dm > cfg.cluster_edge_max_dm
            || r.tail == r.head
            || !is_intersection[r.tail as usize]
            || !is_intersection[r.head as usize]
        {
            continue;
        }
        let (a, b) = (find(&mut parent, r.tail), find(&mut parent, r.head));
        if a != b {
            parent[a.max(b) as usize] = a.min(b);
        }
    }
    let mut label: Vec<u32> = vec![NONE; n];
    for v in 0..n as u32 {
        if is_intersection[v as usize] {
            label[v as usize] = find(&mut parent, v);
        }
    }

    // 横断歩道のノード：carの辺をたどって、最寄りの交差点ノードから上限以内にあれば、そのクラスタに入れる
    let mut crossing = vec![false; n];
    for v in 0..n {
        if view.node_flags(v as u32) & node_flags::CROSSING != 0 {
            crossing[v] = true;
        }
    }
    for e in 0..view.edge_count() as u32 {
        let r = view.edge(e);
        if r.flags & edge_flags::CROSSING != 0 {
            crossing[r.tail as usize] = true;
            crossing[r.head as usize] = true;
        }
    }
    let mut heap: BinaryHeap<Reverse<(u32, u32, u32)>> = BinaryHeap::new();
    for v in 0..n as u32 {
        if is_intersection[v as usize] {
            heap.push(Reverse((0, label[v as usize], v)));
        }
    }
    let mut settled: HashMap<u32, (u32, u32)> = HashMap::new();
    while let Some(Reverse((dist, lab, v))) = heap.pop() {
        if settled.contains_key(&v) {
            continue;
        }
        settled.insert(v, (dist, lab));
        for &d in view.out_dedges(v) {
            let r = view.edge(d.edge());
            if r.access & CAR_ANY == 0 {
                continue;
            }
            let w = view.dedge_to(d);
            let nd = dist + r.length_dm;
            if nd <= cfg.cluster_edge_max_dm && !settled.contains_key(&w) {
                heap.push(Reverse((nd, lab, w)));
            }
        }
    }
    let mut crossing_members = 0u64;
    for (&v, &(_, lab)) in &settled {
        if !is_intersection[v as usize] && crossing[v as usize] {
            label[v as usize] = lab;
            crossing_members += 1;
        }
    }

    // クラスタ番号は、所属ノードのうち最小のノード番号の昇順
    let mut cluster_of_label: HashMap<u32, u32> = HashMap::new();
    let mut node_cluster = vec![NONE; n];
    let mut count = 0u32;
    for v in 0..n {
        let lab = label[v];
        if lab == NONE {
            continue;
        }
        let id = *cluster_of_label.entry(lab).or_insert_with(|| {
            count += 1;
            count - 1
        });
        node_cluster[v] = id;
    }
    let cluster_count = count as usize;

    // 所属ノードの表（クラスタごとに昇順）
    let mut start = vec![0u32; cluster_count + 1];
    for &c in &node_cluster {
        if c != NONE {
            start[c as usize + 1] += 1;
        }
    }
    for i in 0..cluster_count {
        start[i + 1] += start[i];
    }
    let mut cursor: Vec<u32> = start[..cluster_count].to_vec();
    let mut nodes = vec![0u32; start[cluster_count] as usize];
    for v in 0..n as u32 {
        let c = node_cluster[v as usize];
        if c != NONE {
            nodes[cursor[c as usize] as usize] = v;
            cursor[c as usize] += 1;
        }
    }

    let mut records = Vec::with_capacity(cluster_count);
    let mut largest = 0usize;
    let mut size_histogram = [0u64; 6]; // 1, 2, 3, 4, 5〜9, 10以上
    for c in 0..cluster_count {
        let members = &nodes[start[c] as usize..start[c + 1] as usize];
        if members.len() > usize::from(u16::MAX) {
            return Err(format!(
                "交差点クラスタ{c}のノード数{}がu16に収まりません",
                members.len()
            ));
        }
        largest = largest.max(members.len());
        size_histogram[match members.len() {
            1 => 0,
            2 => 1,
            3 => 2,
            4 => 3,
            5..=9 => 4,
            _ => 5,
        }] += 1;
        let (mut lat, mut lon) = (0i64, 0i64);
        let mut top_class = u8::MAX;
        let mut branches = 0u32;
        let mut flags = 0u8;
        for &v in members {
            let coord = view.node_coord(v);
            lat += i64::from(coord.lat_e7);
            lon += i64::from(coord.lon_e7);
            if view.node_flags(v) & node_flags::SIGNAL != 0 {
                flags |= cluster_flags::SIGNAL;
            }
            for &d in view.out_dedges(v) {
                let r = view.edge(d.edge());
                if r.access & CAR_ANY == 0 {
                    continue;
                }
                top_class = top_class.min(r.class);
                if r.flags & edge_flags::ROUNDABOUT != 0 {
                    flags |= cluster_flags::ROUNDABOUT;
                }
                if node_cluster[view.dedge_to(d) as usize] != c as u32 {
                    branches += 1;
                }
            }
        }
        let k = members.len() as i64;
        let mean = |sum: i64| ((sum as f64) / (k as f64)).round() as i32;
        records.push(ClusterRecord {
            lat_e7: mean(lat),
            lon_e7: mean(lon),
            node_start: start[c],
            node_count: members.len() as u16,
            top_class,
            branches: branches.min(255) as u8,
            flags,
            pad: [0; 3],
        });
    }
    let signalized = records
        .iter()
        .filter(|r| r.flags & cluster_flags::SIGNAL != 0)
        .count();
    let intersection_nodes = is_intersection.iter().filter(|&&b| b).count();
    let audit = json!({
        "intersection_nodes": intersection_nodes,
        "clusters": cluster_count,
        "cluster_nodes": nodes.len(),
        "crossing_nodes_added": crossing_members,
        "largest_cluster_nodes": largest,
        "size_histogram": {
            "1": size_histogram[0], "2": size_histogram[1], "3": size_histogram[2],
            "4": size_histogram[3], "5_to_9": size_histogram[4], "10_or_more": size_histogram[5],
        },
        "signalized_clusters": signalized,
        "roundabout_clusters": records.iter().filter(|r| r.flags & cluster_flags::ROUNDABOUT != 0).count(),
    });
    Ok(Clusters {
        node_cluster,
        records,
        nodes,
        audit,
    })
}

// ---------------------------------------------------------------------------
// 近傍検索
// ---------------------------------------------------------------------------

fn build_edge_grid(
    view: &GraphView<'_>,
    cell_e7: i32,
    include: impl Fn(EdgeId) -> bool + Sync,
) -> EdgeGrid {
    let params = GridParams::japan(cell_e7, cell_e7);
    const CHUNK: usize = 1 << 15;
    let edges = view.edge_count();
    let chunks: Vec<Vec<(u32, u32)>> = (0..edges.div_ceil(CHUNK))
        .into_par_iter()
        .map(|c| {
            let mut pairs = Vec::new();
            let mut cells = Vec::new();
            for e in c * CHUNK..((c + 1) * CHUNK).min(edges) {
                if !include(e as u32) {
                    continue;
                }
                cells.clear();
                let line = view.polyline(e as u32);
                for w in line.windows(2) {
                    params.segment_cells(w[0], w[1], &mut cells);
                }
                cells.sort_unstable();
                cells.dedup();
                pairs.extend(cells.iter().map(|&k| (k, e as u32)));
            }
            pairs
        })
        .collect();
    let mut pairs = Vec::with_capacity(chunks.iter().map(Vec::len).sum());
    for chunk in chunks {
        pairs.extend(chunk);
    }
    EdgeGrid::from_pairs(params, pairs)
}

/// 進行方向に並べた折れ線（局所平面）。`forward` が偽なら逆向き。
fn travel_line(view: &GraphView<'_>, frame: &LocalFrame, e: EdgeId, forward: bool) -> XyLine {
    let mut coords = view.polyline(e);
    if !forward {
        coords.reverse();
    }
    XyLine::new(frame, &coords)
}

// ---------------------------------------------------------------------------
// 上下線の双子
// ---------------------------------------------------------------------------

/// carが片方向だけ通れる辺の、通れる向き（`true` が始点→終点）。
fn car_one_way(access_bits: u8) -> Option<bool> {
    match (
        access_bits & access::CAR_FWD != 0,
        access_bits & access::CAR_BWD != 0,
    ) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

/// 同じ道路の名前（`ref` または `name`）を持つか。どちらも空の辺は一致しない。
fn same_road(attrs: &GraphAttrs, a: EdgeId, b: EdgeId) -> bool {
    let (ref_a, ref_b) = (attrs.ref_idx[a as usize], attrs.ref_idx[b as usize]);
    let (name_a, name_b) = (attrs.name_idx[a as usize], attrs.name_idx[b as usize]);
    if ref_a != 0 && ref_a == ref_b {
        return true;
    }
    name_a != 0 && name_a == name_b && !(ref_a != 0 && ref_b != 0 && ref_a != ref_b)
}

/// `a` の点のうち、`b` の折れ線から `gap` 以内で進行方向が逆向きのものの割合。
fn opposite_fraction(
    samples_a: &[((f64, f64), (f64, f64))],
    line_b: &XyLine,
    gap: f64,
    cos_limit: f64,
) -> f64 {
    if samples_a.is_empty() {
        return 0.0;
    }
    let matched = samples_a
        .iter()
        .filter(|(p, dir_a)| {
            let (distance, dir_b) = line_b.nearest(*p);
            distance <= gap && dir_b.is_some_and(|b| dir_a.0 * b.0 + dir_a.1 * b.1 <= -cos_limit)
        })
        .count();
    matched as f64 / samples_a.len() as f64
}

struct Twins {
    pairs: Vec<TwinPair>,
    audit: Value,
}

fn find_twins(view: &GraphView<'_>, attrs: &GraphAttrs, cfg: &TopologyConfig) -> Twins {
    let one_way = |e: EdgeId| car_one_way(view.edge(e).access);
    let candidates: Vec<EdgeId> = (0..view.edge_count() as u32)
        .filter(|&e| one_way(e).is_some())
        .collect();
    let named: Vec<EdgeId> = candidates
        .iter()
        .copied()
        .filter(|&e| attrs.ref_idx[e as usize] != 0 || attrs.name_idx[e as usize] != 0)
        .collect();
    let named_set = {
        let mut flags = vec![false; view.edge_count()];
        for &e in &named {
            flags[e as usize] = true;
        }
        flags
    };
    let grid = build_edge_grid(view, 10_000, |e| named_set[e as usize]);
    // 逆向きとみなす内積の上限は -cos(許容角)。許容角30°なら -0.866 以下
    let cos_limit = cfg.twin_max_angle_deg.to_radians().cos();

    // 辺ごとに最良の相手を探す（並列。結果は辺の順）
    let best: Vec<Option<(EdgeId, f64)>> = named
        .par_iter()
        .map(|&a| {
            let a_forward = one_way(a)?;
            let polyline = view.polyline(a);
            let frame = LocalFrame::new(polyline[polyline.len() / 2]);
            let line_a = travel_line(view, &frame, a, a_forward);
            if line_a.length <= 0.0 {
                return None;
            }
            let samples_a = line_a.samples(cfg.twin_sample_step_m);
            // 近くにある、同じ名前の候補
            let mut near = Vec::new();
            let mut found: Vec<EdgeId> = Vec::new();
            let mut seen = Vec::new();
            for coord in &polyline {
                grid.near(*coord, &mut near);
                for &b in &near {
                    if b != a && !seen.contains(&b) {
                        seen.push(b);
                        if same_road(attrs, a, b) && one_way(b).is_some() {
                            found.push(b);
                        }
                    }
                }
            }
            let mut best: Option<(EdgeId, f64)> = None;
            for b in found {
                let Some(b_forward) = one_way(b) else {
                    continue;
                };
                let line_b = travel_line(view, &frame, b, b_forward);
                if line_b.length <= 0.0 {
                    continue;
                }
                let frac_a = opposite_fraction(&samples_a, &line_b, cfg.twin_max_gap_m, cos_limit);
                let samples_b = line_b.samples(cfg.twin_sample_step_m);
                let frac_b = opposite_fraction(&samples_b, &line_a, cfg.twin_max_gap_m, cos_limit);
                let overlap = (frac_a * line_a.length).min(frac_b * line_b.length);
                let score = overlap / line_a.length.min(line_b.length);
                if score >= cfg.twin_min_overlap
                    && best.is_none_or(|(id, s)| score > s || (score == s && b < id))
                {
                    best = Some((b, score));
                }
            }
            best
        })
        .collect();

    // お互いが最良の相手である組だけを採用する
    let mut best_of: HashMap<EdgeId, EdgeId> = HashMap::new();
    for (&a, b) in named.iter().zip(&best) {
        if let Some((b, _)) = b {
            best_of.insert(a, *b);
        }
    }
    let mut pairs: Vec<TwinPair> = Vec::new();
    for (&a, &b) in &best_of {
        if best_of.get(&b) == Some(&a) {
            pairs.push(TwinPair { a, b });
        }
    }
    pairs.sort_unstable_by_key(|t| (t.a, t.b));
    let audit = json!({
        "one_way_car_edges": candidates.len(),
        "one_way_car_edges_with_ref_or_name": named.len(),
        "edges_with_a_candidate": best_of.len(),
        "twin_pairs": pairs.len() / 2,
        "twin_records": pairs.len(),
    });
    Twins { pairs, audit }
}

// ---------------------------------------------------------------------------
// 歩道と親の車道
// ---------------------------------------------------------------------------

struct Sidewalks {
    parents: Vec<SidewalkParent>,
    audit: Value,
}

fn find_sidewalk_parents(view: &GraphView<'_>, cfg: &TopologyConfig) -> Sidewalks {
    let is_parent_road = |e: EdgeId| {
        let r = view.edge(e);
        r.access & CAR_ANY != 0
            && r.flags & edge_flags::MOTORROAD == 0
            && r.class != HighwayClass::Motorway.code()
            && r.class != HighwayClass::MotorwayLink.code()
    };
    let sidewalks: Vec<EdgeId> = (0..view.edge_count() as u32)
        .filter(|&e| view.edge(e).flags & edge_flags::SIDEWALK != 0)
        .collect();
    let grid = build_edge_grid(view, 5_000, is_parent_road);
    let cos_limit = cfg.sidewalk_max_angle_deg.to_radians().cos();

    let found: Vec<Option<EdgeId>> = sidewalks
        .par_iter()
        .map(|&s| {
            let polyline = view.polyline(s);
            let frame = LocalFrame::new(polyline[polyline.len() / 2]);
            let line = XyLine::new(&frame, &polyline);
            if line.length <= 0.0 {
                return None;
            }
            let step = cfg.sidewalk_sample_step_m.max(line.length / 40.0);
            let samples = line.samples(step);
            let mut lines: HashMap<EdgeId, XyLine> = HashMap::new();
            let mut votes: HashMap<EdgeId, (u32, f64)> = HashMap::new();
            let mut near = Vec::new();
            for (point, dir) in &samples {
                // 局所平面の点を座標に戻す代わりに、折れ線の最寄りの頂点でセルを引く
                let coord = nearest_vertex(&polyline, &frame, *point);
                grid.near(coord, &mut near);
                let mut best: Option<(EdgeId, f64)> = None;
                for &p in &near {
                    if p == s {
                        continue;
                    }
                    let parent_line = lines
                        .entry(p)
                        .or_insert_with(|| XyLine::new(&frame, &view.polyline(p)));
                    let (distance, parent_dir) = parent_line.nearest(*point);
                    if distance > cfg.sidewalk_max_distance_m {
                        continue;
                    }
                    let Some(parent_dir) = parent_dir else {
                        continue;
                    };
                    if (dir.0 * parent_dir.0 + dir.1 * parent_dir.1).abs() < cos_limit {
                        continue;
                    }
                    if best.is_none_or(|(id, d)| distance < d || (distance == d && p < id)) {
                        best = Some((p, distance));
                    }
                }
                if let Some((p, distance)) = best {
                    let entry = votes.entry(p).or_insert((0, 0.0));
                    entry.0 += 1;
                    entry.1 += distance;
                }
            }
            let needed = (cfg.sidewalk_min_match * samples.len() as f64)
                .ceil()
                .max(1.0) as u32;
            votes
                .into_iter()
                .filter(|(_, (n, _))| *n >= needed)
                .min_by(|(id_a, (n_a, d_a)), (id_b, (n_b, d_b))| {
                    n_b.cmp(n_a)
                        .then(d_a.partial_cmp(d_b).unwrap_or(std::cmp::Ordering::Equal))
                        .then(id_a.cmp(id_b))
                })
                .map(|(id, _)| id)
        })
        .collect();

    let parents: Vec<SidewalkParent> = sidewalks
        .iter()
        .zip(&found)
        .filter_map(|(&s, p)| {
            p.map(|parent| SidewalkParent {
                sidewalk: s,
                parent,
            })
        })
        .collect();
    let audit = json!({
        "sidewalk_edges": sidewalks.len(),
        "sidewalk_edges_with_parent": parents.len(),
        "sidewalk_edges_without_parent": sidewalks.len() - parents.len(),
        "parent_candidate_edges": grid.entries(),
    });
    Sidewalks { parents, audit }
}

/// 局所平面の点に最も近い折れ線の頂点の座標。格子のセルを引くための近似で、点は折れ線上にあるので
/// 頂点との差は線分の半分以下（数m〜数十m）。セルの大きさが約50mなので、周囲8セルの検索で足りる。
fn nearest_vertex(
    polyline: &[crate::domain::routing::graph::Coord],
    frame: &LocalFrame,
    point: (f64, f64),
) -> crate::domain::routing::graph::Coord {
    polyline
        .iter()
        .copied()
        .min_by(|a, b| {
            let da = {
                let (x, y) = frame.xy(*a);
                (x - point.0).hypot(y - point.1)
            };
            let db = {
                let (x, y) = frame.xy(*b);
                (x - point.0).hypot(y - point.1)
            };
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .expect("折れ線は空でない")
}

// ---------------------------------------------------------------------------
// 全体
// ---------------------------------------------------------------------------

/// `topology.bin` の中身を作る。
pub fn build_topology(
    parts: &GraphParts,
    attrs: &GraphAttrs,
    cfg: &TopologyConfig,
) -> Result<Topology, String> {
    let view = GraphView::new(parts);
    if attrs.way_id.len() != view.edge_count() || attrs.ref_idx.len() != view.edge_count() {
        return Err("graph_attrs.binの辺の数がgraph.binと一致しません".to_owned());
    }
    let car = snap_bits(&view, Mode::Car, cfg.scc_min_nodes);
    let bike = snap_bits(&view, Mode::Bike, cfg.scc_min_nodes);
    let walk = snap_bits(&view, Mode::Walk, cfg.scc_min_nodes);
    let clusters = build_clusters(&view, cfg)?;
    let twins = find_twins(&view, attrs, cfg);
    let sidewalks = find_sidewalk_parents(&view, cfg);

    let params = json!({
        "nodes": view.node_count(),
        "edges": view.edge_count(),
        "config": cfg.to_json(),
        "snap": { "car": car.audit, "bike": bike.audit, "walk": walk.audit },
        "clusters": clusters.audit,
        "twins": twins.audit,
        "sidewalks": sidewalks.audit,
    });
    Ok(Topology {
        params,
        snap_car: car.bits,
        snap_bike: bike.bits,
        snap_walk: walk.bits,
        node_cluster: clusters.node_cluster,
        clusters: clusters.records,
        cluster_nodes: clusters.nodes,
        twins: twins.pairs,
        sidewalk_parents: sidewalks.parents,
    })
}
