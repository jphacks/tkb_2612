//! P3：吸着可能な辺、交差点クラスタ、上下線の双子、歩道と親の車道の対応（形式書10章、計画書4章）。
//!
//! 入力は P1 の `graph.bin`（`GraphParts`）と `graph_attrs.bin`（`ref`・`name`）。
//!
//! # 判定の規則（形式書10章が「P3で決める」とした部分）
//!
//! - 吸着可能な辺：手段ごとの有向グラフ（`access` のビットの向き）の強連結成分のうち、ノード数が
//!   `scc_min_nodes`（200）以上のものに、辺の両端が入る辺。ただし、その手段で通過できないノード
//!   （`NODEFLG` の障害物。柵・ゲートなど）は通り抜けられないものとして強連結成分を求める（障害物の
//!   ノードは、どの成分にもつなげず、単独の成分として数える）。障害物のノードを端点に持つ辺は、障害物は
//!   出発・到着の辺の端点としては使えるので、次の条件を満たせば吸着可能にする（形式書10章の「両端が
//!   強連結成分に入る」を補った規則。`topology.bin` の `PARAMS.config.snap_barrier_rule` に記録する）。
//!   - もう一方の端点が、しきい値以上の成分に入っている。両端が障害物の辺は吸着不可。
//!   - 辺が両方向に通れる。出発の辺は、障害物のノードの側へは進めず、到着の辺は、障害物のノードの側から
//!     入れないので、一方通行の辺は、障害物の側から出るだけか、障害物へ入るだけになり、出発か到着の
//!     どちらかに使えない（探索が到達できずに全体を調べ尽くすのを避ける）。
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
use crate::domain::routing::prep::geom::{LocalFrame, Sample, XyLine};
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
            "snap_barrier_rule": SNAP_BARRIER_RULE,
        })
    }
}

/// `PARAMS.config.snap_barrier_rule` に書く、吸着可能な辺の判定への補足（モジュールの説明を参照）。
pub const SNAP_BARRIER_RULE: &str = "scc-without-barrier-nodes; edge touching a barrier node is snappable \
     if the other endpoint is in a large component and the edge is passable in both directions";

const CAR_ANY: u8 = access::CAR_FWD | access::CAR_BWD;

// ---------------------------------------------------------------------------
// 吸着可能な辺（強連結成分）
// ---------------------------------------------------------------------------

/// その手段で通過できないノード（`NODEFLG` の障害物）か。
fn blocked(view: &GraphView<'_>, v: NodeId, mode: Mode) -> bool {
    view.node_flags(v) & node_flags::block_bit(mode) != 0
}

/// 手段ごとの有向グラフの強連結成分を求め、ノードごとに「属する成分のノード数」を返す。
/// 反復版のTarjan法。その手段で通過できないノード（`NODEFLG` の障害物）は通り抜けられないものとして、
/// どの成分にもつなげず、単独の成分（ノード数1）にする。
pub fn component_sizes(view: &GraphView<'_>, mode: Mode) -> Vec<u32> {
    component_sizes_with(view, mode, false)
}

/// [`component_sizes`] の本体。`through_barriers` が真なら、障害物のノードも通り抜けられるものとして
/// 求める（障害物を見ない旧い規則。監査で、障害物の規則の効果を数えるために使う）。
pub fn component_sizes_with(view: &GraphView<'_>, mode: Mode, through_barriers: bool) -> Vec<u32> {
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
        if !through_barriers && blocked(view, root, mode) {
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
                if !through_barriers && blocked(view, w, mode) {
                    continue;
                }
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
    // 障害物のノードは訪れていないので、単独の成分（ノード数1）にする
    comp.iter()
        .map(|&c| if c == UNVISITED { 1 } else { sizes[c as usize] })
        .collect()
}

struct SnapResult {
    bits: Vec<u64>,
    audit: Value,
}

/// 辺 `e` が吸着可能か（モジュールの説明の規則）。`sizes` は障害物を通り抜けられないとして求めた成分の大きさ。
fn edge_snappable(
    view: &GraphView<'_>,
    sizes: &[u32],
    e: EdgeId,
    mode: Mode,
    min_nodes: u32,
) -> bool {
    let record = view.edge(e);
    let large = |v: NodeId| sizes[v as usize] >= min_nodes;
    match (
        blocked(view, record.tail, mode),
        blocked(view, record.head, mode),
    ) {
        (false, false) => large(record.tail) && large(record.head),
        (true, true) => false,
        (blocked_tail, _) => {
            let other = if blocked_tail {
                record.head
            } else {
                record.tail
            };
            large(other) && record.access & access::both(mode) == access::both(mode)
        }
    }
}

fn snap_bits(view: &GraphView<'_>, mode: Mode, min_nodes: u32) -> SnapResult {
    let sizes = component_sizes(view, mode);
    // 障害物を見ない旧い規則での吸着可能な辺の数（監査用。効果の大きさを記録する）
    let sizes_through = component_sizes_with(view, mode, true);
    let edges = view.edge_count();
    let mut bits = vec![0u64; edges.div_ceil(64)];
    let (mut passable, mut snappable, mut snappable_through) = (0u64, 0u64, 0u64);
    let (mut touching_barrier, mut snappable_touching_barrier) = (0u64, 0u64);
    for e in 0..edges {
        if !view.edge_has_mode(e as u32, mode) {
            continue;
        }
        passable += 1;
        let record = view.edge(e as u32);
        if sizes_through[record.tail as usize] >= min_nodes
            && sizes_through[record.head as usize] >= min_nodes
        {
            snappable_through += 1;
        }
        let touches = blocked(view, record.tail, mode) || blocked(view, record.head, mode);
        touching_barrier += u64::from(touches);
        if edge_snappable(view, &sizes, e as u32, mode, min_nodes) {
            bits[e / 64] |= 1 << (e % 64);
            snappable += 1;
            snappable_touching_barrier += u64::from(touches);
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
        // 障害物を見ない旧い規則（両端が成分に入る）で数えた吸着可能な辺。差が、障害物の規則で外れた辺
        "edges_snappable_if_barriers_ignored": snappable_through,
        "edges_touching_barrier_nodes": touching_barrier,
        "edges_snappable_touching_barrier_nodes": snappable_touching_barrier,
        // 小数6桁に丸める（JSONの浮動小数の往復で最後の桁がずれないように）
        "snappable_ratio": if passable == 0 { 0.0 } else { (snappable as f64 / passable as f64 * 1e6).round() / 1e6 },
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
    let mut crossing: Vec<bool> = (0..n as u32)
        .map(|v| view.node_flags(v) & node_flags::CROSSING != 0)
        .collect();
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
fn opposite_fraction(samples_a: &[Sample], line_b: &XyLine, gap: f64, cos_limit: f64) -> f64 {
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
            for (point, _) in &samples_a {
                grid.near(frame.coord(*point), &mut near);
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
                grid.near(frame.coord(*point), &mut near);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::geo::METERS_PER_DEGREE;
    use crate::domain::routing::graph::Graph;
    use crate::domain::routing::graph::synthetic::{EdgeSpec, GraphBuilder};
    use serde_json::Value;

    const LAT0: f64 = 35.0;
    const LON0: f64 = 139.0;

    fn node(b: &mut GraphBuilder, north_m: f64, east_m: f64) -> u32 {
        let cos = LAT0.to_radians().cos();
        b.add_node_deg(
            LAT0 + north_m / METERS_PER_DEGREE,
            LON0 + east_m / (METERS_PER_DEGREE * cos),
        )
    }

    fn road(b: &mut GraphBuilder, a: u32, c: u32) -> u32 {
        b.add_edge(
            EdgeSpec::new(a, c)
                .access(CAR_ANY)
                .class(HighwayClass::Residential),
        )
    }

    fn attrs(graph: &Graph, names: &[(u32, u32)]) -> GraphAttrs {
        // 文字列表：オフセット1が"a"、3が"b"
        let edges = graph.edge_count();
        let mut name_idx = vec![0u32; edges];
        let mut ref_idx = vec![0u32; edges];
        for &(e, name) in names {
            name_idx[e as usize] = name;
        }
        ref_idx.truncate(edges);
        GraphAttrs {
            params: Value::Null,
            way_id: (0..edges as i64).collect(),
            node_id: (0..graph.node_count() as i64).collect(),
            ref_idx,
            name_idx,
            strtab: vec![0, b'a', 0, b'b', 0],
            lanes: vec![0; edges],
            width_dm: vec![0; edges],
            layer: vec![0; edges],
        }
    }

    fn config() -> TopologyConfig {
        TopologyConfig {
            scc_min_nodes: 3,
            ..TopologyConfig::default()
        }
    }

    fn bit(bits: &[u64], e: u32) -> bool {
        bits[e as usize / 64] >> (e % 64) & 1 == 1
    }

    #[test]
    fn snap_bits_use_strongly_connected_components() {
        let mut b = GraphBuilder::new();
        // 4ノードの環（両方向）：成分のノード数4
        let ring: Vec<u32> = (0..4)
            .map(|i| node(&mut b, 0.0, f64::from(i) * 30.0))
            .collect();
        let ring_edges: Vec<u32> = (0..4)
            .map(|i| road(&mut b, ring[i], ring[(i + 1) % 4]))
            .collect();
        // 孤立した2ノードの小さな成分（しきい値3未満）
        let (s0, s1) = (node(&mut b, 200.0, 0.0), node(&mut b, 200.0, 30.0));
        let small = road(&mut b, s0, s1);
        // 環から出るだけの一方通行の線 r0 → c0 → c1：弱連結だが強連結ではない
        let (c0, c1) = (node(&mut b, -100.0, 0.0), node(&mut b, -100.0, 30.0));
        let out0 = b.add_edge(EdgeSpec::new(ring[0], c0).access(access::CAR_FWD));
        let out1 = b.add_edge(EdgeSpec::new(c0, c1).access(access::CAR_FWD));
        // 歩行者だけが通れる辺は、carの対象にならない
        let walk_only =
            b.add_edge(EdgeSpec::new(ring[1], ring[2]).access(access::WALK_FWD | access::WALK_BWD));
        let g = b.build();
        let view = GraphView::new(g.parts());

        let sizes = component_sizes(&view, Mode::Car);
        assert_eq!(sizes[ring[0] as usize], 4);
        assert_eq!(sizes[s0 as usize], 2);
        assert_eq!(sizes[c0 as usize], 1);

        let car = snap_bits(&view, Mode::Car, 3);
        for &e in &ring_edges {
            assert!(bit(&car.bits, e), "環の辺 {e}");
        }
        assert!(!bit(&car.bits, small));
        assert!(!bit(&car.bits, out0)); // 終点が成分の外
        assert!(!bit(&car.bits, out1));
        assert!(!bit(&car.bits, walk_only));
        assert_eq!(car.audit["edges_passable"], 7);
        assert_eq!(car.audit["edges_snappable"], 4);
        // しきい値を1にすれば、小さな成分や一方通行の線も吸着できる
        let car1 = snap_bits(&view, Mode::Car, 1);
        assert!(bit(&car1.bits, small) && bit(&car1.bits, out0) && bit(&car1.bits, out1));
        assert!(!bit(&car1.bits, walk_only));
        // walkは歩行者だけの辺（ring[1]-ring[2]）しか通れない。2ノードの成分なので、しきい値2なら吸着できる
        let walk = snap_bits(&view, Mode::Walk, 2);
        assert!(bit(&walk.bits, walk_only));
    }

    /// 障害物（通過できないノード）の向こう側の小さな領域は、吸着先にならない。
    /// 障害物を無視すると、全体が1つの成分になって吸着可能に見える。
    #[test]
    fn pockets_behind_barrier_nodes_are_not_snappable() {
        let mut b = GraphBuilder::new();
        // 本線の環：6ノード（しきい値5以上）
        let main: Vec<u32> = (0..6)
            .map(|i| node(&mut b, 0.0, f64::from(i) * 30.0))
            .collect();
        let main_edges: Vec<u32> = (0..6)
            .map(|i| road(&mut b, main[i], main[(i + 1) % 6]))
            .collect();
        // 本線の環のノードから、障害物のノード bn（carは通過できない）を経て、4ノードの環（ポケット）へ
        let bn = node(&mut b, 100.0, 0.0);
        let gate = road(&mut b, main[0], bn);
        let pocket: Vec<u32> = (0..4)
            .map(|i| node(&mut b, 200.0, f64::from(i) * 30.0))
            .collect();
        let entry = road(&mut b, bn, pocket[0]);
        let pocket_edges: Vec<u32> = (0..4)
            .map(|i| road(&mut b, pocket[i], pocket[(i + 1) % 4]))
            .collect();
        b.add_node_flags(bn, node_flags::block_bit(Mode::Car));
        let g = b.build();
        let view = GraphView::new(g.parts());

        // 障害物を通り抜けられるとすると、全体（6+1+4ノード）が1つの成分
        assert!(
            component_sizes_with(&view, Mode::Car, true)
                .iter()
                .all(|&s| s == 11)
        );
        // 通り抜けられないとすると、本線・障害物・ポケットに分かれる
        let sizes = component_sizes(&view, Mode::Car);
        assert_eq!(sizes[main[3] as usize], 6);
        assert_eq!(sizes[bn as usize], 1);
        assert_eq!(sizes[pocket[2] as usize], 4);

        let car = snap_bits(&view, Mode::Car, 5);
        for &e in &main_edges {
            assert!(bit(&car.bits, e), "本線の辺 {e}");
        }
        // 障害物のノードを端点に持つ辺：もう一方が大きい成分で、両方向に通れるので吸着可能
        assert!(bit(&car.bits, gate));
        // 障害物の向こうの辺は、吸着不可（ポケットが小さい）
        assert!(!bit(&car.bits, entry));
        for &e in &pocket_edges {
            assert!(!bit(&car.bits, e), "ポケットの辺 {e}");
        }
        assert_eq!(car.audit["edges_passable"], 12);
        assert_eq!(car.audit["edges_snappable"], 7);
        assert_eq!(car.audit["edges_snappable_if_barriers_ignored"], 12);
        assert_eq!(car.audit["edges_touching_barrier_nodes"], 2);
        assert_eq!(car.audit["edges_snappable_touching_barrier_nodes"], 1);
    }

    /// 障害物は手段ごと：carが通過できないノードでも、walk・bikeは通り抜けられる。
    #[test]
    fn barrier_nodes_only_affect_the_blocked_mode() {
        let mut b = GraphBuilder::new();
        let main: Vec<u32> = (0..6)
            .map(|i| node(&mut b, 0.0, f64::from(i) * 30.0))
            .collect();
        for i in 0..6 {
            b.add_edge(EdgeSpec::new(main[i], main[(i + 1) % 6]));
        }
        let bn = node(&mut b, 100.0, 0.0);
        b.add_edge(EdgeSpec::new(main[0], bn));
        let pocket: Vec<u32> = (0..4)
            .map(|i| node(&mut b, 200.0, f64::from(i) * 30.0))
            .collect();
        let entry = b.add_edge(EdgeSpec::new(bn, pocket[0]));
        for i in 0..4 {
            b.add_edge(EdgeSpec::new(pocket[i], pocket[(i + 1) % 4]));
        }
        b.add_node_flags(bn, node_flags::block_bit(Mode::Car));
        let g = b.build();
        let view = GraphView::new(g.parts());
        assert!(!bit(&snap_bits(&view, Mode::Car, 5).bits, entry));
        for mode in [Mode::Bike, Mode::Walk] {
            let r = snap_bits(&view, mode, 5);
            assert!(bit(&r.bits, entry), "{mode:?}");
            assert_eq!(r.audit["edges_snappable"], 12, "{mode:?}");
            assert_eq!(r.audit["edges_touching_barrier_nodes"], 0, "{mode:?}");
        }
    }

    /// 両端が障害物の辺と、障害物のノードの側からしか出られない・障害物のノードへしか入れない一方通行の辺は、
    /// 吸着不可（出発か到着のどちらかに使えず、探索が到達できない）。
    #[test]
    fn edges_between_barriers_and_one_way_edges_next_to_a_barrier_are_not_snappable() {
        let mut b = GraphBuilder::new();
        let ring: Vec<u32> = (0..4)
            .map(|i| node(&mut b, 0.0, f64::from(i) * 30.0))
            .collect();
        for i in 0..4 {
            road(&mut b, ring[i], ring[(i + 1) % 4]);
        }
        let block = |b: &mut GraphBuilder, north: f64, east: f64| {
            let v = node(b, north, east);
            b.add_node_flags(v, node_flags::block_bit(Mode::Car));
            v
        };
        let (g1, g2) = (block(&mut b, 100.0, 0.0), block(&mut b, 100.0, 30.0));
        let two_way = road(&mut b, ring[0], g1);
        let between = road(&mut b, g1, g2);
        let into_barrier = b.add_edge(EdgeSpec::new(ring[1], g2).access(access::CAR_FWD));
        let g3 = block(&mut b, -100.0, 0.0);
        let out_of_barrier = b.add_edge(EdgeSpec::new(g3, ring[2]).access(access::CAR_FWD));
        let g = b.build();
        let view = GraphView::new(g.parts());
        let car = snap_bits(&view, Mode::Car, 3);
        assert!(bit(&car.bits, two_way));
        assert!(!bit(&car.bits, between));
        assert!(!bit(&car.bits, into_barrier));
        assert!(!bit(&car.bits, out_of_barrier));
    }

    #[test]
    fn params_record_the_barrier_rule() {
        let mut b = GraphBuilder::new();
        let n: Vec<u32> = (0..3)
            .map(|i| node(&mut b, 0.0, f64::from(i) * 30.0))
            .collect();
        road(&mut b, n[0], n[1]);
        road(&mut b, n[1], n[2]);
        road(&mut b, n[2], n[0]);
        let g = b.build();
        let t = build_topology(g.parts(), &attrs(&g, &[]), &config()).unwrap();
        assert_eq!(t.params["config"]["snap_barrier_rule"], SNAP_BARRIER_RULE);
        let car = &t.params["snap"]["car"];
        assert_eq!(car["edges_snappable"], 3);
        assert_eq!(car["edges_snappable_if_barriers_ignored"], 3);
        assert_eq!(car["edges_touching_barrier_nodes"], 0);
    }

    #[test]
    fn surplus_bits_after_the_last_edge_stay_zero() {
        let mut b = GraphBuilder::new();
        let n: Vec<u32> = (0..3)
            .map(|i| node(&mut b, 0.0, f64::from(i) * 30.0))
            .collect();
        road(&mut b, n[0], n[1]);
        road(&mut b, n[1], n[2]);
        road(&mut b, n[2], n[0]);
        let g = b.build();
        let t = build_topology(g.parts(), &attrs(&g, &[]), &config()).unwrap();
        assert_eq!(t.snap_car, vec![0b111]);
        t.validate(&g).unwrap();
    }

    #[test]
    fn clusters_merge_close_intersections_and_pull_in_crossings() {
        let mut b = GraphBuilder::new();
        // 交差点A：A1とA2が15m離れていて、それぞれ3方向
        let a1 = node(&mut b, 0.0, 0.0);
        let a2 = node(&mut b, 0.0, 15.0);
        let n1 = node(&mut b, 100.0, 0.0);
        let w1 = node(&mut b, 0.0, -100.0);
        let n2 = node(&mut b, 100.0, 15.0);
        let e2 = node(&mut b, 0.0, 115.0);
        // A1の西の枝の10m先に横断歩道のノード、A2の東の枝の30m先にも横断歩道のノード
        let x_near = node(&mut b, 0.0, -10.0);
        let x_far = node(&mut b, 0.0, 45.0);
        b.add_node_flags(x_near, node_flags::CROSSING);
        b.add_node_flags(x_far, node_flags::CROSSING);
        b.add_node_flags(a1, node_flags::SIGNAL);
        road(&mut b, a1, a2);
        b.add_edge(
            EdgeSpec::new(a1, n1)
                .access(CAR_ANY)
                .class(HighwayClass::Primary),
        );
        road(&mut b, a1, x_near);
        road(&mut b, x_near, w1);
        road(&mut b, a2, n2);
        road(&mut b, a2, x_far);
        road(&mut b, x_far, e2);
        // 交差点B：B1とB2が25m離れている（別のクラスタ）
        let b1 = node(&mut b, 500.0, 0.0);
        let b2 = node(&mut b, 500.0, 25.0);
        let leaves: Vec<u32> = [(600.0, 0.0), (500.0, -100.0), (600.0, 25.0), (500.0, 125.0)]
            .iter()
            .map(|&(n, e)| node(&mut b, n, e))
            .collect();
        road(&mut b, b1, b2);
        road(&mut b, b1, leaves[0]);
        road(&mut b, b1, leaves[1]);
        road(&mut b, b2, leaves[2]);
        road(&mut b, b2, leaves[3]);
        let g = b.build();
        let view = GraphView::new(g.parts());
        let c = build_clusters(&view, &TopologyConfig::default()).unwrap();

        let cluster = |n: u32| c.node_cluster[n as usize];
        // クラスタ番号は最小のノード番号の昇順：A（a1=0）、B1、B2
        assert_eq!(c.records.len(), 3);
        assert_eq!((cluster(a1), cluster(a2), cluster(x_near)), (0, 0, 0));
        assert_eq!(cluster(x_far), NONE);
        assert_eq!(cluster(w1), NONE);
        assert_ne!(cluster(b1), cluster(b2));
        assert!(cluster(b1) != NONE && cluster(b2) != NONE);
        assert_eq!(c.audit["crossing_nodes_added"], 1);

        let a = &c.records[0];
        assert_eq!(a.node_count, 3);
        assert_eq!(&c.nodes[a.node_start as usize..][..3], &[a1, a2, x_near]);
        // 外から接する枝：a1-n1、x_near-w1、a2-n2、a2-x_far
        assert_eq!(a.branches, 4);
        assert_eq!(a.top_class, HighwayClass::Primary.code());
        assert_eq!(a.flags & cluster_flags::SIGNAL, cluster_flags::SIGNAL);
        assert_eq!(a.flags & cluster_flags::ROUNDABOUT, 0);
        // 中心は所属ノードの平均
        let sum: i64 = [a1, a2, x_near]
            .iter()
            .map(|&n| i64::from(g.node_coord(n).lon_e7))
            .sum();
        let mean_lon = (sum as f64 / 3.0).round() as i32;
        assert_eq!(a.lon_e7, mean_lon);
        let bb = &c.records[cluster(b1) as usize];
        assert_eq!((bb.node_count, bb.branches, bb.flags), (1, 3, 0));
    }

    #[test]
    fn roundabout_edges_set_the_cluster_flag() {
        let mut b = GraphBuilder::new();
        let ring: Vec<u32> = (0..3)
            .map(|i| {
                let a = f64::from(i) * 2.0943951;
                node(&mut b, 8.0 * a.cos(), 8.0 * a.sin())
            })
            .collect();
        let arms: Vec<u32> = (0..3)
            .map(|i| {
                let a = f64::from(i) * 2.0943951;
                node(&mut b, 80.0 * a.cos(), 80.0 * a.sin())
            })
            .collect();
        for i in 0..3 {
            b.add_edge(
                EdgeSpec::new(ring[i], ring[(i + 1) % 3])
                    .access(access::CAR_FWD)
                    .flags(edge_flags::ROUNDABOUT),
            );
            road(&mut b, ring[i], arms[i]);
        }
        let g = b.build();
        let c = build_clusters(&GraphView::new(g.parts()), &TopologyConfig::default()).unwrap();
        assert_eq!(c.records.len(), 1);
        assert_eq!(c.records[0].node_count, 3);
        assert_eq!(c.records[0].branches, 3);
        assert_eq!(
            c.records[0].flags & cluster_flags::ROUNDABOUT,
            cluster_flags::ROUNDABOUT
        );
    }

    /// 東西に延びる道路。`east_to_west` なら進行方向が西向き。
    fn lane(
        b: &mut GraphBuilder,
        north_m: f64,
        from_east: f64,
        to_east: f64,
        forward_only: bool,
    ) -> u32 {
        let (p, q) = (node(b, north_m, from_east), node(b, north_m, to_east));
        let spec = EdgeSpec::new(p, q)
            .access(if forward_only {
                access::CAR_FWD
            } else {
                CAR_ANY
            })
            .class(HighwayClass::Primary);
        b.add_edge(spec)
    }

    #[test]
    fn twins_need_opposite_parallel_close_one_ways_with_the_same_name() {
        let mut b = GraphBuilder::new();
        let eb = lane(&mut b, 0.0, 0.0, 200.0, true); // 東行き
        let wb = lane(&mut b, 20.0, 200.0, 0.0, true); // 西行き、20m北：双子
        let same_dir = lane(&mut b, 40.0, 0.0, 200.0, true); // 東行き：同じ向き
        let other_name = lane(&mut b, -20.0, 200.0, 0.0, true); // 西行きだが別の名前
        let far = lane(&mut b, 150.0, 200.0, 0.0, true); // 西行き、150m北：遠い
        let two_way = lane(&mut b, -60.0, 0.0, 200.0, false); // 両方向：対象外
        let g = b.build();
        let a = attrs(
            &g,
            &[
                (eb, 1),
                (wb, 1),
                (same_dir, 1),
                (other_name, 3),
                (far, 1),
                (two_way, 1),
            ],
        );
        let view = GraphView::new(g.parts());
        let t = find_twins(&view, &a, &TopologyConfig::default());
        // eb の最良は wb（20m）、wb の最良は eb。far・same_dir は条件外
        assert_eq!(
            t.pairs,
            vec![TwinPair { a: eb, b: wb }, TwinPair { a: wb, b: eb }]
        );
        assert_eq!(t.audit["twin_pairs"], 1);
    }

    #[test]
    fn twins_pair_the_best_overlap_only_and_cut_edges_still_match() {
        let mut b = GraphBuilder::new();
        // 東行きの長い辺1本に対して、西行きは前半・後半の2辺に分かれている
        let long = lane(&mut b, 0.0, 0.0, 300.0, true);
        let half_a = lane(&mut b, 20.0, 150.0, 0.0, true);
        let half_b = lane(&mut b, 20.0, 300.0, 150.0, true);
        let g = b.build();
        let a = attrs(&g, &[(long, 1), (half_a, 1), (half_b, 1)]);
        let t = find_twins(&GraphView::new(g.parts()), &a, &TopologyConfig::default());
        // 重なりは短い方に対する割合なので、どちらの半分もlongと同じ重なり。長い辺は1本としか組めない：
        // 同点なら辺番号の小さい方（half_a）
        assert_eq!(
            t.pairs,
            vec![
                TwinPair { a: long, b: half_a },
                TwinPair { a: half_a, b: long }
            ]
        );
    }

    #[test]
    fn a_short_partner_in_the_middle_of_a_long_edge_is_found() {
        let mut b = GraphBuilder::new();
        let long = lane(&mut b, 0.0, 0.0, 1000.0, true);
        let short = lane(&mut b, 25.0, 650.0, 350.0, true);
        let g = b.build();
        let a = attrs(&g, &[(long, 1), (short, 1)]);
        let t = find_twins(&GraphView::new(g.parts()), &a, &TopologyConfig::default());
        assert_eq!(
            t.pairs,
            vec![
                TwinPair { a: long, b: short },
                TwinPair { a: short, b: long }
            ]
        );
    }

    #[test]
    fn a_long_sidewalk_finds_its_parent_in_the_middle() {
        let mut b = GraphBuilder::new();
        let main = lane(&mut b, 0.0, 0.0, 600.0, false);
        let s = sidewalk(&mut b, 5.0, 0.0, 600.0);
        let g = b.build();
        let t = find_sidewalk_parents(&GraphView::new(g.parts()), &TopologyConfig::default());
        assert_eq!(
            t.parents,
            vec![SidewalkParent {
                sidewalk: s,
                parent: main
            }]
        );
    }

    fn sidewalk(b: &mut GraphBuilder, north_m: f64, from_east: f64, to_east: f64) -> u32 {
        let (p, q) = (node(b, north_m, from_east), node(b, north_m, to_east));
        b.add_edge(
            EdgeSpec::new(p, q)
                .access(access::WALK_FWD | access::WALK_BWD)
                .class(HighwayClass::Footway)
                .flags(edge_flags::SIDEWALK),
        )
    }

    #[test]
    fn sidewalk_parent_is_the_close_parallel_carriageway() {
        let mut b = GraphBuilder::new();
        let main = lane(&mut b, 0.0, 0.0, 200.0, false);
        let other = lane(&mut b, 12.0, 0.0, 200.0, false); // 平行だが歩道から遠い道路（8m先に歩道がある）
        let s_main = sidewalk(&mut b, -4.0, 20.0, 180.0); // mainの4m南
        let s_far = sidewalk(&mut b, 400.0, 20.0, 180.0); // 道路から遠い（親なし）
        // 道路と斜めに交わる歩道（方位の差が大きい）
        let diag = {
            let (p, q) = (node(&mut b, -5.0, 20.0), node(&mut b, -14.0, 40.0));
            b.add_edge(
                EdgeSpec::new(p, q)
                    .access(access::WALK_FWD | access::WALK_BWD)
                    .class(HighwayClass::Footway)
                    .flags(edge_flags::SIDEWALK),
            )
        };
        // 歩道のフラグがない歩行者用の辺は対象外
        let plain = {
            let (p, q) = (node(&mut b, -3.0, 20.0), node(&mut b, -3.0, 180.0));
            b.add_edge(
                EdgeSpec::new(p, q)
                    .access(access::WALK_FWD | access::WALK_BWD)
                    .class(HighwayClass::Footway),
            )
        };
        let g = b.build();
        let s = find_sidewalk_parents(&GraphView::new(g.parts()), &TopologyConfig::default());
        let parents: Vec<_> = s.parents.iter().map(|p| (p.sidewalk, p.parent)).collect();
        // s_main は最寄りの main（4m）。other は12m先でさらに遠い
        assert_eq!(parents, vec![(s_main, main)]);
        assert_eq!(s.audit["sidewalk_edges"], 3);
        assert_eq!(s.audit["sidewalk_edges_without_parent"], 2);
        let _ = (other, s_far, diag, plain);
    }

    #[test]
    fn full_topology_round_trips_and_validates() {
        let mut b = GraphBuilder::new();
        let eb = lane(&mut b, 0.0, 0.0, 200.0, true);
        let wb = lane(&mut b, 20.0, 200.0, 0.0, true);
        let s = sidewalk(&mut b, -4.0, 20.0, 180.0);
        let g = b.build();
        let a = attrs(&g, &[(eb, 1), (wb, 1)]);
        let t = build_topology(g.parts(), &a, &config()).unwrap();
        t.validate(&g).unwrap();
        assert_eq!(t.twin_of(eb), Some(wb));
        assert_eq!(t.twin_of(wb), Some(eb));
        assert_eq!(t.sidewalk_parent(s), Some(eb));
        assert_eq!(t.params["nodes"], g.node_count());
        assert_eq!(t.params["edges"], g.edge_count());
    }
}
