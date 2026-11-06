//! 合成グラフの組み立て器（テストと人工releaseのための小さなグラフ）。
//!
//! 形式書6章の不変条件（CSR、`ADJ` の並び、`EDGE`・`SHAPE` の整合、日本矩形）を満たした
//! [`Graph`](super::Graph) を、ノードと辺の指定から作る。`TimeModel::synthetic()` を使う
//! （`GraphBuilder::set_time_model` で替えられる）。右左折禁止（[`Restrictions`]）も同じ builder で作る。
//! 決定的に動く（同じ入力・同じシードなら同じグラフ）。乱数は外部のcrateを使わず、自前の [`Rng`]（xorshift64*）。
//!
//! 想定する利用者：最短時間経路のテスト、探索（M3a）のテスト（ノード12個以下のランダムグラフ600通りでの
//! 全列挙との比較、ノード数千のグラフ）、人工release（M4）。
//!
//! # 使い方
//!
//! 任意の辺の列から作る。座標は [`GraphBuilder::add_node`]（`Coord`）か
//! [`GraphBuilder::add_node_deg`]（十進度）で与える。辺の長さは座標から計算する
//! （形式書1章の規則）。`EdgeSpec::length_dm` で上書きすると手計算の値に揃えられる。
//!
//! ```
//! use accident_route_backend::contract::Mode;
//! use accident_route_backend::domain::routing::graph::node_flags;
//! use accident_route_backend::domain::routing::graph::synthetic::{EdgeSpec, GraphBuilder};
//!
//! let mut b = GraphBuilder::new();
//! let a = b.add_node_deg(35.0, 139.0);
//! let c = b.add_node_deg(35.0, 139.001);
//! let d = b.add_node_deg(35.001, 139.001);
//! b.add_edge(EdgeSpec::new(a, c).length_dm(1000)); // 100 m、全手段・両方向
//! b.add_edge(EdgeSpec::new(c, d).oneway()); // car・bikeは c→d のみ
//! b.add_node_flags(c, node_flags::SIGNAL);
//! b.forbid_nodes(&[a, c, d], Mode::Car.mask() | Mode::Bike.mask()); // a→c→d を禁止（via-node型）
//! let (graph, restrictions) = b.build_all();
//! assert_eq!(graph.node_count(), 3);
//! assert_eq!(restrictions.items.len(), 1);
//! ```
//!
//! 格子（[`grid`]）とランダムグラフ（[`random_graph`]）は builder を返すので、続けてフラグや右左折禁止、
//! 辺を足せる。
//!
//! ```
//! use accident_route_backend::domain::routing::graph::synthetic::{RandomGraphConfig, grid, random_graph};
//!
//! // 3×4の格子（格子間隔100 m）。ノード番号は `shape.node(行, 列)`。
//! let (b, shape) = grid(3, 4, 100.0);
//! let graph = b.build();
//! assert_eq!(graph.node_count(), 12);
//! assert_eq!(shape.node(2, 3), 11);
//!
//! // 同じシードなら同じグラフ。手段ごとの通行可否・一方通行・信号・障害物・右左折禁止つき。
//! let config = RandomGraphConfig::rich(10);
//! let (g1, r1) = random_graph(42, &config).build_all();
//! let (g2, r2) = random_graph(42, &config).build_all();
//! assert_eq!(g1.parts(), g2.parts());
//! assert_eq!(r1, r2);
//! ```

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::contract::Mode;
use crate::contract::geo::{METERS_PER_DEGREE, straight_distance_m};
use crate::domain::routing::graph::restrictions::{Restriction, Restrictions};
use crate::domain::routing::graph::time::{PerMode, TimeModel};
use crate::domain::routing::graph::{
    Coord, DirEdge, EdgeId, EdgeRecord, Graph, GraphError, GraphParts, HighwayClass, NodeId,
    ShapePoint, access, node_flags,
};

/// 3つの手段すべてのビット（`Restriction::modes`、walk=1・bike=2・car=4）。
pub const ALL_MODES_MASK: u8 = 0b111;

/// 格子とランダムグラフの既定の基準点（東京付近、北緯35.6度・東経139.7度）。
pub const ORIGIN: Coord = Coord::new(356_000_000, 1_397_000_000);

/// 辺 `e` の向き0（始点→終点）の有向辺。
pub const fn fwd(e: EdgeId) -> DirEdge {
    DirEdge::new(e, false)
}

/// 辺 `e` の向き1（終点→始点）の有向辺。
pub const fn bwd(e: EdgeId) -> DirEdge {
    DirEdge::new(e, true)
}

// ---------------------------------------------------------------------------------------------
// 擬似乱数
// ---------------------------------------------------------------------------------------------

/// 決定的な擬似乱数（xorshift64*。シードはsplitmix64で拡散するので、0や連続した値でもよい）。
/// 暗号用途ではない。
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Rng {
            state: if z == 0 { 0x9E37_79B9_7F4A_7C15 } else { z },
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// `0..n` の一様な整数（偏りなし）。`n` は1以上。
    pub fn below(&mut self, n: usize) -> usize {
        assert!(n > 0, "Rng::below(0)");
        let n = n as u64;
        let threshold = n.wrapping_neg() % n;
        loop {
            let x = self.next_u64();
            if x >= threshold {
                return (x % n) as usize;
            }
        }
    }

    /// `lo..=hi` の一様な整数。
    pub fn range_i32(&mut self, lo: i32, hi: i32) -> i32 {
        assert!(lo <= hi, "Rng::range_i32({lo}, {hi})");
        let span = (i64::from(hi) - i64::from(lo) + 1) as usize;
        (i64::from(lo) + self.below(span) as i64) as i32
    }

    /// `[0, 1)` の一様な実数。
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1_u64 << 53) as f64
    }

    /// 確率 `p` で真。`p ≤ 0` なら常に偽、`p ≥ 1` なら常に真（乱数は常に1つ消費する）。
    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    /// 要素を並べ替える（Fisher–Yates）。
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i + 1);
            items.swap(i, j);
        }
    }

    /// 空でない列から1つ選ぶ。
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

// ---------------------------------------------------------------------------------------------
// 辺の指定
// ---------------------------------------------------------------------------------------------

/// 辺の指定。`EdgeSpec::new(tail, head)` は、種類 `Residential`、全手段・両方向に通れる、
/// フラグなし、`maxspeed` なし、形状点なし、長さは座標から計算、とする。
/// フィールドは公開で、メソッドでも変えられる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeSpec {
    pub tail: NodeId,
    pub head: NodeId,
    pub class: HighwayClass,
    /// `access` のビット。0にするとグラフの検査（`from_parts`）が失敗する。
    pub access: u8,
    /// `edge_flags` のビット。
    pub flags: u8,
    pub maxspeed_kmh: u8,
    /// `None` なら始点・形状点・終点の折れ線から計算する（形式書1章）。
    pub length_dm: Option<u32>,
    /// 中間の形状点（始点と終点を含まない。絶対座標）。差分が `i16` を超える区間は、
    /// 形式書6.4節のとおり等分する点を足す。
    pub shape: Vec<Coord>,
}

impl EdgeSpec {
    pub fn new(tail: NodeId, head: NodeId) -> EdgeSpec {
        EdgeSpec {
            tail,
            head,
            class: HighwayClass::Residential,
            access: access::ALL,
            flags: 0,
            maxspeed_kmh: 0,
            length_dm: None,
            shape: Vec::new(),
        }
    }

    pub fn class(mut self, class: HighwayClass) -> Self {
        self.class = class;
        self
    }

    /// `access` のビットをそのまま指定する（`access::CAR_FWD` など）。
    pub fn access(mut self, bits: u8) -> Self {
        self.access = bits;
        self
    }

    /// 一方通行（始点→終点）。carとbikeは始点→終点のみ、walkは両方向（形式書7.2節）。
    pub fn oneway(mut self) -> Self {
        self.access = access::ALL & !(access::CAR_BWD | access::BIKE_BWD);
        self
    }

    /// 一方通行（終点→始点）。carとbikeは終点→始点のみ、walkは両方向。
    pub fn reverse_oneway(mut self) -> Self {
        self.access = access::ALL & !(access::CAR_FWD | access::BIKE_FWD);
        self
    }

    /// その手段を両方向とも通れなくする。
    pub fn deny(mut self, mode: Mode) -> Self {
        self.access &= !access::both(mode);
        self
    }

    pub fn flags(mut self, bits: u8) -> Self {
        self.flags = bits;
        self
    }

    pub fn maxspeed(mut self, kmh: u8) -> Self {
        self.maxspeed_kmh = kmh;
        self
    }

    /// 長さをデシメートルで指定する（座標から計算しない）。
    pub fn length_dm(mut self, dm: u32) -> Self {
        self.length_dm = Some(dm);
        self
    }

    /// 中間の形状点を指定する（始点と終点を含まない）。
    pub fn shape(mut self, points: &[Coord]) -> Self {
        self.shape = points.to_vec();
        self
    }
}

// ---------------------------------------------------------------------------------------------
// builder
// ---------------------------------------------------------------------------------------------

/// 右左折禁止の指定。`GraphBuilder::restrictions` で展開する。
#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingRestriction {
    Forbid {
        seq: Vec<DirEdge>,
        modes: u8,
    },
    Only {
        prefix: Vec<DirEdge>,
        allowed: DirEdge,
        modes: u8,
    },
}

/// 合成グラフの組み立て器。ノード・辺・ノードのフラグ・右左折禁止を足してから `build*` で
/// [`Graph`] と [`Restrictions`] を作る。`build*` は `&self` を取るので、同じ builder から繰り返し作れる。
///
/// ノード番号と辺番号は足した順（0から）。`build` は形式書6章のとおり、`ADJ` を各ノードで昇順にし、
/// 始点と終点が同じ辺（ループ）の2つの有向辺をどちらも同じノードに入れる。
#[derive(Debug, Clone)]
pub struct GraphBuilder {
    nodes: Vec<Coord>,
    node_flags: Vec<u8>,
    edges: Vec<EdgeSpec>,
    time_model: TimeModel,
    pending: Vec<PendingRestriction>,
}

impl Default for GraphBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphBuilder {
    pub fn new() -> GraphBuilder {
        GraphBuilder {
            nodes: Vec::new(),
            node_flags: Vec::new(),
            edges: Vec::new(),
            time_model: TimeModel::synthetic(),
            pending: Vec::new(),
        }
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// ノードを足す。番号を返す。日本矩形の外の座標は足せるが、`build` が失敗する。
    pub fn add_node(&mut self, coord: Coord) -> NodeId {
        self.nodes.push(coord);
        self.node_flags.push(0);
        (self.nodes.len() - 1) as NodeId
    }

    /// 十進度でノードを足す（1e-7度に四捨五入）。
    pub fn add_node_deg(&mut self, lat: f64, lon: f64) -> NodeId {
        self.add_node(Coord::new(
            (lat * 1e7).round() as i32,
            (lon * 1e7).round() as i32,
        ))
    }

    pub fn node_coord(&self, node: NodeId) -> Coord {
        self.nodes[node as usize]
    }

    /// ノードのフラグに、ビットを足す（`node_flags::SIGNAL`、`node_flags::block_bit(mode)` など）。
    pub fn add_node_flags(&mut self, node: NodeId, bits: u8) {
        self.node_flags[node as usize] |= bits;
    }

    /// ノードのフラグを置き換える。
    pub fn set_node_flags(&mut self, node: NodeId, bits: u8) {
        self.node_flags[node as usize] = bits;
    }

    /// 辺を足す。番号を返す。端点は足したノードでなければならない（そうでなければpanic）。
    pub fn add_edge(&mut self, spec: EdgeSpec) -> EdgeId {
        let n = self.nodes.len();
        assert!(
            (spec.tail as usize) < n && (spec.head as usize) < n,
            "辺の端点（{}、{}）がノード数{n}以上です",
            spec.tail,
            spec.head
        );
        self.edges.push(spec);
        (self.edges.len() - 1) as EdgeId
    }

    /// 全手段・両方向に通れる `Residential` の辺を足す（`EdgeSpec::new` の既定）。
    pub fn add_road(&mut self, a: NodeId, b: NodeId) -> EdgeId {
        self.add_edge(EdgeSpec::new(a, b))
    }

    pub fn edge_spec(&self, e: EdgeId) -> &EdgeSpec {
        &self.edges[e as usize]
    }

    /// 足した辺の指定を後から変える。
    pub fn edge_spec_mut(&mut self, e: EdgeId) -> &mut EdgeSpec {
        &mut self.edges[e as usize]
    }

    /// 所要時間のモデルを替える（既定は `TimeModel::synthetic()`）。
    pub fn set_time_model(&mut self, model: TimeModel) {
        self.time_model = model;
    }

    /// `from` から `to` へ向かう有向辺。両端がこの2ノードの辺のうち番号が最小のもの
    /// （並行する辺は番号の小さい方）。なければ `None`。
    pub fn try_dedge(&self, from: NodeId, to: NodeId) -> Option<DirEdge> {
        self.edges.iter().enumerate().find_map(|(e, spec)| {
            if spec.tail == from && spec.head == to {
                Some(fwd(e as EdgeId))
            } else if spec.tail == to && spec.head == from {
                Some(bwd(e as EdgeId))
            } else {
                None
            }
        })
    }

    /// `try_dedge` で、なければpanic。
    pub fn dedge(&self, from: NodeId, to: NodeId) -> DirEdge {
        self.try_dedge(from, to)
            .unwrap_or_else(|| panic!("ノード{from}と{to}を結ぶ辺がありません"))
    }

    /// ノードの列（2つ以上）に沿った有向辺の列。
    pub fn dedges(&self, nodes: &[NodeId]) -> Vec<DirEdge> {
        nodes.windows(2).map(|w| self.dedge(w[0], w[1])).collect()
    }

    /// 右左折禁止：有向辺の列（長さ2以上）を、`modes`（`Mode::mask` のOR、`ALL_MODES_MASK` など）の
    /// 手段で続けて通ってはいけないものとして足す。長さ2はvia-node型、3以上はvia-way型として
    /// `kind` を決める。同じ列は1つにまとめ、手段のビットはORする。
    pub fn forbid(&mut self, seq: &[DirEdge], modes: u8) {
        assert!(
            seq.len() >= 2,
            "右左折禁止の列は2つ以上の有向辺です（{}個）",
            seq.len()
        );
        self.pending.push(PendingRestriction::Forbid {
            seq: seq.to_vec(),
            modes,
        });
    }

    /// `forbid` を、ノードの列で指定する（`[a, b, c]` は a→b→c を禁止）。
    pub fn forbid_nodes(&mut self, nodes: &[NodeId], modes: u8) {
        let seq = self.dedges(nodes);
        self.forbid(&seq, modes);
    }

    /// `only_*` 由来の禁止：有向辺の列 `prefix`（長さ1以上。via-node型なら `[from]`、via-way型なら
    /// `[from, via…]`）の末尾のノードから出る有向辺のうち、`allowed` 以外すべて（通行の可否によらない。
    /// 逆向きに戻る有向辺を含む）を、`prefix` の後に続けて通ってはいけないとする（形式書9章）。
    /// 展開は `restrictions` を呼ぶときに行うので、後から足した辺も対象になる。
    pub fn only(&mut self, prefix: &[DirEdge], allowed: DirEdge, modes: u8) {
        assert!(!prefix.is_empty(), "onlyの列が空です");
        self.pending.push(PendingRestriction::Only {
            prefix: prefix.to_vec(),
            allowed,
            modes,
        });
    }

    /// `only` を、ノードの列で指定する。`nodes` は `prefix` に沿ったノード（2つ以上）で、
    /// `allowed_next` は末尾のノードから許す行き先。
    pub fn only_nodes(&mut self, nodes: &[NodeId], allowed_next: NodeId, modes: u8) {
        let prefix = self.dedges(nodes);
        let last = *nodes.last().expect("nodes");
        let allowed = self.dedge(last, allowed_next);
        self.only(&prefix, allowed, modes);
    }

    /// ノードから出る有向辺（昇順。`ADJ` と同じ並び）。
    fn out_dedges(&self, node: NodeId) -> Vec<DirEdge> {
        let mut out = Vec::new();
        for (e, spec) in self.edges.iter().enumerate() {
            if spec.tail == node {
                out.push(fwd(e as EdgeId));
            }
            if spec.head == node {
                out.push(bwd(e as EdgeId));
            }
        }
        out
    }

    /// 節の組を作る（`Graph::from_parts` に渡す前の形。テストで壊して検査の拒否を調べるのにも使える）。
    pub fn parts(&self) -> GraphParts {
        let n = self.nodes.len();
        let mut node_ofs = vec![0_u32; n + 1];
        for spec in &self.edges {
            node_ofs[spec.tail as usize + 1] += 1;
            node_ofs[spec.head as usize + 1] += 1;
        }
        for v in 0..n {
            node_ofs[v + 1] += node_ofs[v];
        }
        let mut cursor = node_ofs.clone();
        let mut adj = vec![DirEdge(0); 2 * self.edges.len()];
        for (e, spec) in self.edges.iter().enumerate() {
            let e = e as EdgeId;
            adj[cursor[spec.tail as usize] as usize] = fwd(e);
            cursor[spec.tail as usize] += 1;
            adj[cursor[spec.head as usize] as usize] = bwd(e);
            cursor[spec.head as usize] += 1;
        }

        let mut edges = Vec::with_capacity(self.edges.len());
        let mut shape_ofs = vec![0_u32];
        let mut shape = Vec::new();
        let mut interpolated = 0_usize;
        for spec in &self.edges {
            let tail = self.nodes[spec.tail as usize];
            let head = self.nodes[spec.head as usize];
            let mut polyline = Vec::with_capacity(spec.shape.len() + 2);
            polyline.push(tail);
            polyline.extend_from_slice(&spec.shape);
            polyline.push(head);
            let length_dm = spec
                .length_dm
                .unwrap_or_else(|| polyline_length_dm(&polyline));
            let (points, added) = shape_deltas(&polyline);
            interpolated += added;
            shape.extend(points);
            shape_ofs.push(shape.len() as u32);
            edges.push(EdgeRecord {
                tail: spec.tail,
                head: spec.head,
                length_dm,
                class: spec.class.code(),
                access: spec.access,
                flags: spec.flags,
                maxspeed_kmh: spec.maxspeed_kmh,
            });
        }

        let params = json!({
            "rules_version": "synthetic",
            "nodes": n,
            "edges": self.edges.len(),
            "shape_points": shape.len(),
            "interpolated_points": interpolated,
        });
        GraphParts {
            params,
            time_model: self.time_model.clone(),
            node_ofs,
            adj,
            node_ll: self.nodes.clone(),
            node_flags: self.node_flags.clone(),
            edges,
            shape_ofs,
            shape,
        }
    }

    /// `Graph::from_parts` の検査つきで作る。
    pub fn try_build(&self) -> Result<Graph, GraphError> {
        Graph::from_parts(self.parts())
    }

    /// グラフを作る。検査に失敗したらpanic（テストで組み立てる用）。右左折禁止は含まない。
    pub fn build(&self) -> Graph {
        self.try_build()
            .unwrap_or_else(|e| panic!("合成グラフが不正です: {e}"))
    }

    /// 右左折禁止を、形式書9章の正規の形（列の辞書順、同じ列は手段のビットをORして1件）で作る。
    /// `graph` は同じ builder から作ったもの。列がつながっていないなどで不正ならpanic。
    pub fn restrictions(&self, graph: &Graph) -> Restrictions {
        let mut merged: BTreeMap<Vec<DirEdge>, u8> = BTreeMap::new();
        for pending in &self.pending {
            match pending {
                PendingRestriction::Forbid { seq, modes } => {
                    *merged.entry(seq.clone()).or_default() |= modes;
                }
                PendingRestriction::Only {
                    prefix,
                    allowed,
                    modes,
                } => {
                    let via = graph.dedge_to(*prefix.last().expect("prefix"));
                    assert_eq!(
                        graph.dedge_from(*allowed),
                        via,
                        "onlyの許す有向辺が、列の末尾のノードから出ていません"
                    );
                    for &out in graph.out_dedges(via) {
                        if out != *allowed {
                            let mut seq = prefix.clone();
                            seq.push(out);
                            *merged.entry(seq).or_default() |= modes;
                        }
                    }
                }
            }
        }
        let items = merged
            .into_iter()
            .map(|(seq, modes)| Restriction {
                kind: u8::from(seq.len() > 2),
                seq,
                modes,
            })
            .collect();
        let restrictions = Restrictions {
            params: Value::Object(Default::default()),
            items,
        };
        restrictions
            .validate(graph)
            .unwrap_or_else(|e| panic!("合成の右左折禁止が不正です: {e}"));
        restrictions
    }

    /// グラフと右左折禁止を作る。
    pub fn build_all(&self) -> (Graph, Restrictions) {
        let graph = self.build();
        let restrictions = self.restrictions(&graph);
        (graph, restrictions)
    }
}

/// 折れ線の長さ（デシメートル）。各線分の `straight_distance_m` の和を10倍して四捨五入。最小1（形式書1章）。
fn polyline_length_dm(points: &[Coord]) -> u32 {
    let metres: f64 = points
        .windows(2)
        .map(|w| straight_distance_m(w[0].to_latlon(), w[1].to_latlon()))
        .sum();
    ((metres * 10.0).round() as u32).max(1)
}

/// 折れ線（始点、中間の点、終点）から `SHAPE` の差分を作る。差分が `i16` を超える区間には、
/// 線分を等分する点を足す（形式書6.4節）。戻り値は（差分、足した点の数）。
fn shape_deltas(polyline: &[Coord]) -> (Vec<ShapePoint>, usize) {
    // 余裕を見て、1区間の差分の最大を32000にする（丸めで32768を超えないように）。
    const MAX_STEP: i64 = 32_000;
    let mut points = Vec::new();
    let mut added = 0;
    let mut cursor = polyline[0];
    let last_segment = polyline.len() - 2;
    for (i, w) in polyline.windows(2).enumerate() {
        let (from, to) = (w[0], w[1]);
        let dlat = i64::from(to.lat_e7) - i64::from(from.lat_e7);
        let dlon = i64::from(to.lon_e7) - i64::from(from.lon_e7);
        let pieces = (dlat.abs().max(dlon.abs()) + MAX_STEP - 1) / MAX_STEP;
        let pieces = pieces.max(1);
        for k in 1..=pieces {
            let next = if k == pieces {
                to
            } else {
                Coord::new(
                    (i64::from(from.lat_e7) + (dlat * k + pieces / 2) / pieces) as i32,
                    (i64::from(from.lon_e7) + (dlon * k + pieces / 2) / pieces) as i32,
                )
            };
            let is_head = i == last_segment && k == pieces;
            if !is_head {
                points.push(ShapePoint {
                    dlat_e7: i16::try_from(i64::from(next.lat_e7) - i64::from(cursor.lat_e7))
                        .expect("形状点の差分がi16を超えました"),
                    dlon_e7: i16::try_from(i64::from(next.lon_e7) - i64::from(cursor.lon_e7))
                        .expect("形状点の差分がi16を超えました"),
                });
                if k != pieces {
                    added += 1;
                }
            }
            cursor = next;
        }
    }
    (points, added)
}

// ---------------------------------------------------------------------------------------------
// 格子
// ---------------------------------------------------------------------------------------------

/// [`grid`] で作った格子の寸法。ノード番号は行優先（`行 × 列数 + 列`）。行は北へ、列は東へ増える。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridShape {
    pub rows: usize,
    pub cols: usize,
}

impl GridShape {
    pub fn node(self, row: usize, col: usize) -> NodeId {
        assert!(row < self.rows && col < self.cols, "格子の外です");
        (row * self.cols + col) as NodeId
    }
}

/// `rows × cols` の格子の builder と寸法。隣り合うノードを、全手段・両方向に通れる `Residential` の辺で結ぶ。
/// 格子の間隔は `spacing_m`（m）で、基準点は [`ORIGIN`]。辺の長さは座標から計算せず、
/// `round(spacing_m × 10)` デシメートルちょうどにする（手計算の所要時間と比べやすい。
/// 例：100 mなら 1000 dm、30 km/hで120デシ秒）。
///
/// 辺の番号は、行 r（0から）、列 c（0から）の順に、東の辺（(r,c)→(r,c+1)、あれば）、
/// 北の辺（(r,c)→(r+1,c)、あれば）を足した順。ノードの組から有向辺を引くには
/// `GraphBuilder::dedge` を使う。
pub fn grid(rows: usize, cols: usize, spacing_m: f64) -> (GraphBuilder, GridShape) {
    assert!(spacing_m > 0.0, "格子の間隔は正でなければなりません");
    let shape = GridShape { rows, cols };
    let mut builder = GraphBuilder::new();
    let lat_rad = f64::from(ORIGIN.lat_e7) / 1e7 * std::f64::consts::PI / 180.0;
    let dlat = (spacing_m / METERS_PER_DEGREE * 1e7).round() as i32;
    let dlon = (spacing_m / (METERS_PER_DEGREE * lat_rad.cos()) * 1e7).round() as i32;
    let length_dm = ((spacing_m * 10.0).round() as u32).max(1);
    for r in 0..rows {
        for c in 0..cols {
            builder.add_node(Coord::new(
                ORIGIN.lat_e7 + dlat * r as i32,
                ORIGIN.lon_e7 + dlon * c as i32,
            ));
        }
    }
    for r in 0..rows {
        for c in 0..cols {
            if c + 1 < cols {
                builder.add_edge(
                    EdgeSpec::new(shape.node(r, c), shape.node(r, c + 1)).length_dm(length_dm),
                );
            }
            if r + 1 < rows {
                builder.add_edge(
                    EdgeSpec::new(shape.node(r, c), shape.node(r + 1, c)).length_dm(length_dm),
                );
            }
        }
    }
    (builder, shape)
}

// ---------------------------------------------------------------------------------------------
// ランダムグラフ
// ---------------------------------------------------------------------------------------------

/// [`random_graph`] の設定。`RandomGraphConfig::new(nodes)` が最小の設定（全辺が全手段・両方向、
/// 信号・障害物・右左折禁止なし）で、フィールドを上書きして使う：
/// `RandomGraphConfig { oneway_prob: …, ..RandomGraphConfig::new(12) }`。
/// 「いろいろ付いた」設定の見本は [`RandomGraphConfig::rich`]。
///
/// 確率の欄は、0以下で付けない、1以上で必ず付ける。
#[derive(Debug, Clone)]
pub struct RandomGraphConfig {
    /// ノード数。座標は `lat_e7`・`lon_e7` の矩形の中で一様に選ぶ。
    pub nodes: usize,
    /// 全域木（連結にするための `nodes - 1` 本）に足す辺の数。足せない（重複しない組が尽きた）ときは少なくなる。
    pub extra_edges: usize,
    /// 真なら、木の辺は「それまでのノードのうち最も近いもの」に、足す辺は「最も近い3つのうちの1つ」につなぐ
    /// （道路網らしい短い辺になる）。偽なら一様な組。並行する辺・ループは作らない。
    pub local: bool,
    /// ノードの緯度の範囲（1e-7度、両端を含む）。日本矩形の内側にすること。
    pub lat_e7: (i32, i32),
    /// ノードの経度の範囲（1e-7度、両端を含む）。
    pub lon_e7: (i32, i32),
    /// 辺の種類の候補（一様に選ぶ。空にしない）。
    pub classes: Vec<HighwayClass>,
    /// 辺ごとに、その手段を両方向とも通れなくする確率。3手段とも通れなくなる辺には、ランダムな1手段を両方向で戻す。
    pub deny_prob: PerMode<f64>,
    /// 辺ごとに、その手段を一方通行（向きはランダム）にする確率。
    pub oneway_prob: PerMode<f64>,
    /// 辺ごとの中間の形状点の数は `0..=shape_points_max` の一様乱数。
    pub shape_points_max: usize,
    /// ノードごとに、信号・一時停止・踏切のフラグを付ける確率。
    pub signal_prob: f64,
    pub stop_prob: f64,
    pub level_crossing_prob: f64,
    /// ノードごとに、その手段の「通過できない」フラグを付ける確率。
    pub block_prob: PerMode<f64>,
    /// ランダムなvia-node型の禁止（長さ2の列）の数。
    pub via_node_restrictions: usize,
    /// ランダムなvia-way型の禁止（長さ3の列）の数。
    pub via_way_restrictions: usize,
    /// 禁止の対象の手段。`None` なら禁止ごとにランダム（ビット列は0でない）。
    pub restriction_modes: Option<u8>,
}

impl RandomGraphConfig {
    /// 最小の設定：木の辺 + `nodes / 2` 本、`local`、東京付近の約11km×9kmの矩形、種類は `Residential`、
    /// 全辺が全手段・両方向、形状点・信号・障害物・右左折禁止なし。
    pub fn new(nodes: usize) -> RandomGraphConfig {
        let zero = || PerMode {
            walk: 0.0,
            bike: 0.0,
            car: 0.0,
        };
        RandomGraphConfig {
            nodes,
            extra_edges: nodes / 2,
            local: true,
            lat_e7: (ORIGIN.lat_e7, ORIGIN.lat_e7 + 100_000),
            lon_e7: (ORIGIN.lon_e7, ORIGIN.lon_e7 + 100_000),
            classes: vec![HighwayClass::Residential],
            deny_prob: zero(),
            oneway_prob: zero(),
            shape_points_max: 0,
            signal_prob: 0.0,
            stop_prob: 0.0,
            level_crossing_prob: 0.0,
            block_prob: zero(),
            via_node_restrictions: 0,
            via_way_restrictions: 0,
            restriction_modes: None,
        }
    }

    /// いろいろ付いた設定：手段ごとの通行不可（walk・bikeが少し、carは稀）、一方通行（car・bikeが多め、
    /// walkは少し）、形状点0〜2個、信号・一時停止・踏切・障害物、via-node型とvia-way型の右左折禁止を
    /// ノード数に応じて付ける。種類は数種類。全探索との比較など、条件をまんべんなく踏みたいテスト向き。
    pub fn rich(nodes: usize) -> RandomGraphConfig {
        RandomGraphConfig {
            deny_prob: PerMode {
                walk: 0.05,
                bike: 0.10,
                car: 0.05,
            },
            oneway_prob: PerMode {
                walk: 0.05,
                bike: 0.20,
                car: 0.30,
            },
            classes: vec![
                HighwayClass::Primary,
                HighwayClass::Secondary,
                HighwayClass::Residential,
                HighwayClass::Service,
            ],
            shape_points_max: 2,
            signal_prob: 0.15,
            stop_prob: 0.05,
            level_crossing_prob: 0.03,
            block_prob: PerMode {
                walk: 0.02,
                bike: 0.03,
                car: 0.05,
            },
            via_node_restrictions: nodes / 3,
            via_way_restrictions: nodes / 6,
            ..RandomGraphConfig::new(nodes)
        }
    }
}

/// 局所的な近さの比較に使う、2ノードの距離の2乗（経度は緯度の余弦で縮める）。
fn distance_sq(a: Coord, b: Coord, cos_lat: f64) -> f64 {
    let dlat = f64::from(a.lat_e7) - f64::from(b.lat_e7);
    let dlon = (f64::from(a.lon_e7) - f64::from(b.lon_e7)) * cos_lat;
    dlat * dlat + dlon * dlon
}

/// 手段ごとの通行可否と一方通行をランダムに決める。
fn random_access(rng: &mut Rng, config: &RandomGraphConfig) -> u8 {
    let mut bits = 0;
    for mode in Mode::ALL {
        let deny = rng.chance(*config.deny_prob.get(mode));
        let oneway = rng.chance(*config.oneway_prob.get(mode));
        let reverse = rng.chance(0.5);
        if deny {
            continue;
        }
        bits |= if oneway {
            access::bit(mode, reverse)
        } else {
            access::both(mode)
        };
    }
    if bits == 0 {
        let mode = Mode::ALL[rng.below(Mode::ALL.len())];
        bits = access::both(mode);
    }
    bits
}

/// 設定に従ってランダムなグラフの builder を作る。同じ `seed` と設定なら同じ builder になる。
///
/// 1. ノードを矩形の中に一様に置く。
/// 2. 連結にする全域木（ノード `i` を、それより前のノードのどれかにつなぐ）と、`extra_edges` 本の追加の辺を足す。
///    辺の始点・終点の向きはランダム。種類・通行可否・一方通行・形状点はランダム（設定の確率）。
/// 3. ノードのフラグ（信号・一時停止・踏切・手段ごとの障害物）をランダムに付ける。
/// 4. ランダムな右左折禁止（via-node型は長さ2、via-way型は長さ3の連続した有向辺の列）を足す。
///
/// 戻り値の builder に、続けて辺・フラグ・禁止を足してから `build_all` できる。
pub fn random_graph(seed: u64, config: &RandomGraphConfig) -> GraphBuilder {
    assert!(!config.classes.is_empty(), "classesが空です");
    let mut rng = Rng::new(seed);
    let mut builder = GraphBuilder::new();
    let n = config.nodes;
    for _ in 0..n {
        let lat = rng.range_i32(config.lat_e7.0, config.lat_e7.1);
        let lon = rng.range_i32(config.lon_e7.0, config.lon_e7.1);
        builder.add_node(Coord::new(lat, lon));
    }
    let mid_lat_deg = (f64::from(config.lat_e7.0) + f64::from(config.lat_e7.1)) / 2.0 / 1e7;
    let cos_lat = mid_lat_deg.to_radians().cos();

    let mut pairs: BTreeSet<(usize, usize)> = BTreeSet::new();
    // 全域木
    for i in 1..n {
        let j = if config.local {
            let here = builder.nodes[i];
            (0..i)
                .min_by(|&x, &y| {
                    distance_sq(builder.nodes[x], here, cos_lat).total_cmp(&distance_sq(
                        builder.nodes[y],
                        here,
                        cos_lat,
                    ))
                })
                .expect("i >= 1")
        } else {
            rng.below(i)
        };
        pairs.insert((j, i));
        add_random_edge(&mut builder, &mut rng, config, j, i);
    }
    // 追加の辺
    if n >= 2 {
        let mut added = 0;
        let mut attempts = 0;
        while added < config.extra_edges && attempts < config.extra_edges * 20 + 20 {
            attempts += 1;
            let a = rng.below(n);
            let b = if config.local {
                let near = nearest_three(&builder, a, cos_lat);
                near[rng.below(near.len())]
            } else {
                rng.below(n)
            };
            if a == b || pairs.contains(&(a.min(b), a.max(b))) {
                continue;
            }
            pairs.insert((a.min(b), a.max(b)));
            add_random_edge(&mut builder, &mut rng, config, a, b);
            added += 1;
        }
    }

    // ノードのフラグ
    for v in 0..n {
        let mut bits = 0;
        if rng.chance(config.signal_prob) {
            bits |= node_flags::SIGNAL;
        }
        if rng.chance(config.stop_prob) {
            bits |= node_flags::STOP;
        }
        if rng.chance(config.level_crossing_prob) {
            bits |= node_flags::LEVEL_CROSSING;
        }
        for mode in Mode::ALL {
            if rng.chance(*config.block_prob.get(mode)) {
                bits |= node_flags::block_bit(mode);
            }
        }
        builder.add_node_flags(v as NodeId, bits);
    }

    // 右左折禁止
    if builder.edge_count() > 0 {
        let outs: Vec<Vec<DirEdge>> = (0..n).map(|v| builder.out_dedges(v as NodeId)).collect();
        let total_dedges = builder.edge_count() * 2;
        for _ in 0..config.via_node_restrictions {
            let via = rng.below(n);
            if outs[via].is_empty() {
                continue;
            }
            let incoming = rng.pick(&outs[via]).reversed();
            let outgoing = *rng.pick(&outs[via]);
            let modes = random_restriction_modes(&mut rng, config);
            builder.forbid(&[incoming, outgoing], modes);
        }
        for _ in 0..config.via_way_restrictions {
            let mut seq = vec![DirEdge(rng.below(total_dedges) as u32)];
            for _ in 0..2 {
                let node = builder.nodes_of(*seq.last().expect("seq")).1;
                seq.push(*rng.pick(&outs[node as usize]));
            }
            let modes = random_restriction_modes(&mut rng, config);
            builder.forbid(&seq, modes);
        }
    }
    builder
}

/// ノード `a` に近い（`a` 自身を除く）最大3ノード。距離の昇順、同じなら番号の小さい順。
fn nearest_three(builder: &GraphBuilder, a: usize, cos_lat: f64) -> Vec<usize> {
    let here = builder.nodes[a];
    let mut near: Vec<(f64, usize)> = Vec::with_capacity(4);
    for v in (0..builder.nodes.len()).filter(|&v| v != a) {
        let d = distance_sq(builder.nodes[v], here, cos_lat);
        if near.len() < 3 || d < near[near.len() - 1].0 {
            let at = near.partition_point(|&(x, _)| x <= d);
            near.insert(at, (d, v));
            near.truncate(3);
        }
    }
    near.into_iter().map(|(_, v)| v).collect()
}

fn random_restriction_modes(rng: &mut Rng, config: &RandomGraphConfig) -> u8 {
    match config.restriction_modes {
        Some(modes) => modes,
        None => 1 + rng.below(7) as u8,
    }
}

/// ランダムな辺を足す。端点は `a`・`b`（向きはランダム）。形状点は a→b の線分上の点を少しずらしたもの。
fn add_random_edge(
    builder: &mut GraphBuilder,
    rng: &mut Rng,
    config: &RandomGraphConfig,
    a: usize,
    b: usize,
) {
    let (tail, head) = if rng.chance(0.5) { (a, b) } else { (b, a) };
    let class = *rng.pick(&config.classes);
    let bits = random_access(rng, config);
    let count = rng.below(config.shape_points_max + 1);
    let from = builder.nodes[tail];
    let to = builder.nodes[head];
    let jitter_lat = (i64::from(to.lat_e7) - i64::from(from.lat_e7)).abs() as i32 / 8 + 1;
    let jitter_lon = (i64::from(to.lon_e7) - i64::from(from.lon_e7)).abs() as i32 / 8 + 1;
    let lerp = |x: i32, y: i32, k: usize| {
        (i64::from(x) + (i64::from(y) - i64::from(x)) * k as i64 / (count as i64 + 1)) as i32
    };
    let shape: Vec<Coord> = (1..=count)
        .map(|k| {
            Coord::new(
                lerp(from.lat_e7, to.lat_e7, k) + rng.range_i32(-jitter_lat, jitter_lat),
                lerp(from.lon_e7, to.lon_e7, k) + rng.range_i32(-jitter_lon, jitter_lon),
            )
        })
        .collect();
    builder.add_edge(
        EdgeSpec::new(tail as NodeId, head as NodeId)
            .class(class)
            .access(bits)
            .shape(&shape),
    );
}

impl GraphBuilder {
    /// 有向辺の（出発ノード, 到着ノード）。
    fn nodes_of(&self, d: DirEdge) -> (NodeId, NodeId) {
        let spec = &self.edges[d.edge() as usize];
        if d.is_reverse() {
            (spec.head, spec.tail)
        } else {
            (spec.tail, spec.head)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_and_spread() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        let xs: Vec<u64> = (0..8).map(|_| a.next_u64()).collect();
        let ys: Vec<u64> = (0..8).map(|_| b.next_u64()).collect();
        assert_eq!(xs, ys);
        assert_ne!(
            xs,
            (0..8).map(|_| Rng::new(8).next_u64()).collect::<Vec<_>>()
        );
        // シード0でも止まらない。
        let mut z = Rng::new(0);
        assert_ne!(z.next_u64(), z.next_u64());
        // 範囲
        let mut r = Rng::new(1);
        let mut seen = [false; 5];
        for _ in 0..200 {
            let v = r.below(5);
            seen[v] = true;
            let w = r.range_i32(-3, 3);
            assert!((-3..=3).contains(&w));
            let u = r.unit();
            assert!((0.0..1.0).contains(&u));
        }
        assert!(seen.iter().all(|&s| s));
        assert!(!Rng::new(1).chance(0.0) && Rng::new(1).chance(1.0));
        let mut items: Vec<u32> = (0..10).collect();
        Rng::new(3).shuffle(&mut items);
        items.sort_unstable();
        assert_eq!(items, (0..10).collect::<Vec<_>>());
    }

    #[test]
    fn builder_makes_valid_csr_and_adj() {
        let mut b = GraphBuilder::new();
        let n0 = b.add_node_deg(35.0, 139.0);
        let n1 = b.add_node_deg(35.0, 139.001);
        let n2 = b.add_node_deg(35.001, 139.001);
        let e0 = b.add_road(n0, n1);
        let e1 = b.add_edge(EdgeSpec::new(n1, n2).oneway());
        let e2 = b.add_road(n2, n2); // ループ
        let e3 = b.add_road(n1, n0); // 並行辺
        let g = b.build();
        assert_eq!((g.node_count(), g.edge_count()), (3, 4));
        assert_eq!(g.out_dedges(n0), &[fwd(e0), bwd(e3)]);
        assert_eq!(g.out_dedges(n1), &[bwd(e0), fwd(e1), fwd(e3)]);
        // ループの2つの有向辺はどちらも同じノードの隣接リストに入る。
        assert_eq!(g.out_dedges(n2), &[bwd(e1), fwd(e2), bwd(e2)]);
        assert_eq!(g.parts().params["edges"], 4);
        // 長さは座標から計算される（約111 m × 0.819 = 91 m、東西）。
        let len = g.edge(e0).length_dm;
        assert!((900..=920).contains(&len), "{len}");
        assert_eq!(b.dedge(n1, n0), bwd(e0));
        assert_eq!(b.dedges(&[n0, n1, n2]), vec![fwd(e0), fwd(e1)]);
    }

    #[test]
    fn dedge_lookup_and_missing_edge() {
        let mut b = GraphBuilder::new();
        let a = b.add_node_deg(35.0, 139.0);
        let c = b.add_node_deg(35.0, 139.001);
        let d = b.add_node_deg(35.0, 139.002);
        let e = b.add_road(a, c);
        assert_eq!(b.try_dedge(a, c), Some(fwd(e)));
        assert_eq!(b.try_dedge(c, a), Some(bwd(e)));
        assert_eq!(b.try_dedge(a, d), None);
    }

    #[test]
    fn explicit_length_and_shape_deltas() {
        let mut b = GraphBuilder::new();
        let a = b.add_node(Coord::new(350_000_000, 1_390_000_000));
        let c = b.add_node(Coord::new(350_000_000, 1_390_200_000));
        let e = b.add_edge(
            EdgeSpec::new(a, c)
                .length_dm(1234)
                .shape(&[Coord::new(350_000_100, 1_390_050_000)]),
        );
        // 始点→形状点の経度差は50,000で i16 を超えるので、等分の点が入る。
        let g = b.build();
        assert_eq!(g.edge(e).length_dm, 1234);
        let line = g.edge_polyline(e);
        assert_eq!(line.first(), Some(&Coord::new(350_000_000, 1_390_000_000)));
        assert_eq!(line.last(), Some(&Coord::new(350_000_000, 1_390_200_000)));
        assert!(line.contains(&Coord::new(350_000_100, 1_390_050_000)));
        let added = g.parts().params["interpolated_points"].as_u64().unwrap();
        // 50,000 → 2区間、(200,000-50,000) = 150,000 → 5区間。
        assert_eq!(added, 1 + 4);
        assert_eq!(g.parts().shape.len(), 1 + 1 + 4);
        // 差分はすべて i16 に収まり、折れ線は単調に進む。
        for w in line.windows(2) {
            assert!((w[1].lon_e7 - w[0].lon_e7).abs() <= 32_767);
        }
    }

    #[test]
    fn invalid_inputs_are_reported_by_try_build() {
        let mut b = GraphBuilder::new();
        let a = b.add_node(Coord::new(0, 0)); // 日本矩形の外
        let c = b.add_node_deg(35.0, 139.0);
        b.add_road(a, c);
        assert!(matches!(b.try_build(), Err(GraphError::Coordinates(_))));

        let mut b = GraphBuilder::new();
        let a = b.add_node_deg(35.0, 139.0);
        let c = b.add_node_deg(35.0, 139.001);
        b.add_edge(EdgeSpec::new(a, c).access(0));
        assert!(matches!(b.try_build(), Err(GraphError::Edge(_))));
    }

    #[test]
    #[should_panic(expected = "端点")]
    fn add_edge_rejects_unknown_nodes() {
        let mut b = GraphBuilder::new();
        let a = b.add_node_deg(35.0, 139.0);
        b.add_road(a, 5);
    }

    #[test]
    fn edge_spec_modifiers() {
        let s = EdgeSpec::new(0, 1).oneway();
        assert_eq!(
            s.access,
            access::CAR_FWD | access::BIKE_FWD | access::WALK_FWD | access::WALK_BWD
        );
        let s = EdgeSpec::new(0, 1).reverse_oneway();
        assert_eq!(
            s.access,
            access::CAR_BWD | access::BIKE_BWD | access::WALK_FWD | access::WALK_BWD
        );
        let s = EdgeSpec::new(0, 1).deny(Mode::Car).deny(Mode::Walk);
        assert_eq!(s.access, access::both(Mode::Bike));
        let s = EdgeSpec::new(0, 1)
            .class(HighwayClass::Steps)
            .flags(0x02)
            .maxspeed(40);
        assert_eq!(
            (s.class, s.flags, s.maxspeed_kmh),
            (HighwayClass::Steps, 2, 40)
        );
    }

    #[test]
    fn grid_has_expected_shape_and_exact_lengths() {
        let (b, shape) = grid(3, 4, 100.0);
        assert_eq!((b.node_count(), b.edge_count()), (12, 3 * 3 + 2 * 4));
        let g = b.build();
        assert!((0..g.edge_count() as u32).all(|e| g.edge(e).length_dm == 1000));
        assert_eq!(shape.node(1, 2), 6);
        let n = shape.node(1, 1);
        // 内部のノードは次数4。
        assert_eq!(g.out_dedges(n).len(), 4);
        // 東に隣り合うノードは緯度が同じ、経度が増える。北は緯度が増える。
        let (c0, c1, c2) = (
            g.node_coord(shape.node(0, 0)),
            g.node_coord(shape.node(0, 1)),
            g.node_coord(shape.node(1, 0)),
        );
        assert_eq!(c0.lat_e7, c1.lat_e7);
        assert!(c1.lon_e7 > c0.lon_e7 && c2.lat_e7 > c0.lat_e7 && c2.lon_e7 == c0.lon_e7);
        assert_eq!(grid(0, 5, 10.0).0.build().node_count(), 0);
        assert_eq!(grid(1, 1, 10.0).0.build().edge_count(), 0);
    }

    #[test]
    fn restrictions_are_canonical_and_validated() {
        let (mut b, s) = grid(3, 3, 100.0);
        let n = |r, c| s.node(r, c);
        // via-nodeとvia-way、同じ列の手段ORと重複。
        b.forbid_nodes(&[n(0, 0), n(0, 1), n(0, 2)], Mode::Car.mask());
        b.forbid_nodes(&[n(0, 0), n(0, 1), n(0, 2)], Mode::Bike.mask());
        b.forbid_nodes(&[n(0, 0), n(0, 1), n(1, 1), n(2, 1)], ALL_MODES_MASK);
        b.forbid_nodes(&[n(0, 1), n(1, 1), n(1, 2)], Mode::Walk.mask());
        let (g, r) = b.build_all();
        assert_eq!(r.items.len(), 3);
        assert!(r.items.windows(2).all(|w| w[0].seq < w[1].seq));
        let merged = r
            .items
            .iter()
            .find(|i| i.seq == b.dedges(&[n(0, 0), n(0, 1), n(0, 2)]))
            .unwrap();
        assert_eq!((merged.modes, merged.kind), (6, 0));
        assert_eq!(r.items.iter().filter(|i| i.kind == 1).count(), 1);
        assert_eq!(r.validate(&g), Ok(()));
    }

    #[test]
    fn only_expands_to_every_other_outgoing_dedge() {
        let (mut b, s) = grid(3, 3, 100.0);
        let n = |r, c| s.node(r, c);
        // (0,0)→(0,1) の後は (0,2) だけ許す。(0,1) からは (0,0)（引き返し）、(0,2)、(1,1) へ出られる。
        b.only_nodes(&[n(0, 0), n(0, 1)], n(0, 2), Mode::Car.mask());
        let (_, r) = b.build_all();
        let prefix = b.dedge(n(0, 0), n(0, 1));
        let mut got: Vec<Vec<DirEdge>> = r.items.iter().map(|i| i.seq.clone()).collect();
        got.sort();
        let mut want = vec![
            vec![prefix, b.dedge(n(0, 1), n(0, 0))],
            vec![prefix, b.dedge(n(0, 1), n(1, 1))],
        ];
        want.sort();
        assert_eq!(got, want);
        assert!(r.items.iter().all(|i| i.modes == 4 && i.kind == 0));
    }

    #[test]
    #[should_panic(expected = "右左折禁止が不正")]
    fn unconnected_restriction_panics_on_build() {
        let (mut b, s) = grid(2, 2, 100.0);
        let d1 = b.dedge(s.node(0, 0), s.node(0, 1));
        let d2 = b.dedge(s.node(1, 0), s.node(1, 1));
        b.forbid(&[d1, d2], ALL_MODES_MASK);
        let _ = b.build_all();
    }

    #[test]
    fn node_flags_reach_the_graph() {
        let (mut b, s) = grid(2, 2, 100.0);
        b.add_node_flags(s.node(0, 0), node_flags::SIGNAL);
        b.add_node_flags(s.node(0, 0), node_flags::block_bit(Mode::Car));
        b.set_node_flags(s.node(1, 1), node_flags::STOP);
        let g = b.build();
        assert_eq!(g.node_flags(s.node(0, 0)), 0x11);
        assert!(
            g.node_blocked(s.node(0, 0), Mode::Car) && !g.node_blocked(s.node(0, 0), Mode::Bike)
        );
        assert_eq!(g.node_flags(s.node(1, 1)), node_flags::STOP);
    }

    #[test]
    fn random_graph_is_valid_for_many_seeds_and_sizes() {
        for seed in 0..40_u64 {
            for nodes in [0, 1, 2, 3, 7, 12, 40] {
                for config in [
                    RandomGraphConfig::new(nodes),
                    RandomGraphConfig::rich(nodes),
                    RandomGraphConfig {
                        local: false,
                        extra_edges: nodes * 2,
                        ..RandomGraphConfig::rich(nodes)
                    },
                ] {
                    let (g, r) = random_graph(seed, &config).build_all();
                    assert_eq!(g.node_count(), nodes);
                    if nodes >= 2 {
                        assert!(g.edge_count() >= nodes - 1, "{seed} {nodes}");
                    }
                    assert_eq!(r.validate(&g), Ok(()));
                    for e in 0..g.edge_count() as u32 {
                        assert_ne!(g.edge(e).access, 0);
                    }
                }
            }
        }
    }

    #[test]
    fn random_graph_is_deterministic_and_seed_dependent() {
        let config = RandomGraphConfig::rich(12);
        let a = random_graph(5, &config).build_all();
        let b = random_graph(5, &config).build_all();
        assert_eq!(a.0.parts(), b.0.parts());
        assert_eq!(a.1, b.1);
        let c = random_graph(6, &config).build_all();
        assert_ne!(a.0.parts(), c.0.parts());
    }

    #[test]
    fn random_graph_is_connected_and_inside_the_rectangle() {
        for seed in 0..10_u64 {
            let config = RandomGraphConfig::new(30);
            let g = random_graph(seed, &config).build();
            // 全域木があるので、無向グラフとして連結。
            let mut seen = vec![false; g.node_count()];
            let mut stack = vec![0_u32];
            seen[0] = true;
            while let Some(v) = stack.pop() {
                for &d in g.out_dedges(v) {
                    let w = g.dedge_to(d);
                    if !seen[w as usize] {
                        seen[w as usize] = true;
                        stack.push(w);
                    }
                }
            }
            assert!(seen.iter().all(|&s| s), "seed {seed}");
            assert!((0..g.node_count() as u32).all(|v| g.node_coord(v).in_japan()));
            // 並行辺・ループはない。
            let mut pairs = BTreeSet::new();
            for e in 0..g.edge_count() as u32 {
                let r = g.edge(e);
                assert_ne!(r.tail, r.head);
                assert!(pairs.insert((r.tail.min(r.head), r.tail.max(r.head))));
            }
        }
    }

    #[test]
    fn rich_config_exercises_the_features() {
        let mut access_variety = BTreeSet::new();
        let (mut signals, mut blocks, mut restrictions, mut shapes) = (0, 0, 0, 0);
        for seed in 0..30_u64 {
            let (g, r) = random_graph(seed, &RandomGraphConfig::rich(12)).build_all();
            for e in 0..g.edge_count() as u32 {
                access_variety.insert(g.edge(e).access);
            }
            for v in 0..g.node_count() as u32 {
                let f = g.node_flags(v);
                signals += usize::from(f & node_flags::SIGNAL != 0);
                blocks += usize::from(f & 0x70 != 0);
            }
            restrictions += r.items.len();
            shapes += g.parts().shape.len();
        }
        assert!(access_variety.len() >= 6, "{access_variety:?}");
        assert!(signals > 10 && blocks > 5 && restrictions > 30 && shapes > 30);
    }

    #[test]
    fn large_random_graph_builds() {
        let config = RandomGraphConfig {
            extra_edges: 2500,
            ..RandomGraphConfig::new(5000)
        };
        let g = random_graph(1, &config).build();
        assert_eq!(g.node_count(), 5000);
        assert!(g.edge_count() > 5000);
    }
}
