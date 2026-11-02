//! 共通グラフ（形式書5〜6章）。ノード・辺・有向辺、`graph.bin` の節に対応する `Graph`。

pub mod restrictions;
pub mod rules;
pub mod time;

use std::fmt;

use serde_json::Value;

use crate::contract::{LatLon, Mode};

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
}

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

impl Graph {
    /// 節から作る。形式書6.6節の検査（`NODEOFS` が非減少で末尾が2E、`ADJ` の各要素の辺番号がE未満で
    /// その出発ノードが範囲のノードと一致、`EDGE` の `tail`・`head` がN未満、`length_dm ≥ 1`、
    /// `access ≠ 0`、`class ≤ 21`、`SHPOFS` が非減少で末尾が形状点数、`NODELL` が日本矩形の内側）を行う。
    /// 小さな合成グラフでもそのまま使える（件数に下限を設けない）。
    pub fn from_parts(parts: GraphParts) -> Result<Graph, GraphError> {
        let _ = parts;
        todo!("形式書6.6節の検査")
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
}
