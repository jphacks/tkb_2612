//! 共通グラフ（形式書5〜6章）。ノード・辺・有向辺、`graph.bin` の節に対応する `Graph`。

pub mod restrictions;
pub mod rules;
pub mod synthetic;
pub mod time;

use std::fmt;

use serde_json::Value;

use crate::contract::{LatLon, Mode};
use crate::domain::routing::NONE;

use self::time::TimeModel;

/// ノード番号（形式書5章）。`NONE` は「なし」。
pub type NodeId = u32;
/// 辺番号（形式書1章）。`graph.bin` の `EDGE` 節での順番。
pub type EdgeId = u32;

/// 座標（形式書1章）。1e-7度単位の整数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Coord {
    pub lat_e7: i32,
    pub lon_e7: i32,
}

impl Coord {
    /// `NODELL` 節の1要素の大きさ（バイト）。
    pub const SIZE: usize = 8;

    pub const fn new(lat_e7: i32, lon_e7: i32) -> Self {
        Self { lat_e7, lon_e7 }
    }

    /// 十進度へ。`x as f64 / 1e7`（形式書1章）。
    pub fn to_latlon(self) -> LatLon {
        LatLon::new(f64::from(self.lat_e7) / 1e7, f64::from(self.lon_e7) / 1e7)
    }

    /// 十進度から1e-7度に四捨五入して作る。
    pub fn from_latlon(p: LatLon) -> Self {
        Self {
            lat_e7: (p.lat * 1e7).round() as i32,
            lon_e7: (p.lon * 1e7).round() as i32,
        }
    }

    /// 日本矩形（形式書5章。緯度20〜46度・経度122〜154度、両端を含む）の内側か。
    pub const fn in_japan(self) -> bool {
        self.lat_e7 >= LAT_E7_MIN
            && self.lat_e7 <= LAT_E7_MAX
            && self.lon_e7 >= LON_E7_MIN
            && self.lon_e7 <= LON_E7_MAX
    }
}

/// 日本矩形（形式書5章）の南端（1e-7度）。
pub const LAT_E7_MIN: i32 = 200_000_000;
/// 日本矩形の北端（1e-7度）。
pub const LAT_E7_MAX: i32 = 460_000_000;
/// 日本矩形の西端（1e-7度）。
pub const LON_E7_MIN: i32 = 1_220_000_000;
/// 日本矩形の東端（1e-7度）。
pub const LON_E7_MAX: i32 = 1_540_000_000;

/// 有向辺（形式書1章）：`2×辺番号 + 向き`。向き0は辺の始点→終点、1は終点→始点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DirEdge(pub u32);

impl DirEdge {
    pub const fn new(edge: EdgeId, reverse: bool) -> Self {
        Self(edge * 2 + reverse as u32)
    }

    /// 辺番号。
    pub const fn edge(self) -> EdgeId {
        self.0 >> 1
    }

    /// 向きが1（終点→始点）か。
    pub const fn is_reverse(self) -> bool {
        self.0 & 1 == 1
    }

    /// 逆向きの有向辺（`d ^ 1`）。
    pub const fn reversed(self) -> Self {
        Self(self.0 ^ 1)
    }
}

/// 辺の属性 `access` のビット（形式書6.2節）。手段×方向の通行可否。
pub mod access {
    use crate::contract::Mode;

    /// car：始点→終点。
    pub const CAR_FWD: u8 = 0x01;
    /// car：終点→始点。
    pub const CAR_BWD: u8 = 0x02;
    /// bike：始点→終点。
    pub const BIKE_FWD: u8 = 0x04;
    /// bike：終点→始点。
    pub const BIKE_BWD: u8 = 0x08;
    /// walk：始点→終点。
    pub const WALK_FWD: u8 = 0x10;
    /// walk：終点→始点。
    pub const WALK_BWD: u8 = 0x20;
    /// 使うビットの全体。ビット6〜7は0。
    pub const ALL: u8 = 0x3f;

    /// 手段と向き（`reverse` が終点→始点）に対応するビット。
    pub const fn bit(mode: Mode, reverse: bool) -> u8 {
        match (mode, reverse) {
            (Mode::Car, false) => CAR_FWD,
            (Mode::Car, true) => CAR_BWD,
            (Mode::Bike, false) => BIKE_FWD,
            (Mode::Bike, true) => BIKE_BWD,
            (Mode::Walk, false) => WALK_FWD,
            (Mode::Walk, true) => WALK_BWD,
        }
    }

    /// 手段の両方向のビット。
    pub const fn both(mode: Mode) -> u8 {
        bit(mode, false) | bit(mode, true)
    }
}

/// 辺の属性 `flags` のビット（形式書6.2節）。ビット7は0（予約）。
pub mod edge_flags {
    /// `motorroad=yes`。
    pub const MOTORROAD: u8 = 0x01;
    /// 歩道。
    pub const SIDEWALK: u8 = 0x02;
    /// 横断歩道。
    pub const CROSSING: u8 = 0x04;
    /// トンネル。
    pub const TUNNEL: u8 = 0x08;
    /// 橋。
    pub const BRIDGE: u8 = 0x10;
    /// 環状交差点。
    pub const ROUNDABOUT: u8 = 0x20;
    /// 有料。
    pub const TOLL: u8 = 0x40;
}

/// `NODEFLG` のビット（形式書6.3節）。ビット7は0（予約）。
pub mod node_flags {
    use crate::contract::Mode;

    /// 信号。
    pub const SIGNAL: u8 = 0x01;
    /// 横断歩道のノード。
    pub const CROSSING: u8 = 0x02;
    /// 一時停止。
    pub const STOP: u8 = 0x04;
    /// 踏切。
    pub const LEVEL_CROSSING: u8 = 0x08;
    /// carはこのノードを通過できない。
    pub const BLOCK_CAR: u8 = 0x10;
    /// bikeはこのノードを通過できない。
    pub const BLOCK_BIKE: u8 = 0x20;
    /// walkはこのノードを通過できない。
    pub const BLOCK_WALK: u8 = 0x40;

    /// 手段ごとの「通過できない」ビット。
    pub const fn block_bit(mode: Mode) -> u8 {
        match mode {
            Mode::Car => BLOCK_CAR,
            Mode::Bike => BLOCK_BIKE,
            Mode::Walk => BLOCK_WALK,
        }
    }
}

/// `EDGE` 節のレコード（形式書6.2節、16バイト）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeRecord {
    /// 始点ノード。
    pub tail: u32,
    /// 終点ノード。
    pub head: u32,
    /// 長さ（デシメートル、1以上。形式書1章の規則）。
    pub length_dm: u32,
    /// 道路の種類（`HighwayClass::code`、形式書6.5節）。
    pub class: u8,
    /// 手段×方向の通行可否（`access` のビット）。0はグラフに入れない。
    pub access: u8,
    /// `edge_flags` のビット。
    pub flags: u8,
    /// `maxspeed` タグの値（km/h）。0は不明・解釈不能、255は255以上。
    pub maxspeed_kmh: u8,
}

impl EdgeRecord {
    /// `EDGE` 節の `elem_size`。
    pub const SIZE: usize = 16;
}

/// `SHAPE` 節のレコード（形式書6.1節、4バイト）。直前の点からの差分（1e-7度）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShapePoint {
    pub dlat_e7: i16,
    pub dlon_e7: i16,
}

impl ShapePoint {
    /// `SHAPE` 節の `elem_size`。
    pub const SIZE: usize = 4;
}

/// 道路の種類（形式書6.5節）。値は `EdgeRecord::class` に入る。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum HighwayClass {
    Motorway = 0,
    MotorwayLink = 1,
    Trunk = 2,
    TrunkLink = 3,
    Primary = 4,
    PrimaryLink = 5,
    Secondary = 6,
    SecondaryLink = 7,
    Tertiary = 8,
    TertiaryLink = 9,
    Unclassified = 10,
    Residential = 11,
    LivingStreet = 12,
    Service = 13,
    Road = 14,
    Track = 15,
    Path = 16,
    Cycleway = 17,
    Footway = 18,
    Pedestrian = 19,
    Steps = 20,
    Bridleway = 21,
}

impl HighwayClass {
    /// 種類の数。`class ≤ 21`（形式書6.6節）。
    pub const COUNT: usize = 22;

    pub const ALL: [HighwayClass; Self::COUNT] = [
        HighwayClass::Motorway,
        HighwayClass::MotorwayLink,
        HighwayClass::Trunk,
        HighwayClass::TrunkLink,
        HighwayClass::Primary,
        HighwayClass::PrimaryLink,
        HighwayClass::Secondary,
        HighwayClass::SecondaryLink,
        HighwayClass::Tertiary,
        HighwayClass::TertiaryLink,
        HighwayClass::Unclassified,
        HighwayClass::Residential,
        HighwayClass::LivingStreet,
        HighwayClass::Service,
        HighwayClass::Road,
        HighwayClass::Track,
        HighwayClass::Path,
        HighwayClass::Cycleway,
        HighwayClass::Footway,
        HighwayClass::Pedestrian,
        HighwayClass::Steps,
        HighwayClass::Bridleway,
    ];

    pub const fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.get(usize::from(code)).copied()
    }

    /// OSMの `highway` タグの値。
    pub const fn highway_tag(self) -> &'static str {
        match self {
            HighwayClass::Motorway => "motorway",
            HighwayClass::MotorwayLink => "motorway_link",
            HighwayClass::Trunk => "trunk",
            HighwayClass::TrunkLink => "trunk_link",
            HighwayClass::Primary => "primary",
            HighwayClass::PrimaryLink => "primary_link",
            HighwayClass::Secondary => "secondary",
            HighwayClass::SecondaryLink => "secondary_link",
            HighwayClass::Tertiary => "tertiary",
            HighwayClass::TertiaryLink => "tertiary_link",
            HighwayClass::Unclassified => "unclassified",
            HighwayClass::Residential => "residential",
            HighwayClass::LivingStreet => "living_street",
            HighwayClass::Service => "service",
            HighwayClass::Road => "road",
            HighwayClass::Track => "track",
            HighwayClass::Path => "path",
            HighwayClass::Cycleway => "cycleway",
            HighwayClass::Footway => "footway",
            HighwayClass::Pedestrian => "pedestrian",
            HighwayClass::Steps => "steps",
            HighwayClass::Bridleway => "bridleway",
        }
    }

    /// `highway` タグの値から。グラフに入れない種類（`construction` など）は `None`。
    pub fn from_highway_tag(tag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.highway_tag() == tag)
    }
}

/// `Graph::from_parts` の検査の失敗（形式書6.6節）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphError {
    /// 節の件数の矛盾（`NODELL` と `NODEFLG` と `NODEOFS` など）。
    Counts(String),
    /// `NODEOFS` が非減少でない、末尾が2Eでない。
    NodeOffsets(String),
    /// `ADJ` の辺番号・出発ノード・昇順の違反。
    Adjacency(String),
    /// `EDGE` の `tail`・`head`・`length_dm`・`access`・`class` の違反。
    Edge(String),
    /// `SHPOFS`・`SHAPE` の違反。
    Shape(String),
    /// `NODELL` が日本矩形の外。
    Coordinates(String),
    /// `NODEFLG` の予約ビット（ビット7）が立っている。
    NodeFlags(String),
    /// `TIMEMODL` の値が不正（非有限、負、角度の閾値の逆転など。形式書6.6節「8章の形で読める」）。
    TimeModel(String),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, message) = match self {
            GraphError::Counts(m) => ("件数", m),
            GraphError::NodeOffsets(m) => ("NODEOFS", m),
            GraphError::Adjacency(m) => ("ADJ", m),
            GraphError::Edge(m) => ("EDGE", m),
            GraphError::Shape(m) => ("SHAPE", m),
            GraphError::Coordinates(m) => ("NODELL", m),
            GraphError::NodeFlags(m) => ("NODEFLG", m),
            GraphError::TimeModel(m) => ("TIMEMODL", m),
        };
        write!(f, "graph.binが不正です（{kind}）: {message}")
    }
}

impl std::error::Error for GraphError {}

/// `graph.bin` の節（形式書6.1節）をそのまま持つ組。`Graph::from_parts` に渡す。
#[derive(Debug, Clone, PartialEq)]
pub struct GraphParts {
    /// `PARAMS`（JSON）。
    pub params: Value,
    /// `TIMEMODL`（8章）。
    pub time_model: TimeModel,
    /// `NODEOFS`：長さN+1。
    pub node_ofs: Vec<u32>,
    /// `ADJ`：長さ2E。
    pub adj: Vec<DirEdge>,
    /// `NODELL`：長さN。
    pub node_ll: Vec<Coord>,
    /// `NODEFLG`：長さN。
    pub node_flags: Vec<u8>,
    /// `EDGE`：長さE。
    pub edges: Vec<EdgeRecord>,
    /// `SHPOFS`：長さE+1。
    pub shape_ofs: Vec<u32>,
    /// `SHAPE`：形状点数。
    pub shape: Vec<ShapePoint>,
}

/// 共通グラフ（常駐、読み取り専用）。
#[derive(Debug, Clone)]
pub struct Graph {
    parts: GraphParts,
}

/// 節の件数の整合（形式書6.1節）。以降の検査が添字で読めるようにする。
fn check_counts(parts: &GraphParts) -> Result<(), GraphError> {
    let n = parts.node_ll.len();
    let e = parts.edges.len();
    if n >= NONE as usize {
        return Err(GraphError::Counts(format!(
            "ノード数{n}が上限（u32::MAXを「なし」に使うため未満）を超えています"
        )));
    }
    if e >= 1 << 31 {
        return Err(GraphError::Counts(format!(
            "辺数{e}が2^31以上です（有向辺番号がu32に収まらない）"
        )));
    }
    let expected = [
        ("NODEFLG", parts.node_flags.len(), n),
        ("NODEOFS", parts.node_ofs.len(), n + 1),
        ("ADJ", parts.adj.len(), 2 * e),
        ("SHPOFS", parts.shape_ofs.len(), e + 1),
    ];
    for (name, actual, want) in expected {
        if actual != want {
            return Err(GraphError::Counts(format!(
                "{name}の件数が{actual}です（{want}のはず。N={n}、E={e}）"
            )));
        }
    }
    Ok(())
}

fn check_node_offsets(parts: &GraphParts) -> Result<(), GraphError> {
    let ofs = &parts.node_ofs;
    if ofs[0] != 0 {
        return Err(GraphError::NodeOffsets(format!(
            "NODEOFS[0]が{}です（0のはず）",
            ofs[0]
        )));
    }
    if let Some(i) = ofs.windows(2).position(|w| w[0] > w[1]) {
        return Err(GraphError::NodeOffsets(format!(
            "NODEOFS[{i}]={}がNODEOFS[{}]={}より大きい（非減少でない）",
            ofs[i],
            i + 1,
            ofs[i + 1]
        )));
    }
    let last = ofs[ofs.len() - 1] as usize;
    if last != parts.adj.len() {
        return Err(GraphError::NodeOffsets(format!(
            "NODEOFSの末尾が{last}です（2E={}のはず）",
            parts.adj.len()
        )));
    }
    Ok(())
}

fn check_coordinates(parts: &GraphParts) -> Result<(), GraphError> {
    match parts.node_ll.iter().position(|c| !c.in_japan()) {
        Some(v) => {
            let c = parts.node_ll[v];
            Err(GraphError::Coordinates(format!(
                "ノード{v}の座標（lat_e7={}、lon_e7={}）が日本矩形の外です",
                c.lat_e7, c.lon_e7
            )))
        }
        None => Ok(()),
    }
}

fn check_node_flags(parts: &GraphParts) -> Result<(), GraphError> {
    match parts.node_flags.iter().position(|f| f & 0x80 != 0) {
        Some(v) => Err(GraphError::NodeFlags(format!(
            "ノード{v}のフラグ{:#04x}で予約ビット（ビット7）が立っています",
            parts.node_flags[v]
        ))),
        None => Ok(()),
    }
}

fn check_edges(parts: &GraphParts) -> Result<(), GraphError> {
    let n = parts.node_ll.len();
    for (i, e) in parts.edges.iter().enumerate() {
        if e.tail as usize >= n || e.head as usize >= n {
            return Err(GraphError::Edge(format!(
                "辺{i}の端点（tail={}、head={}）がN={n}以上です",
                e.tail, e.head
            )));
        }
        if e.length_dm < 1 {
            return Err(GraphError::Edge(format!("辺{i}のlength_dmが0です")));
        }
        if e.access == 0 {
            return Err(GraphError::Edge(format!(
                "辺{i}のaccessが0です（通れない辺はグラフに入れない）"
            )));
        }
        if e.access & !access::ALL != 0 {
            return Err(GraphError::Edge(format!(
                "辺{i}のaccess {:#04x}で予約ビット（ビット6〜7）が立っています",
                e.access
            )));
        }
        if HighwayClass::from_code(e.class).is_none() {
            return Err(GraphError::Edge(format!(
                "辺{i}のclass {}が21を超えています",
                e.class
            )));
        }
        if e.flags & 0x80 != 0 {
            return Err(GraphError::Edge(format!(
                "辺{i}のflags {:#04x}で予約ビット（ビット7）が立っています",
                e.flags
            )));
        }
    }
    Ok(())
}

/// `ADJ`：ノードvの範囲の要素が、vを出発点とする有向辺で、狭義昇順であること。
/// 範囲の長さの合計が2Eで、各有向辺の出発ノードはただ1つなので、2E個の有向辺がちょうど1回ずつ現れる。
/// 呼び出す前に `check_node_offsets`・`check_edges` を通っていること。
fn check_adjacency(parts: &GraphParts) -> Result<(), GraphError> {
    let e_count = parts.edges.len();
    for v in 0..parts.node_ll.len() {
        let range = parts.node_ofs[v] as usize..parts.node_ofs[v + 1] as usize;
        let mut prev: Option<DirEdge> = None;
        for pos in range {
            let d = parts.adj[pos];
            if d.edge() as usize >= e_count {
                return Err(GraphError::Adjacency(format!(
                    "ADJ[{pos}]の辺番号{}がE={e_count}以上です",
                    d.edge()
                )));
            }
            let record = &parts.edges[d.edge() as usize];
            let from = if d.is_reverse() {
                record.head
            } else {
                record.tail
            };
            if from as usize != v {
                return Err(GraphError::Adjacency(format!(
                    "ADJ[{pos}]（有向辺{}）の出発ノード{from}が、属する範囲のノード{v}と違います",
                    d.0
                )));
            }
            if prev.is_some_and(|p| p >= d) {
                return Err(GraphError::Adjacency(format!(
                    "ノード{v}のADJが狭義昇順ではありません（ADJ[{pos}]={}）",
                    d.0
                )));
            }
            prev = Some(d);
        }
    }
    Ok(())
}

/// `SHPOFS` が0から始まり非減少で末尾が形状点数であること。形状点の差分を足していった座標が
/// `i32` に収まること（`edge_polyline` の加算が溢れない）。
fn check_shape(parts: &GraphParts) -> Result<(), GraphError> {
    let ofs = &parts.shape_ofs;
    if ofs[0] != 0 {
        return Err(GraphError::Shape(format!(
            "SHPOFS[0]が{}です（0のはず）",
            ofs[0]
        )));
    }
    if let Some(i) = ofs.windows(2).position(|w| w[0] > w[1]) {
        return Err(GraphError::Shape(format!(
            "SHPOFS[{i}]={}がSHPOFS[{}]={}より大きい（非減少でない）",
            ofs[i],
            i + 1,
            ofs[i + 1]
        )));
    }
    let last = ofs[ofs.len() - 1] as usize;
    if last != parts.shape.len() {
        return Err(GraphError::Shape(format!(
            "SHPOFSの末尾が{last}です（形状点数{}のはず）",
            parts.shape.len()
        )));
    }
    for (e, record) in parts.edges.iter().enumerate() {
        let points = &parts.shape[ofs[e] as usize..ofs[e + 1] as usize];
        let start = parts.node_ll[record.tail as usize];
        let mut lat = Some(start.lat_e7);
        let mut lon = Some(start.lon_e7);
        for p in points {
            lat = lat.and_then(|x| x.checked_add(i32::from(p.dlat_e7)));
            lon = lon.and_then(|x| x.checked_add(i32::from(p.dlon_e7)));
        }
        if lat.is_none() || lon.is_none() {
            return Err(GraphError::Shape(format!(
                "辺{e}の形状点の差分を足すと座標がi32を超えます"
            )));
        }
    }
    Ok(())
}

impl Graph {
    /// 節から作る。形式書6.6節の検査（`NODEOFS` が非減少で末尾が2E、`ADJ` の各要素の辺番号がE未満で
    /// その出発ノードが範囲のノードと一致、`EDGE` の `tail`・`head` がN未満、`length_dm ≥ 1`、
    /// `access ≠ 0`、`class ≤ 21`、`SHPOFS` が非減少で末尾が形状点数、`NODELL` が日本矩形の内側）を行う。
    /// 小さな合成グラフでもそのまま使える（件数に下限を設けない）。
    pub fn from_parts(parts: GraphParts) -> Result<Graph, GraphError> {
        check_counts(&parts)?;
        check_node_offsets(&parts)?;
        check_coordinates(&parts)?;
        check_node_flags(&parts)?;
        check_edges(&parts)?;
        check_adjacency(&parts)?;
        check_shape(&parts)?;
        parts.time_model.validate().map_err(GraphError::TimeModel)?;
        Ok(Graph { parts })
    }

    /// 節に戻す（`routing_files::encode_graph` が使う）。
    pub fn parts(&self) -> &GraphParts {
        &self.parts
    }

    pub fn into_parts(self) -> GraphParts {
        self.parts
    }

    /// ノード数N。
    pub fn node_count(&self) -> usize {
        self.parts.node_ll.len()
    }

    /// 辺数E。
    pub fn edge_count(&self) -> usize {
        self.parts.edges.len()
    }

    /// ノードから出る有向辺（`ADJ[NODEOFS[v]..NODEOFS[v+1]]`、通行の可否によらない）。
    pub fn out_dedges(&self, node: NodeId) -> &[DirEdge] {
        let v = node as usize;
        let start = self.parts.node_ofs[v] as usize;
        let end = self.parts.node_ofs[v + 1] as usize;
        &self.parts.adj[start..end]
    }

    pub fn edge(&self, e: EdgeId) -> &EdgeRecord {
        &self.parts.edges[e as usize]
    }

    /// 有向辺の出発ノード（向き0は `tail`、向き1は `head`）。
    pub fn dedge_from(&self, d: DirEdge) -> NodeId {
        let edge = self.edge(d.edge());
        if d.is_reverse() { edge.head } else { edge.tail }
    }

    /// 有向辺の到着ノード。
    pub fn dedge_to(&self, d: DirEdge) -> NodeId {
        self.dedge_from(d.reversed())
    }

    /// その手段で有向辺の向きに通れるか（`access` のビット）。
    /// ノードの「通過できない」ビット（`node_blocked`）は含まない。
    pub fn passable(&self, d: DirEdge, mode: Mode) -> bool {
        self.edge(d.edge()).access & access::bit(mode, d.is_reverse()) != 0
    }

    pub fn node_coord(&self, node: NodeId) -> Coord {
        self.parts.node_ll[node as usize]
    }

    /// `NODEFLG`。
    pub fn node_flags(&self, node: NodeId) -> u8 {
        self.parts.node_flags[node as usize]
    }

    /// その手段が経路の途中で通過できないノードか（形式書6.3節）。
    /// 出発・到着の辺の端点としては使える。
    pub fn node_blocked(&self, node: NodeId, mode: Mode) -> bool {
        self.node_flags(node) & node_flags::block_bit(mode) != 0
    }

    /// 辺の折れ線（始点ノード、中間形状点、終点ノードの順。形式書6.1節・6.4節）。
    pub fn edge_polyline(&self, e: EdgeId) -> Vec<Coord> {
        let record = self.edge(e);
        let start = self.parts.shape_ofs[e as usize] as usize;
        let end = self.parts.shape_ofs[e as usize + 1] as usize;
        let mut points = Vec::with_capacity(end - start + 2);
        let mut current = self.node_coord(record.tail);
        points.push(current);
        for p in &self.parts.shape[start..end] {
            current = Coord::new(
                current.lat_e7 + i32::from(p.dlat_e7),
                current.lon_e7 + i32::from(p.dlon_e7),
            );
            points.push(current);
        }
        points.push(self.node_coord(record.head));
        points
    }

    /// 有向辺の進行方向に並べた折れ線。
    pub fn dedge_polyline(&self, d: DirEdge) -> Vec<Coord> {
        let mut points = self.edge_polyline(d.edge());
        if d.is_reverse() {
            points.reverse();
        }
        points
    }

    pub fn time_model(&self) -> &TimeModel {
        &self.parts.time_model
    }

    /// `PARAMS`（JSON）。
    pub fn params(&self) -> &Value {
        &self.parts.params
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::routing::graph::time::TimeModel;

    #[test]
    fn dir_edge_bits() {
        let d = DirEdge::new(7, false);
        assert_eq!((d.0, d.edge(), d.is_reverse()), (14, 7, false));
        let r = d.reversed();
        assert_eq!((r.0, r.edge(), r.is_reverse()), (15, 7, true));
        assert_eq!(r.reversed(), d);
        assert_eq!(DirEdge::new(7, true), r);
    }

    #[test]
    fn highway_class_codes_round_trip() {
        for (i, class) in HighwayClass::ALL.into_iter().enumerate() {
            assert_eq!(usize::from(class.code()), i);
            assert_eq!(HighwayClass::from_code(class.code()), Some(class));
            assert_eq!(
                HighwayClass::from_highway_tag(class.highway_tag()),
                Some(class)
            );
        }
        assert_eq!(HighwayClass::from_code(22), None);
        assert_eq!(HighwayClass::from_highway_tag("construction"), None);
        assert_eq!(HighwayClass::Residential.code(), 11);
        assert_eq!(HighwayClass::Bridleway.code(), 21);
    }

    #[test]
    fn access_bits_match_the_table() {
        assert_eq!(access::bit(Mode::Car, false), 0x01);
        assert_eq!(access::bit(Mode::Car, true), 0x02);
        assert_eq!(access::bit(Mode::Bike, false), 0x04);
        assert_eq!(access::bit(Mode::Bike, true), 0x08);
        assert_eq!(access::bit(Mode::Walk, false), 0x10);
        assert_eq!(access::bit(Mode::Walk, true), 0x20);
        assert_eq!(access::both(Mode::Walk), 0x30);
        assert_eq!(node_flags::block_bit(Mode::Walk), 0x40);
    }

    #[test]
    fn coord_conversion() {
        let c = Coord::new(356_000_000, 1_396_000_000);
        let p = c.to_latlon();
        assert_eq!((p.lat, p.lon), (35.6, 139.6));
        assert_eq!(Coord::from_latlon(p), c);
    }

    /// 0 -- 1 の1辺（形状点1つ）だけの最小のグラフ。`from_parts` を通さず直接組む。
    fn tiny() -> Graph {
        Graph {
            parts: GraphParts {
                params: Value::Null,
                time_model: TimeModel::synthetic(),
                node_ofs: vec![0, 1, 2],
                adj: vec![DirEdge::new(0, false), DirEdge::new(0, true)],
                node_ll: vec![
                    Coord::new(350_000_000, 1_390_000_000),
                    Coord::new(350_000_100, 1_390_000_300),
                ],
                node_flags: vec![0, node_flags::BLOCK_CAR],
                edges: vec![EdgeRecord {
                    tail: 0,
                    head: 1,
                    length_dm: 40,
                    class: HighwayClass::Residential.code(),
                    access: access::CAR_FWD | access::WALK_FWD | access::WALK_BWD,
                    flags: 0,
                    maxspeed_kmh: 0,
                }],
                shape_ofs: vec![0, 1],
                shape: vec![ShapePoint {
                    dlat_e7: 50,
                    dlon_e7: 100,
                }],
            },
        }
    }

    #[test]
    fn accessors_on_a_tiny_graph() {
        let g = tiny();
        let (fwd, bwd) = (DirEdge::new(0, false), DirEdge::new(0, true));
        assert_eq!((g.node_count(), g.edge_count()), (2, 1));
        assert_eq!(g.out_dedges(0), &[fwd]);
        assert_eq!(g.out_dedges(1), &[bwd]);
        assert_eq!((g.dedge_from(fwd), g.dedge_to(fwd)), (0, 1));
        assert_eq!((g.dedge_from(bwd), g.dedge_to(bwd)), (1, 0));
        assert!(g.passable(fwd, Mode::Car) && !g.passable(bwd, Mode::Car));
        assert!(g.passable(bwd, Mode::Walk) && !g.passable(fwd, Mode::Bike));
        assert!(g.node_blocked(1, Mode::Car) && !g.node_blocked(1, Mode::Walk));
        let line = g.edge_polyline(0);
        assert_eq!(line.len(), 3);
        assert_eq!(line[1], Coord::new(350_000_050, 1_390_000_100));
        assert_eq!(
            g.dedge_polyline(bwd),
            line.into_iter().rev().collect::<Vec<_>>()
        );
    }

    // ---- from_parts（形式書6.6節） ----

    use crate::domain::routing::graph::synthetic::{GraphBuilder, grid};

    /// 2×2の格子：ノード0〜3、辺e0=0-1、e1=0-2、e2=1-3、e3=2-3。各ノードの `ADJ` は2要素。
    fn base_parts() -> GraphParts {
        grid(2, 2, 100.0).0.parts()
    }

    fn rejected(mutate: impl FnOnce(&mut GraphParts)) -> GraphError {
        let mut parts = base_parts();
        mutate(&mut parts);
        Graph::from_parts(parts).expect_err("検査で拒否されるはず")
    }

    #[test]
    fn from_parts_accepts_valid_graphs() {
        let parts = base_parts();
        let graph = Graph::from_parts(parts.clone()).unwrap();
        assert_eq!(graph.parts(), &parts);
        assert_eq!(graph.into_parts(), parts);

        // 空のグラフ、辺のないグラフ
        assert!(Graph::from_parts(GraphBuilder::new().parts()).is_ok());
        let mut one = GraphBuilder::new();
        one.add_node_deg(35.0, 139.0);
        assert!(Graph::from_parts(one.parts()).is_ok());

        // `tiny()` と同じ内容
        assert!(Graph::from_parts(tiny().into_parts()).is_ok());
    }

    #[test]
    fn from_parts_accepts_the_rectangle_corners_and_loops() {
        let mut b = GraphBuilder::new();
        let sw = b.add_node(Coord::new(LAT_E7_MIN, LON_E7_MIN));
        let ne = b.add_node(Coord::new(LAT_E7_MAX, LON_E7_MAX));
        b.add_road(sw, ne);
        b.add_road(ne, ne);
        let graph = b.try_build().unwrap();
        // ループの2つの有向辺が同じノードに入る。
        assert_eq!(graph.out_dedges(ne).len(), 3);
    }

    #[test]
    fn from_parts_rejects_count_mismatches() {
        for mutate in [
            (|p: &mut GraphParts| {
                p.node_flags.pop();
            }) as fn(&mut GraphParts),
            |p| p.node_ofs.push(8),
            |p| {
                p.adj.pop();
            },
            |p| {
                p.shape_ofs.pop();
            },
            |p| {
                p.node_ll.pop();
            },
            |p| p.node_ofs.clear(),
        ] {
            assert!(matches!(rejected(mutate), GraphError::Counts(_)));
        }
    }

    #[test]
    fn from_parts_rejects_bad_node_offsets() {
        assert!(matches!(
            rejected(|p| p.node_ofs[0] = 1),
            GraphError::NodeOffsets(_)
        ));
        // 非減少でない
        assert!(matches!(
            rejected(|p| p.node_ofs[2] = 1),
            GraphError::NodeOffsets(_)
        ));
        // 末尾が2Eでない
        assert!(matches!(
            rejected(|p| p.node_ofs[4] = 7),
            GraphError::NodeOffsets(_)
        ));
        assert!(matches!(
            rejected(|p| p.node_ofs[4] = 9),
            GraphError::NodeOffsets(_)
        ));
    }

    #[test]
    fn from_parts_rejects_bad_adjacency() {
        // 辺番号がE以上
        assert!(matches!(
            rejected(|p| p.adj[0] = DirEdge::new(4, false)),
            GraphError::Adjacency(_)
        ));
        // 出発ノードが範囲のノードと違う（別のノードの有向辺と入れ替える）
        assert!(matches!(
            rejected(|p| p.adj.swap(0, 2)),
            GraphError::Adjacency(_)
        ));
        // 昇順でない
        assert!(matches!(
            rejected(|p| p.adj.swap(0, 1)),
            GraphError::Adjacency(_)
        ));
        // 重複
        assert!(matches!(
            rejected(|p| p.adj[1] = p.adj[0]),
            GraphError::Adjacency(_)
        ));
        // 逆向きの有向辺を出発ノードの範囲に入れ違える
        assert!(matches!(
            rejected(|p| p.adj[0] = DirEdge::new(0, true)),
            GraphError::Adjacency(_)
        ));
    }

    #[test]
    fn from_parts_rejects_bad_edges() {
        let edge_mutations: [fn(&mut EdgeRecord); 7] = [
            |e| e.tail = 4,
            |e| e.head = 99,
            |e| e.length_dm = 0,
            |e| e.access = 0,
            |e| e.access |= 0x40,
            |e| e.class = 22,
            |e| e.flags |= 0x80,
        ];
        for mutate in edge_mutations {
            let error = rejected(|p| mutate(&mut p.edges[1]));
            assert!(matches!(error, GraphError::Edge(_)), "{error}");
        }
        // 境界の値は受け付ける。
        let mut parts = base_parts();
        parts.edges[0].class = 21;
        parts.edges[0].length_dm = 1;
        parts.edges[0].access = access::ALL;
        parts.edges[0].flags = 0x7f;
        parts.edges[0].maxspeed_kmh = 255;
        assert!(Graph::from_parts(parts).is_ok());
    }

    #[test]
    fn from_parts_rejects_bad_shapes() {
        let point = ShapePoint {
            dlat_e7: 1,
            dlon_e7: 1,
        };
        // SHPOFS[0] が0でない
        assert!(matches!(
            rejected(|p| {
                p.shape = vec![point];
                p.shape_ofs = vec![1, 1, 1, 1, 1];
            }),
            GraphError::Shape(_)
        ));
        // 非減少でない
        assert!(matches!(
            rejected(|p| {
                p.shape = vec![point, point];
                p.shape_ofs = vec![0, 2, 1, 2, 2];
            }),
            GraphError::Shape(_)
        ));
        // 末尾が形状点数と違う
        assert!(matches!(
            rejected(|p| {
                p.shape = vec![point, point];
                p.shape_ofs = vec![0, 1, 1, 1, 1];
            }),
            GraphError::Shape(_)
        ));
        assert!(matches!(
            rejected(|p| p.shape_ofs = vec![0, 0, 0, 0, 1]),
            GraphError::Shape(_)
        ));
        // 差分を足した座標がi32を超える
        let far = ShapePoint {
            dlat_e7: 0,
            dlon_e7: i16::MAX,
        };
        assert!(matches!(
            rejected(|p| {
                p.shape = vec![far; 30_000];
                p.shape_ofs = vec![0, 30_000, 30_000, 30_000, 30_000];
            }),
            GraphError::Shape(_)
        ));
        // 形状点があっても整合していれば受け付ける。
        let mut parts = base_parts();
        parts.shape = vec![point; 3];
        parts.shape_ofs = vec![0, 1, 1, 3, 3];
        assert!(Graph::from_parts(parts).is_ok());
    }

    #[test]
    fn from_parts_rejects_nodes_outside_japan() {
        for bad in [
            Coord::new(LAT_E7_MIN - 1, 1_390_000_000),
            Coord::new(LAT_E7_MAX + 1, 1_390_000_000),
            Coord::new(350_000_000, LON_E7_MIN - 1),
            Coord::new(350_000_000, LON_E7_MAX + 1),
            Coord::new(0, 0),
        ] {
            let error = rejected(|p| p.node_ll[2] = bad);
            assert!(matches!(error, GraphError::Coordinates(_)), "{error}");
        }
    }

    #[test]
    fn from_parts_rejects_reserved_node_flag_bit_and_bad_time_models() {
        assert!(matches!(
            rejected(|p| p.node_flags[0] = 0x80),
            GraphError::NodeFlags(_)
        ));
        assert!(
            Graph::from_parts({
                let mut p = base_parts();
                p.node_flags = vec![0x7f; 4];
                p
            })
            .is_ok()
        );
        assert!(matches!(
            rejected(|p| p.time_model.speed_kmh.car[3] = -1.0),
            GraphError::TimeModel(_)
        ));
        assert!(matches!(
            rejected(|p| p.time_model.turn.bike.u_turn_min_deg = 10.0),
            GraphError::TimeModel(_)
        ));
    }

    #[test]
    fn from_parts_error_messages_name_the_section() {
        let message = rejected(|p| p.edges[0].length_dm = 0).to_string();
        assert!(
            message.contains("graph.bin") && message.contains("EDGE"),
            "{message}"
        );
    }

    #[test]
    fn coord_in_japan_boundaries() {
        assert!(Coord::new(LAT_E7_MIN, LON_E7_MIN).in_japan());
        assert!(Coord::new(LAT_E7_MAX, LON_E7_MAX).in_japan());
        assert!(!Coord::new(LAT_E7_MIN - 1, LON_E7_MIN).in_japan());
        assert!(!Coord::new(LAT_E7_MAX, LON_E7_MAX + 1).in_japan());
        assert!(Coord::new(356_000_000, 1_397_000_000).in_japan());
    }
}
