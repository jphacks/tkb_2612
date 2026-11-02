//! P1：wayの表とノードの表から、辺の分割・CSR・形状点を作る（形式書5〜6章、計画書3.3節）。
//!
//! 入力は `data::osm` がPBFから作る2つの表（`WayTable`・`NodeTable`）。ここにはI/Oがない。
//!
//! 手順：
//!
//! 1. 座標が日本矩形に入るノードだけを有効とし、wayを「有効なノードが続く区間」に分ける
//!    （範囲外・欠けたノードを含むwayは、その部分を捨てる。形式書5章）。連続して同じノードの
//!    重複は1つにする。2ノード未満の区間は捨てる。
//! 2. 区間の中のノードの出現回数と端点から、グラフのノードを決める。グラフのノードは、
//!    区間の端点、2回以上現れるノード（交差点）、信号・横断歩道・一時停止・踏切・障害物のノード
//!    （`NODEFLG` を持つノード）、右左折禁止relationのviaノード。残りは辺の形状点になる。
//! 3. ノード番号はOSMノードIDの昇順、辺番号はwayのIDの昇順（way内は先頭から）で振る。
//!    どちらも同じ入力から同じ結果になる（並列処理に依存しない）。

use serde_json::{Map, Value, json};

use crate::domain::routing::graph::rules::{RULES_VERSION, WayExclusion};
use crate::domain::routing::graph::{
    Coord, DirEdge, EdgeRecord, GraphParts, HighwayClass, ShapePoint, access, node_flags,
};
use crate::domain::routing::prep::attrs::{GraphAttrs, StringInterner};
use crate::domain::routing::prep::geom::length_dm;
use crate::domain::routing::prep::grid::in_japan;
use crate::domain::routing::prep::time_defaults::default_time_model;

/// 座標がないノードの印（`NodeTable::lat_e7`）。
pub const NODE_MISSING: i32 = i32::MIN;
/// 前処理の内部だけで使うノードのビット：右左折禁止relationのviaノード（`NODEFLG` のビット7は
/// 予約なので、出力前に落とす）。
pub const NODE_FORCE: u8 = 0x80;
/// 辺の分割点にするノードのビット。
const NODE_SPLIT_MASK: u8 = node_flags::SIGNAL
    | node_flags::CROSSING
    | node_flags::STOP
    | node_flags::LEVEL_CROSSING
    | node_flags::BLOCK_CAR
    | node_flags::BLOCK_BIKE
    | node_flags::BLOCK_WALK
    | NODE_FORCE;
/// 形状点の差分の絶対値の上限（形式書6.4節は±32767。丸めの余裕を1とる）。
const MAX_SHAPE_DELTA: i64 = 32_766;

/// wayごとの、タグから決まる属性。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WayMeta {
    pub class: u8,
    pub access: u8,
    pub flags: u8,
    pub maxspeed_kmh: u8,
    pub lanes: u8,
    pub width_dm: u16,
    pub layer: i8,
}

/// グラフに入れるwayの表。way IDの昇順で、IDは重複しない。
#[derive(Debug, Clone, Default)]
pub struct WayTable {
    pub id: Vec<i64>,
    pub meta: Vec<WayMeta>,
    pub ref_str: Vec<Option<Box<str>>>,
    pub name_str: Vec<Option<Box<str>>>,
    /// 長さ `way数 + 1`。wayの構成ノードは `refs[ref_ofs[w]..ref_ofs[w+1]]`。
    pub ref_ofs: Vec<u32>,
    /// `NodeTable` の添字。
    pub refs: Vec<u32>,
}

impl WayTable {
    pub fn len(&self) -> usize {
        self.id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.id.is_empty()
    }

    fn refs_of(&self, w: usize) -> &[u32] {
        &self.refs[self.ref_ofs[w] as usize..self.ref_ofs[w + 1] as usize]
    }
}

/// wayが参照するノードの表。OSMノードIDの昇順で、IDは重複しない。
#[derive(Debug, Clone, Default)]
pub struct NodeTable {
    pub id: Vec<i64>,
    /// `NODE_MISSING` は座標がないノード（PBFにない）。
    pub lat_e7: Vec<i32>,
    pub lon_e7: Vec<i32>,
    /// `NODEFLG` のビット0〜6（`rules::classify_node`）と `NODE_FORCE`。
    pub flags: Vec<u8>,
}

impl NodeTable {
    pub fn len(&self) -> usize {
        self.id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.id.is_empty()
    }

    fn coord(&self, n: u32) -> Coord {
        Coord::new(self.lat_e7[n as usize], self.lon_e7[n as usize])
    }

    fn valid(&self, n: usize) -> bool {
        self.lat_e7[n] != NODE_MISSING && in_japan(Coord::new(self.lat_e7[n], self.lon_e7[n]))
    }
}

/// wayの除外件数（理由別）。`data::osm` が数えて、`graph.bin` の `PARAMS` に渡す。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExclusionCounts {
    pub not_highway: u64,
    pub unsupported_highway: u64,
    pub area_yes: u64,
    pub no_access: u64,
}

impl ExclusionCounts {
    pub fn add(&mut self, reason: WayExclusion) {
        match reason {
            WayExclusion::NotHighway => self.not_highway += 1,
            WayExclusion::UnsupportedHighway => self.unsupported_highway += 1,
            WayExclusion::Area => self.area_yes += 1,
            WayExclusion::NoAccess => self.no_access += 1,
        }
    }

    pub fn merge(&mut self, other: &ExclusionCounts) {
        self.not_highway += other.not_highway;
        self.unsupported_highway += other.unsupported_highway;
        self.area_yes += other.area_yes;
        self.no_access += other.no_access;
    }

    pub fn to_json(&self) -> Value {
        json!({
            WayExclusion::NotHighway.as_str(): self.not_highway,
            WayExclusion::UnsupportedHighway.as_str(): self.unsupported_highway,
            WayExclusion::Area.as_str(): self.area_yes,
            WayExclusion::NoAccess.as_str(): self.no_access,
        })
    }
}

/// P1の出力。
#[derive(Debug)]
pub struct BuiltGraph {
    pub parts: GraphParts,
    pub attrs: GraphAttrs,
    /// 件数・除外・計測の監査（`graph.bin` の `PARAMS` に入れたものより詳しい）。
    pub audit: Value,
}

/// wayの構成ノードを「有効なノードが続く区間」に分けて、区間ごとに `f(way添字, ノード添字の列)` を呼ぶ。
/// 連続する同じノードは1つにし、2ノード未満の区間は呼ばない。
fn for_each_run(ways: &WayTable, valid: &[bool], mut f: impl FnMut(usize, &[u32])) {
    let mut run: Vec<u32> = Vec::new();
    for w in 0..ways.len() {
        run.clear();
        for &n in ways.refs_of(w) {
            if !valid[n as usize] {
                if run.len() >= 2 {
                    f(w, &run);
                }
                run.clear();
                continue;
            }
            if run.last() == Some(&n) {
                continue;
            }
            run.push(n);
        }
        if run.len() >= 2 {
            f(w, &run);
        }
    }
}

/// 辺ごとの中間点を、差分が±32767に収まるように並べ直して `shape` に足す（形式書6.4節）。
/// 足した点の数を返す。`tail` は始点ノードの座標。最後の形状点から終点ノードへの線分は
/// 差分を持たないので、等分しない。
fn push_shape(tail: Coord, interior: &[Coord], shape: &mut Vec<ShapePoint>) -> u64 {
    fn push(from: Coord, to: Coord, shape: &mut Vec<ShapePoint>) {
        shape.push(ShapePoint {
            dlat_e7: (i64::from(to.lat_e7) - i64::from(from.lat_e7)) as i16,
            dlon_e7: (i64::from(to.lon_e7) - i64::from(from.lon_e7)) as i16,
        });
    }
    let mut interpolated = 0;
    let mut current = tail;
    for &point in interior {
        let dlat = i64::from(point.lat_e7) - i64::from(current.lat_e7);
        let dlon = i64::from(point.lon_e7) - i64::from(current.lon_e7);
        let pieces = (dlat.abs().max(dlon.abs()) + MAX_SHAPE_DELTA - 1) / MAX_SHAPE_DELTA;
        let start = current;
        for i in 1..pieces {
            // 線分を等分する点（1e-7度に四捨五入）
            let mid = Coord::new(
                (i64::from(start.lat_e7) + div_round(dlat * i, pieces)) as i32,
                (i64::from(start.lon_e7) + div_round(dlon * i, pieces)) as i32,
            );
            push(current, mid, shape);
            current = mid;
            interpolated += 1;
        }
        push(current, point, shape);
        current = point;
    }
    interpolated
}

/// 整数の割り算の四捨五入（0から遠ざかる向きの半分）。
fn div_round(numerator: i64, denominator: i64) -> i64 {
    let half = denominator / 2;
    if numerator >= 0 {
        (numerator + half) / denominator
    } else {
        -((-numerator + half) / denominator)
    }
}

/// P1の本体。`extra_params` は `graph.bin` の `PARAMS` に足すキー（PBFの識別子、wayの除外件数など）。
pub fn build_graph(
    ways: &WayTable,
    nodes: &NodeTable,
    extra_params: Map<String, Value>,
) -> Result<BuiltGraph, String> {
    if ways.ref_ofs.len() != ways.len() + 1 {
        return Err("wayの表のref_ofsの長さが不正です".to_owned());
    }
    let valid: Vec<bool> = (0..nodes.len()).map(|n| nodes.valid(n)).collect();

    // --- 2. 区間の中のノードの出現と端点 ---
    let mut occurrences = vec![0u8; nodes.len()];
    let mut endpoint = vec![false; nodes.len()];
    let mut runs = 0u64;
    let mut run_nodes = 0u64;
    for_each_run(ways, &valid, |_, run| {
        runs += 1;
        run_nodes += run.len() as u64;
        for &n in run {
            let slot = &mut occurrences[n as usize];
            *slot = slot.saturating_add(1);
        }
        endpoint[run[0] as usize] = true;
        endpoint[run[run.len() - 1] as usize] = true;
    });
    let invalid_refs = ways.refs.iter().filter(|&&n| !valid[n as usize]).count() as u64;

    // グラフのノードか（出現しないノードは対象外）
    let mut graph_node_count = 0u64;
    let mut reasons = [0u64; 4]; // 端点、交差点（2回以上）、フラグ付き、viaノード
    let node_no: Vec<u32> = {
        let mut next = 0u32;
        (0..nodes.len())
            .map(|n| {
                let occ = occurrences[n];
                if occ == 0 {
                    return u32::MAX;
                }
                let flags = nodes.flags[n];
                let junction = occ >= 2;
                let flagged = flags & NODE_SPLIT_MASK & !NODE_FORCE != 0;
                let forced = flags & NODE_FORCE != 0;
                if endpoint[n] || junction || flagged || forced {
                    reasons[0] += u64::from(endpoint[n]);
                    reasons[1] += u64::from(junction && !endpoint[n]);
                    reasons[2] += u64::from(flagged && !endpoint[n] && !junction);
                    reasons[3] += u64::from(forced && !endpoint[n] && !junction && !flagged);
                    let id = next;
                    next += 1;
                    graph_node_count += 1;
                    id
                } else {
                    u32::MAX
                }
            })
            .collect()
    };
    let node_total = usize::try_from(graph_node_count).map_err(|e| e.to_string())?;
    if node_total >= u32::MAX as usize {
        return Err("ノード数がu32に収まりません".to_owned());
    }

    // --- 3. ノードの節 ---
    let mut node_ll = Vec::with_capacity(node_total);
    let mut node_flag_bytes = Vec::with_capacity(node_total);
    let mut node_osm_id = Vec::with_capacity(node_total);
    for n in 0..nodes.len() {
        if node_no[n] == u32::MAX {
            continue;
        }
        node_ll.push(nodes.coord(n as u32));
        node_flag_bytes.push(nodes.flags[n] & !NODE_FORCE & 0x7f);
        node_osm_id.push(nodes.id[n]);
    }

    // --- 4. 辺 ---
    let mut edge_total = 0usize;
    for_each_run(ways, &valid, |_, run| {
        let graph_nodes = run
            .iter()
            .filter(|&&n| node_no[n as usize] != u32::MAX)
            .count();
        edge_total += graph_nodes - 1;
    });
    if edge_total >= (u32::MAX / 2) as usize {
        return Err("辺数が2^31未満に収まりません".to_owned());
    }

    let mut edges: Vec<EdgeRecord> = Vec::with_capacity(edge_total);
    let mut shape_ofs: Vec<u32> = Vec::with_capacity(edge_total + 1);
    let mut shape: Vec<ShapePoint> = Vec::with_capacity(run_nodes as usize);
    let mut way_id = Vec::with_capacity(edge_total);
    let mut ref_idx = Vec::with_capacity(edge_total);
    let mut name_idx = Vec::with_capacity(edge_total);
    let mut lanes = Vec::with_capacity(edge_total);
    let mut width_dm = Vec::with_capacity(edge_total);
    let mut layer = Vec::with_capacity(edge_total);
    let mut strings = StringInterner::new();
    let mut interpolated = 0u64;
    let mut failure: Option<String> = None;
    shape_ofs.push(0);

    let mut polyline: Vec<Coord> = Vec::new();
    let mut interior: Vec<Coord> = Vec::new();
    for_each_run(ways, &valid, |w, run| {
        let meta = ways.meta[w];
        let ref_id = ways.ref_str[w].as_deref().map_or(0, |s| strings.intern(s));
        let name_id = ways.name_str[w].as_deref().map_or(0, |s| strings.intern(s));
        let mut tail = node_no[run[0] as usize];
        polyline.clear();
        polyline.push(nodes.coord(run[0]));
        interior.clear();
        for &n in &run[1..] {
            let coord = nodes.coord(n);
            polyline.push(coord);
            let head = node_no[n as usize];
            if head == u32::MAX {
                interior.push(coord);
                continue;
            }
            // 辺を確定する
            let tail_coord = polyline[0];
            interpolated += push_shape(tail_coord, &interior, &mut shape);
            match u32::try_from(shape.len()) {
                Ok(len) => shape_ofs.push(len),
                Err(_) => {
                    failure = Some("形状点の数がu32に収まりません".to_owned());
                    return;
                }
            }
            edges.push(EdgeRecord {
                tail,
                head,
                length_dm: length_dm(&polyline),
                class: meta.class,
                access: meta.access,
                flags: meta.flags,
                maxspeed_kmh: meta.maxspeed_kmh,
            });
            way_id.push(ways.id[w]);
            ref_idx.push(ref_id);
            name_idx.push(name_id);
            lanes.push(meta.lanes);
            width_dm.push(meta.width_dm);
            layer.push(meta.layer);
            tail = head;
            polyline.clear();
            polyline.push(coord);
            interior.clear();
        }
    });
    if let Some(message) = failure {
        return Err(message);
    }
    debug_assert_eq!(edges.len(), edge_total);

    // --- 5. CSR ---
    let mut node_ofs = vec![0u32; node_total + 1];
    for edge in &edges {
        node_ofs[edge.tail as usize + 1] += 1;
        node_ofs[edge.head as usize + 1] += 1;
    }
    for v in 0..node_total {
        node_ofs[v + 1] += node_ofs[v];
    }
    let mut cursor: Vec<u32> = node_ofs[..node_total].to_vec();
    let mut adj = vec![DirEdge(0); edges.len() * 2];
    for (e, edge) in edges.iter().enumerate() {
        // 辺番号の昇順に足すので、各ノードの中は昇順になる。ループは2e、2e+1の順
        let slot = &mut cursor[edge.tail as usize];
        adj[*slot as usize] = DirEdge::new(e as u32, false);
        *slot += 1;
        let slot = &mut cursor[edge.head as usize];
        adj[*slot as usize] = DirEdge::new(e as u32, true);
        *slot += 1;
    }

    // --- 6. 監査と節 ---
    let mut by_class = vec![0u64; HighwayClass::COUNT];
    let (mut car_edges, mut bike_edges, mut walk_edges) = (0u64, 0u64, 0u64);
    let (mut car_oneway, mut bike_oneway) = (0u64, 0u64);
    for edge in &edges {
        by_class[edge.class as usize] += 1;
        let a = edge.access;
        car_edges += u64::from(a & access::both(crate::contract::Mode::Car) != 0);
        bike_edges += u64::from(a & access::both(crate::contract::Mode::Bike) != 0);
        walk_edges += u64::from(a & access::both(crate::contract::Mode::Walk) != 0);
        car_oneway += u64::from((a & access::CAR_FWD != 0) != (a & access::CAR_BWD != 0));
        bike_oneway += u64::from((a & access::BIKE_FWD != 0) != (a & access::BIKE_BWD != 0));
    }
    let flag_count = |bit: u8| node_flag_bytes.iter().filter(|&&f| f & bit != 0).count() as u64;
    let class_counts: Map<String, Value> = HighwayClass::ALL
        .iter()
        .map(|c| {
            (
                c.highway_tag().to_owned(),
                json!(by_class[c.code() as usize]),
            )
        })
        .collect();
    let audit = json!({
        "ways_in_table": ways.len(),
        "way_node_refs": ways.refs.len(),
        "runs": runs,
        "run_nodes": run_nodes,
        "invalid_or_missing_refs": invalid_refs,
        "table_nodes": nodes.len(),
        "nodes": node_total,
        "node_reasons": {
            "way_endpoint": reasons[0],
            "junction": reasons[1],
            "tagged_node": reasons[2],
            "restriction_via": reasons[3],
        },
        "edges": edges.len(),
        "shape_points": shape.len(),
        "interpolated_points": interpolated,
        "edges_by_class": class_counts,
        "edges_passable": { "car": car_edges, "bike": bike_edges, "walk": walk_edges },
        "edges_oneway": { "car": car_oneway, "bike": bike_oneway },
        "node_flags": {
            "signal": flag_count(node_flags::SIGNAL),
            "crossing": flag_count(node_flags::CROSSING),
            "stop": flag_count(node_flags::STOP),
            "level_crossing": flag_count(node_flags::LEVEL_CROSSING),
            "block_car": flag_count(node_flags::BLOCK_CAR),
            "block_bike": flag_count(node_flags::BLOCK_BIKE),
            "block_walk": flag_count(node_flags::BLOCK_WALK),
        },
    });

    let mut params = Map::new();
    params.insert("rules_version".to_owned(), json!(RULES_VERSION));
    params.insert("nodes".to_owned(), json!(node_total));
    params.insert("edges".to_owned(), json!(edges.len()));
    params.insert("shape_points".to_owned(), json!(shape.len()));
    params.insert("interpolated_points".to_owned(), json!(interpolated));
    for (key, value) in extra_params {
        params.insert(key, value);
    }

    let attrs = GraphAttrs {
        params: json!({
            "nodes": node_total,
            "edges": edges.len(),
            "strtab_index": "byte_offset",
            "rules_version": RULES_VERSION,
        }),
        way_id,
        node_id: node_osm_id,
        ref_idx,
        name_idx,
        strtab: strings.finish(),
        lanes,
        width_dm,
        layer,
    };
    let parts = GraphParts {
        params: Value::Object(params),
        time_model: default_time_model(),
        node_ofs,
        adj,
        node_ll,
        node_flags: node_flag_bytes,
        edges,
        shape_ofs,
        shape,
    };
    Ok(BuiltGraph {
        parts,
        attrs,
        audit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::routing::prep::view::GraphView;

    /// 小さなテスト用の組み立て：ノードは `(id, lat_e7, lon_e7, flags)`、wayは `(id, class, access, ノードID列)`。
    struct Builder {
        nodes: Vec<(i64, i32, i32, u8)>,
        ways: Vec<(i64, HighwayClass, u8, Vec<i64>)>,
    }

    const BASE_LAT: i32 = 350_000_000;
    const BASE_LON: i32 = 1_390_000_000;

    impl Builder {
        fn new() -> Self {
            Self {
                nodes: Vec::new(),
                ways: Vec::new(),
            }
        }

        /// 北へ `north`、東へ `east`（1e-7度）
        fn node(&mut self, id: i64, north: i32, east: i32) -> &mut Self {
            self.nodes.push((id, BASE_LAT + north, BASE_LON + east, 0));
            self
        }

        fn flagged(&mut self, id: i64, north: i32, east: i32, flags: u8) -> &mut Self {
            self.nodes
                .push((id, BASE_LAT + north, BASE_LON + east, flags));
            self
        }

        fn way(&mut self, id: i64, class: HighwayClass, access: u8, nodes: &[i64]) -> &mut Self {
            self.ways.push((id, class, access, nodes.to_vec()));
            self
        }

        fn build(&self) -> BuiltGraph {
            let mut nodes: Vec<_> = self.nodes.clone();
            nodes.sort_by_key(|n| n.0);
            let table = NodeTable {
                id: nodes.iter().map(|n| n.0).collect(),
                lat_e7: nodes.iter().map(|n| n.1).collect(),
                lon_e7: nodes.iter().map(|n| n.2).collect(),
                flags: nodes.iter().map(|n| n.3).collect(),
            };
            let mut ways: Vec<_> = self.ways.clone();
            ways.sort_by_key(|w| w.0);
            let mut w = WayTable {
                ref_ofs: vec![0],
                ..WayTable::default()
            };
            for (id, class, access, refs) in &ways {
                w.id.push(*id);
                w.meta.push(WayMeta {
                    class: class.code(),
                    access: *access,
                    ..WayMeta::default()
                });
                w.ref_str.push(None);
                w.name_str.push(None);
                for r in refs {
                    w.refs.push(table.id.binary_search(r).unwrap_or_else(|_| {
                        panic!("ノード{r}が表にない");
                    }) as u32);
                }
                w.ref_ofs.push(w.refs.len() as u32);
            }
            build_graph(&w, &table, Map::new()).unwrap()
        }
    }

    const TWO_WAY: u8 = access::CAR_FWD | access::CAR_BWD;

    #[test]
    fn splits_at_junctions_and_way_ends_and_keeps_shape_points() {
        // 1--2--3--4 を way10、3--5 を way20。3が交差点。2は形状点
        let mut b = Builder::new();
        b.node(1, 0, 0)
            .node(2, 0, 1000)
            .node(3, 0, 2000)
            .node(4, 0, 3000)
            .node(5, 1000, 2000)
            .way(10, HighwayClass::Residential, TWO_WAY, &[1, 2, 3, 4])
            .way(20, HighwayClass::Residential, TWO_WAY, &[3, 5]);
        let g = b.build();
        // ノード：1,3,4,5（OSM ID昇順）。2は形状点
        assert_eq!(g.attrs.node_id, vec![1, 3, 4, 5]);
        // 辺：way10 は 1-3、3-4。way20 は 3-5
        assert_eq!(g.attrs.way_id, vec![10, 10, 20]);
        let e = &g.parts.edges;
        assert_eq!((e[0].tail, e[0].head), (0, 1));
        assert_eq!((e[1].tail, e[1].head), (1, 2));
        assert_eq!((e[2].tail, e[2].head), (1, 3));
        // 辺0 は 2 を形状点に持つ
        assert_eq!(g.parts.shape_ofs, vec![0, 1, 1, 1]);
        let view = GraphView::new(&g.parts);
        assert_eq!(
            view.polyline(0),
            vec![
                Coord::new(BASE_LAT, BASE_LON),
                Coord::new(BASE_LAT, BASE_LON + 1000),
                Coord::new(BASE_LAT, BASE_LON + 2000),
            ]
        );
        // 長さは折れ線全体（約18.2m）。形状点があっても変わらない
        assert!((180..=184).contains(&e[0].length_dm), "{}", e[0].length_dm);
        assert!((90..=92).contains(&e[1].length_dm), "{}", e[1].length_dm);
        // CSR：ノード1(OSM 3)から出る有向辺は 辺0の逆、辺1の順、辺2の順
        assert_eq!(g.parts.node_ofs, vec![0, 1, 4, 5, 6]);
        assert_eq!(
            g.parts.adj,
            vec![
                DirEdge::new(0, false),
                DirEdge::new(0, true),
                DirEdge::new(1, false),
                DirEdge::new(2, false),
                DirEdge::new(1, true),
                DirEdge::new(2, true),
            ]
        );
        // 各ノードの中は昇順
        for v in 0..4 {
            let r = &g.parts.adj[g.parts.node_ofs[v] as usize..g.parts.node_ofs[v + 1] as usize];
            assert!(r.windows(2).all(|w| w[0] < w[1]));
        }
        assert_eq!(g.parts.params["nodes"], 4);
        assert_eq!(g.parts.params["edges"], 3);
        assert_eq!(g.parts.params["rules_version"], "jp-rules-1");
    }

    #[test]
    fn node_and_edge_numbers_follow_osm_ids_not_input_order() {
        let mut b = Builder::new();
        // ノード・wayの追加順を逆にしても同じ結果になる
        b.node(9, 0, 4000)
            .node(7, 0, 2000)
            .node(5, 0, 0)
            .way(200, HighwayClass::Tertiary, TWO_WAY, &[9, 7])
            .way(100, HighwayClass::Tertiary, TWO_WAY, &[5, 7]);
        let g = b.build();
        assert_eq!(g.attrs.node_id, vec![5, 7, 9]);
        assert_eq!(g.attrs.way_id, vec![100, 200]);
        assert_eq!((g.parts.edges[0].tail, g.parts.edges[0].head), (0, 1));
        assert_eq!((g.parts.edges[1].tail, g.parts.edges[1].head), (2, 1));
    }

    #[test]
    fn tagged_nodes_and_forced_nodes_become_graph_nodes() {
        let mut b = Builder::new();
        b.node(1, 0, 0)
            .flagged(2, 0, 1000, node_flags::SIGNAL)
            .node(3, 0, 2000)
            .flagged(4, 0, 3000, node_flags::BLOCK_CAR)
            .node(5, 0, 4000)
            .flagged(6, 0, 5000, NODE_FORCE)
            .node(7, 0, 6000)
            .way(
                10,
                HighwayClass::Residential,
                TWO_WAY,
                &[1, 2, 3, 4, 5, 6, 7],
            );
        let g = b.build();
        // 端点 1,7 と、信号 2、障害物 4、viaノード 6 がグラフのノード。3,5 は形状点
        assert_eq!(g.attrs.node_id, vec![1, 2, 4, 6, 7]);
        assert_eq!(
            g.parts.node_flags,
            vec![0, node_flags::SIGNAL, node_flags::BLOCK_CAR, 0, 0]
        );
        assert_eq!(g.parts.edges.len(), 4);
        // 内部のビットは出力に残らない
        assert!(g.parts.node_flags.iter().all(|f| f & NODE_FORCE == 0));
        assert_eq!(g.audit["node_reasons"]["restriction_via"], 1);
        assert_eq!(g.audit["node_reasons"]["tagged_node"], 2);
    }

    #[test]
    fn loops_and_ring_ways_are_kept() {
        // 環状のway：1-2-3-1。1が端点で、辺は1本のループ
        let mut b = Builder::new();
        b.node(1, 0, 0).node(2, 0, 1000).node(3, 1000, 1000).way(
            10,
            HighwayClass::Residential,
            TWO_WAY,
            &[1, 2, 3, 1],
        );
        let g = b.build();
        assert_eq!(g.attrs.node_id, vec![1]);
        assert_eq!(g.parts.edges.len(), 1);
        assert_eq!((g.parts.edges[0].tail, g.parts.edges[0].head), (0, 0));
        // ループは同じノードの隣接リストに2回現れる
        assert_eq!(
            g.parts.adj,
            vec![DirEdge::new(0, false), DirEdge::new(0, true)]
        );
        assert_eq!(g.parts.shape.len(), 2);
    }

    #[test]
    fn crossing_ways_share_a_node_but_a_node_used_once_inside_a_way_does_not() {
        // 十字：way10 は 1-2-3、way20 は 4-2-5。2が交差点
        let mut b = Builder::new();
        b.node(1, 0, -1000)
            .node(2, 0, 0)
            .node(3, 0, 1000)
            .node(4, -1000, 0)
            .node(5, 1000, 0)
            .way(10, HighwayClass::Residential, TWO_WAY, &[1, 2, 3])
            .way(20, HighwayClass::Residential, TWO_WAY, &[4, 2, 5]);
        let g = b.build();
        assert_eq!(g.attrs.node_id, vec![1, 2, 3, 4, 5]);
        assert_eq!(g.parts.edges.len(), 4);
        assert_eq!(g.parts.node_ofs[2] - g.parts.node_ofs[1], 4); // 交差点の次数は4
    }

    #[test]
    fn missing_and_out_of_range_nodes_split_the_way() {
        let mut b = Builder::new();
        b.node(1, 0, 0)
            .node(2, 0, 1000)
            .node(4, 0, 3000)
            .node(5, 0, 4000);
        // 3 は座標がない（PBFにない）
        b.nodes.push((3, NODE_MISSING, NODE_MISSING, 0));
        // 6 は日本矩形の外
        b.nodes.push((6, 100_000_000, BASE_LON, 0));
        b.node(7, 0, 5000);
        b.way(10, HighwayClass::Residential, TWO_WAY, &[1, 2, 3, 4, 5])
            .way(20, HighwayClass::Residential, TWO_WAY, &[5, 6, 7]);
        let g = b.build();
        // way10 は 1-2 と 4-5 に分かれ、way20 は 5 と 7 が孤立して捨てられる
        assert_eq!(g.attrs.way_id, vec![10, 10]);
        assert_eq!(g.attrs.node_id, vec![1, 2, 4, 5]);
        assert_eq!(g.audit["invalid_or_missing_refs"], 2);
        assert_eq!(g.audit["runs"], 2);
    }

    #[test]
    fn consecutive_duplicate_nodes_are_collapsed_and_single_node_runs_dropped() {
        let mut b = Builder::new();
        b.node(1, 0, 0).node(2, 0, 1000);
        b.way(10, HighwayClass::Residential, TWO_WAY, &[1, 1, 2, 2])
            .way(20, HighwayClass::Residential, TWO_WAY, &[2, 2]);
        let g = b.build();
        assert_eq!(g.attrs.way_id, vec![10]);
        assert_eq!(g.parts.edges.len(), 1);
    }

    #[test]
    fn long_segments_get_interpolated_shape_points() {
        // 形状点の差分が±32767を超える：北へ0.01度（約1.1km）の形状点
        let mut b = Builder::new();
        b.node(1, 0, 0)
            .node(2, 100_000, 5_000)
            .node(3, 100_000, 10_000)
            .node(4, 100_000, 100_000)
            .way(10, HighwayClass::Residential, TWO_WAY, &[1, 2, 3, 4]);
        let g = b.build();
        // 辺は1本。中間点は 2,3。1→2 は差分が (100000, 5000) なので4等分（4区間）して3点を足す
        assert_eq!(g.parts.edges.len(), 1);
        let n = g.parts.shape.len();
        assert_eq!(g.parts.params["interpolated_points"], 3);
        assert_eq!(n, 5);
        for p in &g.parts.shape {
            assert!(i32::from(p.dlat_e7).abs() <= 32_767 && i32::from(p.dlon_e7).abs() <= 32_767);
        }
        // 元の点は折れ線に残り、足した点はその間を等分する
        let view = GraphView::new(&g.parts);
        let line = view.polyline(0);
        assert_eq!(line.len(), 7);
        assert_eq!(line[4], Coord::new(BASE_LAT + 100_000, BASE_LON + 5_000));
        assert_eq!(line[5], Coord::new(BASE_LAT + 100_000, BASE_LON + 10_000));
        assert_eq!(line[1], Coord::new(BASE_LAT + 25_000, BASE_LON + 1_250));
        // 長さは足す前の折れ線で決める
        let direct = crate::domain::routing::prep::geom::length_dm(&[
            Coord::new(BASE_LAT, BASE_LON),
            Coord::new(BASE_LAT + 100_000, BASE_LON + 5_000),
            Coord::new(BASE_LAT + 100_000, BASE_LON + 10_000),
            Coord::new(BASE_LAT + 100_000, BASE_LON + 100_000),
        ]);
        assert_eq!(g.parts.edges[0].length_dm, direct);
    }

    #[test]
    fn last_segment_to_the_head_node_is_not_interpolated() {
        // 最後の形状点から終点ノードへの線分は差分を持たないので、長くても点を足さない
        let mut b = Builder::new();
        b.node(1, 0, 0).node(2, 0, 1000).node(3, 0, 500_000).way(
            10,
            HighwayClass::Residential,
            TWO_WAY,
            &[1, 2, 3],
        );
        let g = b.build();
        assert_eq!(g.parts.shape.len(), 1);
        assert_eq!(g.parts.params["interpolated_points"], 0);
    }

    #[test]
    fn attrs_follow_the_edges_of_each_way() {
        let mut b = Builder::new();
        b.node(1, 0, 0).node(2, 0, 1000).node(3, 0, 2000);
        b.way(10, HighwayClass::Primary, TWO_WAY, &[1, 2, 3]);
        let mut graph = b.build();
        assert_eq!(graph.attrs.ref_idx, vec![0]);
        assert_eq!(graph.attrs.string(0), "");

        // 文字列とlanes等を持つwayを手で組む
        let nodes = NodeTable {
            id: vec![1, 2, 3],
            lat_e7: vec![BASE_LAT; 3],
            lon_e7: vec![BASE_LON, BASE_LON + 1000, BASE_LON + 2000],
            flags: vec![0, NODE_FORCE, 0],
        };
        let ways = WayTable {
            id: vec![10, 11],
            meta: vec![
                WayMeta {
                    class: HighwayClass::Primary.code(),
                    access: TWO_WAY,
                    lanes: 2,
                    width_dm: 70,
                    layer: -1,
                    ..WayMeta::default()
                },
                WayMeta {
                    class: HighwayClass::Primary.code(),
                    access: TWO_WAY,
                    ..WayMeta::default()
                },
            ],
            ref_str: vec![Some("国道1号".into()), Some("国道1号".into())],
            name_str: vec![Some("本町通り".into()), None],
            ref_ofs: vec![0, 3, 5],
            refs: vec![0, 1, 2, 0, 2],
        };
        graph = build_graph(&ways, &nodes, Map::new()).unwrap();
        // way10 は 2 で分割されて2辺、way11 は1辺
        assert_eq!(graph.attrs.way_id, vec![10, 10, 11]);
        assert_eq!(graph.attrs.lanes, vec![2, 2, 0]);
        assert_eq!(graph.attrs.width_dm, vec![70, 70, 0]);
        assert_eq!(graph.attrs.layer, vec![-1, -1, 0]);
        let r = graph.attrs.ref_idx[0];
        assert!(graph.attrs.ref_idx.iter().all(|&x| x == r));
        assert_eq!(graph.attrs.string(r), "国道1号");
        assert_eq!(graph.attrs.string(graph.attrs.name_idx[0]), "本町通り");
        assert_eq!(graph.attrs.name_idx[2], 0);
    }

    #[test]
    fn extra_params_are_added_to_graph_params() {
        let mut extra = Map::new();
        extra.insert("osm_file".to_owned(), json!("x.osm.pbf"));
        let nodes = NodeTable {
            id: vec![1, 2],
            lat_e7: vec![BASE_LAT; 2],
            lon_e7: vec![BASE_LON, BASE_LON + 1000],
            flags: vec![0, 0],
        };
        let ways = WayTable {
            id: vec![1],
            meta: vec![WayMeta {
                class: HighwayClass::Residential.code(),
                access: TWO_WAY,
                ..WayMeta::default()
            }],
            ref_str: vec![None],
            name_str: vec![None],
            ref_ofs: vec![0, 2],
            refs: vec![0, 1],
        };
        let g = build_graph(&ways, &nodes, extra).unwrap();
        assert_eq!(g.parts.params["osm_file"], "x.osm.pbf");
        assert_eq!(g.parts.params["nodes"], 2);
    }
}
