//! M1.1〜M1.4：小さなPBFで P1（osm-graph）・P2（restrictions）・P3（topology）・P7（od-index）を通す。
//!
//! 手書きのPBF（このファイルの `PbfWriter`）を `routing_prep` の各サブコマンドに渡して、
//! 4つのファイルが作られること、encode → decode の往復が一致すること、via-node・via-way・
//! `only_*` の禁止が正しい有向辺の列になること、同じ入力から同じバイト列になることを確かめる。
//!
//! 地図（単位は0.001度＝約100m、座標は北緯35度・東経139度からの差）：
//!
//! ```text
//!   T字路                                   via-way
//!            4 (1,1)                    11 --A110-- 12 --C111-- 13
//!            |  way102 (6を通る)                       |
//!   1 -100a- 5 -100b- 2 --101-- 3 --103--> 7 ==104==> 8     way112（14で分かれる）
//!   (0,0)  (0,.5)   (0,1)  (0,2)   (0,3)  (0,4)          |
//!      9 =105= 10 （歩道）                       17 -114- 14
//!                                                        |
//!                                                16 -B113- 15
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use accident_route_backend::contract::Mode;
use accident_route_backend::data::osm;
use accident_route_backend::data::release::sha256_hex;
use accident_route_backend::data::routing_files::{
    decode_graph, decode_graph_attrs, decode_graph_parts, decode_odindex, decode_restrictions,
    decode_topology, encode_graph, encode_graph_attrs, encode_odindex, encode_restrictions,
    encode_topology,
};
use accident_route_backend::domain::routing::graph::restrictions::TurnAutomaton;
use accident_route_backend::domain::routing::graph::{DirEdge, access, edge_flags, node_flags};
use serde_json::Value;

// ---------------------------------------------------------------------------
// 手書きのPBF
// ---------------------------------------------------------------------------

fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v & 0x7f) as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn tag_varint(out: &mut Vec<u8>, field: u32, v: u64) {
    varint(out, u64::from(field) << 3);
    varint(out, v);
}

fn tag_bytes(out: &mut Vec<u8>, field: u32, bytes: &[u8]) {
    varint(out, u64::from(field) << 3 | 2);
    varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn packed(out: &mut Vec<u8>, field: u32, values: impl IntoIterator<Item = u64>) {
    let mut body = Vec::new();
    for v in values {
        varint(&mut body, v);
    }
    tag_bytes(out, field, &body);
}

/// 文字列表。0番目は空の文字列。
#[derive(Default)]
struct Strings {
    list: Vec<String>,
    index: BTreeMap<String, u64>,
}

impl Strings {
    fn new() -> Self {
        let mut s = Self::default();
        s.id("");
        s
    }

    fn id(&mut self, text: &str) -> u64 {
        if let Some(&i) = self.index.get(text) {
            return i;
        }
        let i = self.list.len() as u64;
        self.list.push(text.to_owned());
        self.index.insert(text.to_owned(), i);
        i
    }

    fn table(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for s in &self.list {
            tag_bytes(&mut out, 1, s.as_bytes());
        }
        out
    }
}

type Tags<'a> = &'a [(&'a str, &'a str)];

struct NodeSpec {
    id: i64,
    lat_e7: i32,
    lon_e7: i32,
    tags: Vec<(String, String)>,
}

struct WaySpec {
    id: i64,
    refs: Vec<i64>,
    tags: Vec<(String, String)>,
}

struct RelationSpec {
    id: i64,
    tags: Vec<(String, String)>,
    /// （種類 0:node 1:way 2:relation、ID、ロール）
    members: Vec<(u64, i64, String)>,
}

#[derive(Default)]
struct PbfWriter {
    nodes: Vec<NodeSpec>,
    ways: Vec<WaySpec>,
    relations: Vec<RelationSpec>,
}

fn owned(tags: Tags<'_>) -> Vec<(String, String)> {
    tags.iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

impl PbfWriter {
    fn node(&mut self, id: i64, lat_e7: i32, lon_e7: i32, tags: Tags<'_>) {
        self.nodes.push(NodeSpec {
            id,
            lat_e7,
            lon_e7,
            tags: owned(tags),
        });
    }

    fn way(&mut self, id: i64, refs: &[i64], tags: Tags<'_>) {
        self.ways.push(WaySpec {
            id,
            refs: refs.to_vec(),
            tags: owned(tags),
        });
    }

    fn relation(&mut self, id: i64, tags: Tags<'_>, members: &[(u64, i64, &str)]) {
        self.relations.push(RelationSpec {
            id,
            tags: owned(tags),
            members: members
                .iter()
                .map(|(k, i, r)| (*k, *i, (*r).to_owned()))
                .collect(),
        });
    }

    fn blob(kind: &str, body: &[u8], out: &mut Vec<u8>) {
        let mut blob = Vec::new();
        tag_bytes(&mut blob, 1, body); // raw
        tag_varint(&mut blob, 2, body.len() as u64); // raw_size
        let mut header = Vec::new();
        tag_bytes(&mut header, 1, kind.as_bytes());
        tag_varint(&mut header, 3, blob.len() as u64);
        out.extend_from_slice(&(header.len() as u32).to_be_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(&blob);
    }

    fn primitive_block(strings: &Strings, groups: &[Vec<u8>]) -> Vec<u8> {
        let mut block = Vec::new();
        tag_bytes(&mut block, 1, &strings.table());
        for g in groups {
            tag_bytes(&mut block, 2, g);
        }
        tag_varint(&mut block, 17, 100); // granularity
        block
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        // ヘッダー
        let mut header = Vec::new();
        tag_bytes(&mut header, 4, b"OsmSchema-V0.6");
        tag_bytes(&mut header, 4, b"DenseNodes");
        tag_bytes(&mut header, 16, b"routing_prep_osm test");
        tag_varint(&mut header, 32, 1_700_000_000);
        Self::blob("OSMHeader", &header, &mut out);

        // ブロック1：ノード（密な形式）
        let mut nodes: Vec<&NodeSpec> = self.nodes.iter().collect();
        nodes.sort_by_key(|n| n.id);
        let mut strings = Strings::new();
        let (mut ids, mut lats, mut lons, mut keys_vals) = (vec![], vec![], vec![], vec![]);
        let (mut prev_id, mut prev_lat, mut prev_lon) = (0i64, 0i64, 0i64);
        for n in &nodes {
            ids.push(zigzag(n.id - prev_id));
            lats.push(zigzag(i64::from(n.lat_e7) - prev_lat));
            lons.push(zigzag(i64::from(n.lon_e7) - prev_lon));
            (prev_id, prev_lat, prev_lon) = (n.id, i64::from(n.lat_e7), i64::from(n.lon_e7));
            for (k, v) in &n.tags {
                keys_vals.push(strings.id(k));
                keys_vals.push(strings.id(v));
            }
            keys_vals.push(0);
        }
        let mut dense = Vec::new();
        packed(&mut dense, 1, ids);
        packed(&mut dense, 8, lats);
        packed(&mut dense, 9, lons);
        packed(&mut dense, 10, keys_vals);
        let mut group = Vec::new();
        tag_bytes(&mut group, 2, &dense);
        Self::blob(
            "OSMData",
            &Self::primitive_block(&strings, &[group]),
            &mut out,
        );

        // ブロック2：way と relation
        let mut strings = Strings::new();
        let mut ways: Vec<&WaySpec> = self.ways.iter().collect();
        ways.sort_by_key(|w| w.id);
        let mut way_group = Vec::new();
        for w in ways {
            let mut msg = Vec::new();
            tag_varint(&mut msg, 1, w.id as u64);
            let keys: Vec<u64> = w.tags.iter().map(|(k, _)| strings.id(k)).collect();
            let vals: Vec<u64> = w.tags.iter().map(|(_, v)| strings.id(v)).collect();
            packed(&mut msg, 2, keys);
            packed(&mut msg, 3, vals);
            let mut prev = 0i64;
            packed(
                &mut msg,
                8,
                w.refs.iter().map(|&r| {
                    let d = zigzag(r - prev);
                    prev = r;
                    d
                }),
            );
            tag_bytes(&mut way_group, 3, &msg);
        }
        let mut relations: Vec<&RelationSpec> = self.relations.iter().collect();
        relations.sort_by_key(|r| r.id);
        let mut relation_group = Vec::new();
        for r in relations {
            let mut msg = Vec::new();
            tag_varint(&mut msg, 1, r.id as u64);
            let keys: Vec<u64> = r.tags.iter().map(|(k, _)| strings.id(k)).collect();
            let vals: Vec<u64> = r.tags.iter().map(|(_, v)| strings.id(v)).collect();
            packed(&mut msg, 2, keys);
            packed(&mut msg, 3, vals);
            let roles: Vec<u64> = r
                .members
                .iter()
                .map(|(_, _, role)| strings.id(role))
                .collect();
            packed(&mut msg, 8, roles);
            let mut prev = 0i64;
            packed(
                &mut msg,
                9,
                r.members.iter().map(|(_, id, _)| {
                    let d = zigzag(id - prev);
                    prev = *id;
                    d
                }),
            );
            packed(&mut msg, 10, r.members.iter().map(|(kind, _, _)| *kind));
            tag_bytes(&mut relation_group, 4, &msg);
        }
        Self::blob(
            "OSMData",
            &Self::primitive_block(&strings, &[way_group, relation_group]),
            &mut out,
        );
        out
    }

    fn write_to(&self, path: &Path) {
        fs::write(path, self.encode()).unwrap();
    }
}

// ---------------------------------------------------------------------------
// テストデータ
// ---------------------------------------------------------------------------

const LAT0: i32 = 350_000_000;
const LON0: i32 = 1_390_000_000;
/// 0.001度
const UNIT: i32 = 10_000;

const NODE: u64 = 0;
const WAY: u64 = 1;

fn network() -> PbfWriter {
    let mut p = PbfWriter::default();
    let at = |y: f64, x: f64| {
        (
            LAT0 + (y * f64::from(UNIT)) as i32,
            LON0 + (x * f64::from(UNIT)) as i32,
        )
    };
    let node = |p: &mut PbfWriter, id: i64, y: f64, x: f64, tags: Tags<'_>| {
        let (lat, lon) = at(y, x);
        p.node(id, lat, lon, tags);
    };

    // T字路
    node(&mut p, 1, 0.0, 0.0, &[]);
    node(&mut p, 5, 0.0, 0.5, &[("barrier", "bollard")]);
    node(&mut p, 2, 0.0, 1.0, &[("highway", "traffic_signals")]);
    node(&mut p, 3, 0.0, 2.0, &[]);
    node(&mut p, 4, 1.0, 1.0, &[]);
    node(&mut p, 6, 0.5, 1.2, &[]); // way102 の形状点
    node(&mut p, 7, 0.0, 3.0, &[]);
    node(&mut p, 8, 0.0, 4.0, &[]);
    // 歩道（way100b の約11m北、x=0.55〜0.95）
    node(&mut p, 9, 0.1, 0.55, &[]);
    node(&mut p, 10, 0.1, 0.95, &[]);
    let r = [("highway", "residential")];
    p.way(100, &[1, 5, 2], &r);
    p.way(101, &[2, 3], &r);
    p.way(102, &[2, 6, 4], &r);
    p.way(103, &[3, 7], &[("highway", "secondary"), ("oneway", "yes")]);
    p.way(104, &[7, 8], &[("highway", "motorway")]);
    p.way(
        105,
        &[9, 10],
        &[("highway", "footway"), ("footway", "sidewalk")],
    );

    // via-way
    node(&mut p, 11, -5.0, 0.0, &[]);
    node(&mut p, 12, -5.0, 1.0, &[]);
    node(&mut p, 13, -5.0, 2.0, &[]);
    node(&mut p, 14, -6.0, 1.0, &[]);
    node(&mut p, 15, -7.0, 1.0, &[]);
    node(&mut p, 16, -7.0, 2.0, &[]);
    node(&mut p, 17, -6.0, 2.0, &[]);
    p.way(110, &[11, 12], &r);
    p.way(111, &[12, 13], &r);
    p.way(112, &[12, 14, 15], &r);
    p.way(113, &[15, 16], &r);
    p.way(114, &[14, 17], &r);

    // グラフに入らないway（理由別の件数を確かめる）
    p.way(900, &[1, 2], &[("building", "yes")]);
    p.way(901, &[1, 2], &[("highway", "construction")]);
    p.way(
        902,
        &[1, 2],
        &[("highway", "residential"), ("access", "no")],
    );
    p.way(
        903,
        &[1, 2, 4],
        &[("highway", "pedestrian"), ("area", "yes")],
    );

    // 右左折禁止
    p.relation(
        1000,
        &[("type", "restriction"), ("restriction", "no_left_turn")],
        &[(WAY, 100, "from"), (NODE, 2, "via"), (WAY, 102, "to")],
    );
    p.relation(
        1001,
        &[("type", "restriction"), ("restriction", "only_straight_on")],
        &[(WAY, 100, "from"), (NODE, 2, "via"), (WAY, 101, "to")],
    );
    p.relation(
        1002,
        &[("type", "restriction"), ("restriction", "no_u_turn")],
        &[(WAY, 101, "from"), (NODE, 2, "via"), (WAY, 101, "to")],
    );
    p.relation(
        1010,
        &[("type", "restriction"), ("restriction", "no_right_turn")],
        &[(WAY, 110, "from"), (WAY, 112, "via"), (WAY, 113, "to")],
    );
    p.relation(
        1011,
        &[
            ("type", "restriction"),
            ("restriction:motorcar", "only_straight_on"),
        ],
        &[(WAY, 113, "from"), (WAY, 112, "via"), (WAY, 110, "to")],
    );
    // 数えるだけのもの
    p.relation(
        1020,
        &[
            ("type", "restriction"),
            (
                "restriction:conditional",
                "no_left_turn @ (Mo-Fr 07:00-09:00)",
            ),
        ],
        &[(WAY, 100, "from"), (NODE, 2, "via"), (WAY, 102, "to")],
    );
    // 右左折禁止ではないrelation（読み飛ばす）
    p.relation(
        1030,
        &[("type", "route"), ("route", "bus")],
        &[(WAY, 100, "")],
    );
    p
}

fn d(edge: u32, reverse: bool) -> DirEdge {
    DirEdge::new(edge, reverse)
}

// ---------------------------------------------------------------------------
// サブコマンドの実行
// ---------------------------------------------------------------------------

fn run(args: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_routing_prep"))
        .args(args)
        .output()
        .expect("routing_prepを起動できる");
    assert!(
        output.status.success(),
        "routing_prep {args:?} が失敗しました\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct Built {
    dir: PathBuf,
}

impl Built {
    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn bytes(&self, name: &str) -> Vec<u8> {
        fs::read(self.path(name)).unwrap()
    }

    fn json(&self, name: &str) -> Value {
        serde_json::from_slice(&self.bytes(name)).unwrap()
    }
}

/// PBFを書いて、4つのサブコマンドを順に実行する。
fn build_all(dir: &Path, pbf: &PbfWriter) -> Built {
    fs::create_dir_all(dir).unwrap();
    let pbf_path = dir.join("test.osm.pbf");
    pbf.write_to(&pbf_path);
    let (pbf_s, dir_s) = (pbf_path.to_str().unwrap(), dir.to_str().unwrap());
    let p = |name: &str| dir.join(name).to_str().unwrap().to_owned();
    run(&[
        "osm-graph",
        "--pbf",
        pbf_s,
        "--output-dir",
        dir_s,
        "--threads",
        "2",
    ]);
    run(&[
        "restrictions",
        "--pbf",
        pbf_s,
        "--graph",
        &p("graph.bin"),
        "--attrs",
        &p("graph_attrs.bin"),
        "--output",
        &p("restrictions.bin"),
        "--threads",
        "2",
    ]);
    run(&[
        "topology",
        "--graph",
        &p("graph.bin"),
        "--attrs",
        &p("graph_attrs.bin"),
        "--output",
        &p("topology.bin"),
        "--scc-min-nodes",
        "3",
        "--threads",
        "2",
    ]);
    run(&[
        "od-index",
        "--graph",
        &p("graph.bin"),
        "--output",
        &p("odindex.bin"),
        "--threads",
        "2",
    ]);
    Built {
        dir: dir.to_path_buf(),
    }
}

// ---------------------------------------------------------------------------
// 検査
// ---------------------------------------------------------------------------

#[test]
fn graph_has_the_expected_nodes_edges_and_attributes() {
    let tmp = tempfile::tempdir().unwrap();
    let built = build_all(tmp.path(), &network());
    let graph_bytes = built.bytes("graph.bin");
    let parts = decode_graph_parts(&graph_bytes).unwrap().value;
    let attrs = decode_graph_attrs(&built.bytes("graph_attrs.bin"))
        .unwrap()
        .value;

    // ノードはOSMノードIDの昇順。6（形状点）は含まない
    assert_eq!(
        attrs.node_id,
        vec![1, 2, 3, 4, 5, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17]
    );
    // 辺はwayのIDの昇順、way内は先頭から。100は障害物のノード5で2辺、112は14で2辺
    assert_eq!(
        attrs.way_id,
        vec![
            100, 100, 101, 102, 103, 104, 105, 110, 111, 112, 112, 113, 114
        ]
    );
    assert_eq!(parts.params["nodes"], 16);
    assert_eq!(parts.params["edges"], 13);
    assert_eq!(parts.params["rules_version"], "jp-rules-1");
    assert_eq!(
        parts.params["osm"]["osmosis_replication_timestamp"],
        1_700_000_000
    );
    assert_eq!(
        parts.params["osm"]["sha256"],
        built_sha(&built, "test.osm.pbf")
    );
    let excluded = &parts.params["excluded_ways"];
    assert_eq!(excluded["not_highway"], 1);
    assert_eq!(excluded["unsupported_highway"], 1);
    assert_eq!(excluded["no_access"], 1);
    assert_eq!(excluded["area_yes"], 1);

    // ノードのフラグ
    let node_of = |osm: i64| attrs.node_by_osm_id(osm).unwrap() as usize;
    assert_eq!(parts.node_flags[node_of(2)], node_flags::SIGNAL);
    assert_eq!(parts.node_flags[node_of(5)], node_flags::BLOCK_CAR);
    assert_eq!(parts.node_flags[node_of(1)], 0);

    // 辺の属性
    let edge = |way: i64, nth: usize| attrs.way_edges(way).start as usize + nth;
    let secondary = parts.edges[edge(103, 0)];
    assert_eq!(
        secondary.access,
        access::CAR_FWD | access::BIKE_FWD | access::WALK_FWD | access::WALK_BWD
    );
    assert_eq!(parts.edges[edge(104, 0)].access, access::CAR_FWD);
    assert_eq!(
        parts.edges[edge(105, 0)].access,
        access::WALK_FWD | access::WALK_BWD
    );
    assert_eq!(parts.edges[edge(105, 0)].flags, edge_flags::SIDEWALK);
    // way102 は 6 を形状点に持つ
    let e102 = edge(102, 0);
    let (start, end) = (
        parts.shape_ofs[e102] as usize,
        parts.shape_ofs[e102 + 1] as usize,
    );
    assert_eq!(end - start, 1);
    let r = accident_route_backend::domain::routing::prep::view::GraphView::new(&parts);
    let line = r.polyline(e102 as u32);
    assert_eq!(line.len(), 3);
    assert_eq!(line[1].lat_e7, LAT0 + UNIT / 2);
    assert_eq!(line[1].lon_e7, LON0 + UNIT * 6 / 5);
    // 100a は x=0〜0.5（約45.6m）、100b は 0.5〜1.0
    assert!((455..=458).contains(&parts.edges[edge(100, 0)].length_dm));

    // Graph::from_parts の検査にも通る。往復でバイト列が変わらない
    let decoded = decode_graph(&graph_bytes).unwrap();
    assert_eq!(decoded.value.parts(), &parts);
    assert_eq!(encode_graph(&decoded.value), graph_bytes);
    let attrs_bytes = built.bytes("graph_attrs.bin");
    let attrs_decoded = decode_graph_attrs(&attrs_bytes).unwrap();
    assert_eq!(
        encode_graph_attrs(&attrs_decoded.value, &attrs_decoded.deps),
        attrs_bytes
    );
    assert_eq!(attrs_decoded.deps["graph.bin"], sha256_hex(&graph_bytes));
    // 監査JSON
    let audit = built.json("graph.audit.json");
    assert_eq!(audit["audit"]["nodes"], 16);
    assert_eq!(audit["audit"]["node_reasons"]["tagged_node"], 1); // 障害物の 5 だけ（信号の 2 は端点でもある）
}

fn built_sha(built: &Built, name: &str) -> String {
    sha256_hex(&built.bytes(name))
}

#[test]
fn restrictions_become_directed_edge_sequences() {
    let tmp = tempfile::tempdir().unwrap();
    let built = build_all(tmp.path(), &network());
    let graph_bytes = built.bytes("graph.bin");
    let graph = decode_graph(&graph_bytes).unwrap().value;
    let attrs = decode_graph_attrs(&built.bytes("graph_attrs.bin"))
        .unwrap()
        .value;
    let bytes = built.bytes("restrictions.bin");
    let decoded = decode_restrictions(&bytes).unwrap();
    assert_eq!(decoded.deps["graph.bin"], sha256_hex(&graph_bytes));
    decoded.value.validate(&graph).unwrap();

    // 辺番号：100a=0、100b=1、101=2、102=3、103=4、104=5、105=6、110=7、111=8、112a=9、112b=10、113=11、114=12
    let car_bike = Mode::Car.mask() | Mode::Bike.mask();
    let expected: Vec<(Vec<DirEdge>, u8, u8)> = vec![
        // 1001 only_straight_on：100→101 以外を禁止（Uターンと 102）。1000 no_left_turn（100→102）と重なる
        (vec![d(1, false), d(1, true)], car_bike, 0),
        (vec![d(1, false), d(3, false)], car_bike, 0),
        // 1002 no_u_turn：101 を 2 に向かって入り、同じ 101 を戻る
        (vec![d(2, true), d(2, false)], car_bike, 0),
        // 1010 no_right_turn（via-way）：110 → 112（2辺）→ 113
        (
            vec![d(7, false), d(9, false), d(10, false), d(11, false)],
            car_bike,
            1,
        ),
        // 1011 only_straight_on（restriction:motorcar、via-way）：113 → 112（逆向き）→ 110 以外を禁止
        (
            vec![d(11, true), d(10, true), d(9, true), d(8, false)],
            Mode::Car.mask(),
            1,
        ),
        (
            vec![d(11, true), d(10, true), d(9, true), d(9, false)],
            Mode::Car.mask(),
            1,
        ),
    ];
    let actual: Vec<(Vec<DirEdge>, u8, u8)> = decoded
        .value
        .items
        .iter()
        .map(|r| (r.seq.clone(), r.modes, r.kind))
        .collect();
    assert_eq!(actual, expected);
    // 並び順は列の辞書順
    assert!(actual.windows(2).all(|w| {
        w[0].0.iter().map(|d| d.0).collect::<Vec<_>>()
            < w[1].0.iter().map(|d| d.0).collect::<Vec<_>>()
    }));
    // 辺の元のwayで確かめる（読みやすさのため）
    let names = |seq: &[DirEdge]| -> Vec<i64> {
        seq.iter()
            .map(|d| attrs.way_id[d.edge() as usize])
            .collect()
    };
    assert_eq!(names(&expected[3].0), vec![110, 112, 112, 113]);
    assert_eq!(names(&expected[0].0), vec![100, 100]);

    // 監査
    let audit = &decoded.value.params;
    assert_eq!(audit["relations_total"], 6);
    assert_eq!(audit["conditional_ignored"], 1);
    assert_eq!(audit["relations_without_supported_tag"], 1);
    assert_eq!(audit["resolved"]["via_node_no"], 2);
    assert_eq!(audit["resolved"]["via_node_only"], 1);
    assert_eq!(audit["resolved"]["via_way_no"], 1);
    assert_eq!(audit["resolved"]["via_way_only"], 1);
    assert_eq!(audit["unresolved_total"], 0);
    assert_eq!(audit["only_derived_prohibitions"], 4);
    assert_eq!(audit["items"], 6);

    // 往復でバイト列が変わらない
    assert_eq!(encode_restrictions(&decoded.value, &decoded.deps), bytes);

    // 照合器：1010 の列を通る経路だけが止まる（carとbike）
    let automaton = TurnAutomaton::build(&decoded.value, Mode::Car);
    let walk = |seq: &[DirEdge]| {
        let mut state = automaton.start();
        for &d in seq {
            match automaton.step(state, d) {
                Some(next) => state = next,
                None => return false,
            }
        }
        true
    };
    assert!(!walk(&[
        d(7, false),
        d(9, false),
        d(10, false),
        d(11, false)
    ]));
    // 途中で 112 を降りて 114 へ向かう（14 から 114）のは禁止されない
    assert!(walk(&[d(7, false), d(9, false), d(12, false)]));
    // walkには適用しない
    assert_eq!(
        TurnAutomaton::build(&decoded.value, Mode::Walk).state_count(),
        1
    );
}

#[test]
fn topology_and_odindex_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let built = build_all(tmp.path(), &network());
    let graph_bytes = built.bytes("graph.bin");
    let graph = decode_graph(&graph_bytes).unwrap().value;
    let attrs = decode_graph_attrs(&built.bytes("graph_attrs.bin"))
        .unwrap()
        .value;

    // topology
    let bytes = built.bytes("topology.bin");
    let decoded = decode_topology(&bytes).unwrap();
    decoded.value.validate(&graph).unwrap();
    assert_eq!(decoded.deps["graph.bin"], sha256_hex(&graph_bytes));
    assert_eq!(encode_topology(&decoded.value, &decoded.deps), bytes);
    let topology = &decoded.value;
    let node_of = |osm: i64| attrs.node_by_osm_id(osm).unwrap();
    // 交差点ノード（carの次数3以上）：2、12、14。20mを超えて離れているので別々のクラスタ
    assert_eq!(topology.cluster_count(), 3);
    for osm in [2, 12, 14] {
        assert!(topology.cluster_of(node_of(osm)).is_some(), "{osm}");
    }
    assert!(topology.cluster_of(node_of(3)).is_none());
    let c2 = &topology.clusters[topology.cluster_of(node_of(2)).unwrap() as usize];
    assert_eq!(c2.branches, 3);
    assert_eq!(c2.flags & 1, 1); // 信号あり
    assert_eq!(c2.top_class, 11); // residential
    // 歩道の親は 100b（辺1）
    assert_eq!(topology.sidewalk_parent(6), Some(1));
    assert_eq!(topology.sidewalk_parent(1), None);
    // 吸着：しきい値3ノード。T字路の6ノード（1,5,2,3,4 と、3から先の 7・8 は一方通行）のうち、
    // 両方向に通れる成分の辺は吸着できる。一方通行の 103・104（成分が1ノード）は吸着できない
    // （P3は、通過できないノード（車止め）を通り抜けられないものとして成分を求める。辺0（ノード1と車止め5）は、
    // 車止めの向こうの行き止まり（ノード1）なので吸着できない。辺1（車止め5とノード2）は、もう一方の端点が
    // 大きな成分に入っているので吸着できる）
    assert!(!topology.snappable(0, Mode::Car));
    assert!(topology.snappable(1, Mode::Car) && topology.snappable(3, Mode::Car));
    assert!(!topology.snappable(4, Mode::Car) && !topology.snappable(5, Mode::Car));
    // 歩行者だけの歩道は、2ノードの成分なのでwalkでも吸着できない
    assert!(!topology.snappable(6, Mode::Walk));
    assert!(!topology.snappable(6, Mode::Car));

    // odindex
    let bytes = built.bytes("odindex.bin");
    let decoded = decode_odindex(&bytes).unwrap();
    assert_eq!(decoded.deps["graph.bin"], sha256_hex(&graph_bytes));
    assert_eq!(encode_odindex(&decoded.value, &decoded.deps), bytes);
    let index = &decoded.value;
    assert_eq!((index.params.rows, index.params.cols), (10_401, 12_801));
    // すべての辺が、折れ線の頂点を含むセルに登録されている
    for e in 0..graph.edge_count() as u32 {
        for c in graph.edge_polyline(e) {
            let key = index.cell_key(c).unwrap();
            assert!(index.edges_in_cell(key).contains(&e), "edge {e}");
        }
    }
    // 辺1（100b、東西に約50m）は、0.0025度のセルをまたがない（同じセルに収まる）
    let key = index
        .cell_key(graph.node_coord(attrs.node_by_osm_id(2).unwrap()))
        .unwrap();
    assert!(index.edges_in_cell(key).contains(&1));
}

/// 柵・車止め（`barrier=bollard`）のノードの向こうの行き止まりは、carの吸着先にならない（P3の規則）。
/// 旧い規則（障害物を無視して強連結成分を求める）では、車止めの向こうも大きな成分に入って吸着できてしまい、
/// 着けない目的地になる。walk・bikeは車止めを通れるので、吸着できる。
#[test]
fn topology_excludes_dead_ends_behind_barrier_nodes_for_the_blocked_mode() {
    let mut p = PbfWriter::default();
    let at = |y: f64, x: f64| {
        (
            LAT0 + (y * f64::from(UNIT)) as i32,
            LON0 + (x * f64::from(UNIT)) as i32,
        )
    };
    let mut node = |id: i64, y: f64, x: f64, tags: Tags<'_>| {
        let (lat, lon) = at(y, x);
        p.node(id, lat, lon, tags);
    };
    // 本線：4ノードの環（しきい値3以上）。2〜4は信号を付けて、グラフのノードにする（辺を区切る）
    let signal = [("highway", "traffic_signals")];
    node(1, 0.0, 0.0, &[]);
    node(2, 0.0, 1.0, &signal);
    node(3, 1.0, 1.0, &signal);
    node(4, 1.0, 0.0, &signal);
    // 環のノード1から南へ、車止め（10）を通って行き止まり（12）へ
    node(10, -0.5, 0.0, &[("barrier", "bollard")]);
    node(11, -0.8, 0.1, &[]);
    node(12, -1.0, 0.0, &[]);
    let r = [("highway", "residential")];
    p.way(200, &[1, 2, 3, 4, 1], &r);
    p.way(201, &[1, 10, 11, 12], &r);

    let tmp = tempfile::tempdir().unwrap();
    let built = build_all(tmp.path(), &p);
    let graph = decode_graph(&built.bytes("graph.bin")).unwrap().value;
    let attrs = decode_graph_attrs(&built.bytes("graph_attrs.bin"))
        .unwrap()
        .value;
    let topology = decode_topology(&built.bytes("topology.bin")).unwrap().value;
    topology.validate(&graph).unwrap();
    let node_of = |osm: i64| attrs.node_by_osm_id(osm).unwrap();
    let edge_between = |a: i64, b: i64| {
        (0..graph.edge_count() as u32)
            .find(|&e| {
                let r = graph.edge(e);
                (r.tail, r.head) == (node_of(a), node_of(b))
                    || (r.tail, r.head) == (node_of(b), node_of(a))
            })
            .unwrap_or_else(|| panic!("ノード{a}と{b}の間の辺がない"))
    };
    let gate = edge_between(1, 10); // 環のノードと車止めの間
    let dead_end = edge_between(10, 12); // 車止めの向こうの行き止まり
    assert!(graph.node_blocked(node_of(10), Mode::Car));
    assert!(!graph.node_blocked(node_of(10), Mode::Walk));
    // car：車止めのノードを端点に持つ辺でも、もう一方が大きな成分なら吸着できる。向こうの行き止まりは吸着できない
    assert!(topology.snappable(gate, Mode::Car));
    assert!(!topology.snappable(dead_end, Mode::Car));
    // bike・walk：車止めを通れるので、成分は環と行き止まりを含む6ノード
    for mode in [Mode::Bike, Mode::Walk] {
        assert!(topology.snappable(gate, mode), "{mode:?}");
        assert!(topology.snappable(dead_end, mode), "{mode:?}");
    }
    // PARAMS：規則と、障害物を見ない旧い規則との差
    let params = &topology.params;
    assert!(
        params["config"]["snap_barrier_rule"]
            .as_str()
            .unwrap()
            .starts_with("scc-without-barrier-nodes")
    );
    let car = &params["snap"]["car"];
    assert_eq!(
        car["edges_snappable_if_barriers_ignored"].as_u64().unwrap(),
        car["edges_snappable"].as_u64().unwrap() + 1
    );
    assert_eq!(
        params["snap"]["walk"]["edges_snappable_if_barriers_ignored"],
        params["snap"]["walk"]["edges_snappable"]
    );
}

#[test]
fn same_input_gives_identical_files() {
    let tmp_a = tempfile::tempdir().unwrap();
    let tmp_b = tempfile::tempdir().unwrap();
    let a = build_all(tmp_a.path(), &network());
    let b = build_all(tmp_b.path(), &network());
    for name in [
        "graph.bin",
        "graph_attrs.bin",
        "restrictions.bin",
        "topology.bin",
        "odindex.bin",
    ] {
        assert_eq!(a.bytes(name), b.bytes(name), "{name}");
    }
}

#[test]
fn reader_returns_ways_nodes_and_relations() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("t.osm.pbf");
    network().write_to(&path);
    let relations = osm::read_restriction_relations(&path).unwrap();
    assert_eq!(
        relations.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![1000, 1001, 1002, 1010, 1011, 1020]
    );
    assert_eq!(relations[3].members.len(), 3);
    assert_eq!(relations[3].members[1].role, "via");
    let info = osm::pbf_info(&path).unwrap();
    assert_eq!(info.replication_timestamp, Some(1_700_000_000));
    assert_eq!(info.bytes, fs::metadata(&path).unwrap().len());
    let raw = osm::read_ways(&path).unwrap();
    assert_eq!(raw.ways_seen, 15);
    assert_eq!(raw.id.len(), 11); // 辺ではなくway（100〜105、110〜114）
    assert!(raw.id.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(raw.excluded.not_highway, 1);
}

#[test]
fn bad_inputs_fail_with_a_message() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("nothing.pbf");
    let out = Command::new(env!("CARGO_BIN_EXE_routing_prep"))
        .args([
            "osm-graph",
            "--pbf",
            missing.to_str().unwrap(),
            "--output-dir",
            tmp.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!String::from_utf8_lossy(&out.stderr).is_empty());
    // 引数の不足
    let out = Command::new(env!("CARGO_BIN_EXE_routing_prep"))
        .args(["od-index"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

/// 全国のPBFから作った4つのファイル（`ROUTING_OSM_DIR` に置いたもの）を読み、規模が `plan2.md` の実測
/// （1,476万ノード、2,003万辺）から±5%以内であることを確かめる。重いので既定では動かさない。
///
/// ```sh
/// ROUTING_OSM_DIR=data/work/M1/osm cargo test --release --test routing_prep_osm -- --ignored
/// ```
#[test]
#[ignore = "全国のPBFから作ったファイルが必要（ROUTING_OSM_DIR）"]
fn nationwide_files_match_the_plan_scale() {
    let Ok(dir) = std::env::var("ROUTING_OSM_DIR") else {
        panic!("ROUTING_OSM_DIR に graph.bin などのあるディレクトリを指定してください");
    };
    let built = Built {
        dir: PathBuf::from(dir),
    };
    let graph = decode_graph(&built.bytes("graph.bin")).unwrap().value;
    let restrictions = decode_restrictions(&built.bytes("restrictions.bin"))
        .unwrap()
        .value;
    restrictions.validate(&graph).unwrap();
    let topology = decode_topology(&built.bytes("topology.bin")).unwrap().value;
    topology.validate(&graph).unwrap();
    decode_odindex(&built.bytes("odindex.bin")).unwrap();
    let within = |actual: usize, expected: f64| {
        let ratio = actual as f64 / expected;
        assert!(
            (0.95..=1.05).contains(&ratio),
            "{actual} / {expected} = {ratio}"
        );
    };
    within(graph.node_count(), 14_761_413.0);
    within(graph.edge_count(), 20_026_942.0);
    assert!(!restrictions.items.is_empty());
}
