//! 割り当ての試験用の小さな世界（合成グラフ・手書きの位相・事故の属性）。

use crate::domain::routing::NONE;
use crate::domain::routing::accident::{AccidentAttr, MISSING_U8, RouteClass};
use crate::domain::routing::graph::synthetic::{EdgeSpec, GraphBuilder};
use crate::domain::routing::graph::{Coord, Graph, HighwayClass, NodeId};
use crate::domain::routing::prep::assign::{AssignParams, RoadIndex, Scratch, accident_coord};
use crate::domain::routing::prep::assign_tables::Tables;
use crate::domain::routing::prep::attrs::{GraphAttrs, StringInterner};
use crate::domain::routing::topology::{ClusterRecord, Topology, TwinPair};
use serde_json::Value;

/// 辺の指定。
#[derive(Clone)]
pub struct Spec {
    pub tail: usize,
    pub head: usize,
    pub class: HighwayClass,
    pub flags: u8,
    pub oneway: bool,
    pub ref_: &'static str,
    pub name: &'static str,
    pub lanes: u8,
    pub width_dm: u16,
    pub layer: i8,
}

impl Spec {
    pub fn new(tail: usize, head: usize, class: HighwayClass) -> Spec {
        Spec {
            tail,
            head,
            class,
            flags: 0,
            oneway: false,
            ref_: "",
            name: "",
            lanes: 0,
            width_dm: 0,
            layer: 0,
        }
    }

    pub fn with_ref(mut self, r: &'static str) -> Spec {
        self.ref_ = r;
        self
    }

    pub fn flags(mut self, flags: u8) -> Spec {
        self.flags = flags;
        self
    }

    pub fn oneway(mut self) -> Spec {
        self.oneway = true;
        self
    }
}

pub struct World {
    pub graph: Graph,
    pub attrs: GraphAttrs,
    pub topology: Topology,
}

impl World {
    pub fn index(&self) -> RoadIndex<'_> {
        RoadIndex::build(
            self.graph.parts(),
            &self.attrs,
            &self.topology,
            AssignParams::default(),
        )
    }
}

/// 北緯35度・東経139度からの差（m）で座標を作る。
pub fn at_m(north_m: f64, east_m: f64) -> Coord {
    let lat = 35.0 + north_m / 111_194.926_6;
    let lon = 139.0 + east_m / (111_194.926_6 * (35.0_f64.to_radians()).cos());
    accident_coord(lat, lon)
}

/// ノード（北・東のm）、辺、クラスタ（ノード番号の組）、双子（辺番号の組）から世界を作る。
pub fn world(
    nodes: &[(f64, f64)],
    edges: &[Spec],
    clusters: &[&[NodeId]],
    twins: &[(u32, u32)],
) -> World {
    let mut builder = GraphBuilder::new();
    for &(n, e) in nodes {
        builder.add_node(at_m(n, e));
    }
    for spec in edges {
        let mut e = EdgeSpec::new(spec.tail as u32, spec.head as u32)
            .class(spec.class)
            .flags(spec.flags);
        if spec.oneway {
            e = e.oneway();
        }
        builder.add_edge(e);
    }
    let graph = builder.build();
    let mut interner = StringInterner::new();
    let ref_idx = edges.iter().map(|s| interner.intern(s.ref_)).collect();
    let name_idx = edges.iter().map(|s| interner.intern(s.name)).collect();
    let attrs = GraphAttrs {
        params: Value::Null,
        way_id: (0..edges.len() as i64).collect(),
        node_id: (0..nodes.len() as i64).collect(),
        ref_idx,
        name_idx,
        strtab: interner.finish(),
        lanes: edges.iter().map(|s| s.lanes).collect(),
        width_dm: edges.iter().map(|s| s.width_dm).collect(),
        layer: edges.iter().map(|s| s.layer).collect(),
    };
    let parts = graph.parts();
    let mut node_cluster = vec![NONE; nodes.len()];
    let mut records = Vec::new();
    let mut cluster_nodes = Vec::new();
    for (i, members) in clusters.iter().enumerate() {
        let mut sorted = members.to_vec();
        sorted.sort_unstable();
        let start = cluster_nodes.len() as u32;
        let (mut lat, mut lon) = (0_i64, 0_i64);
        for &v in &sorted {
            node_cluster[v as usize] = i as u32;
            cluster_nodes.push(v);
            lat += i64::from(parts.node_ll[v as usize].lat_e7);
            lon += i64::from(parts.node_ll[v as usize].lon_e7);
        }
        let n = sorted.len() as i64;
        let top_class = sorted
            .iter()
            .flat_map(|&v| {
                let from = parts.node_ofs[v as usize] as usize;
                let to = parts.node_ofs[v as usize + 1] as usize;
                parts.adj[from..to]
                    .iter()
                    .map(|d| parts.edges[d.edge() as usize].class)
            })
            .min()
            .unwrap_or(0);
        records.push(ClusterRecord {
            lat_e7: (lat / n) as i32,
            lon_e7: (lon / n) as i32,
            node_start: start,
            node_count: sorted.len() as u16,
            top_class,
            branches: 4,
            flags: 0,
            pad: [0; 3],
        });
    }
    let mut twin_records: Vec<TwinPair> = twins
        .iter()
        .flat_map(|&(a, b)| [TwinPair { a, b }, TwinPair { a: b, b: a }])
        .collect();
    twin_records.sort_by_key(|t| (t.a, t.b));
    twin_records.dedup();
    let words = edges.len().div_ceil(64);
    let topology = Topology {
        params: Value::Null,
        snap_car: vec![0; words],
        snap_bike: vec![0; words],
        snap_walk: vec![0; words],
        node_cluster,
        clusters: records,
        cluster_nodes,
        twins: twin_records,
        sidewalk_parents: Vec::new(),
    };
    World {
        graph,
        attrs,
        topology,
    }
}

/// 路線コードの上4桁と道路形状から事故の属性を作る（車道幅員・歩車道区分・地形は欠損）。
pub fn attr(route_upper: u16, road_shape: u8) -> AccidentAttr {
    AccidentAttr {
        route_upper,
        route_lower: 0,
        route_class: RouteClass::from_route_upper(route_upper).map_or(MISSING_U8, RouteClass::code),
        road_shape,
        carriageway_width: MISSING_U8,
        sidewalk_division: MISSING_U8,
        terrain: MISSING_U8,
    }
}

/// 表を使わずに、1件の事故を割り当てる。
pub fn assign(
    world: &World,
    point: Coord,
    attr: &AccidentAttr,
) -> crate::domain::routing::prep::assign::Outcome {
    world.index().assign_one(
        &Tables::neutral(),
        point,
        attr,
        &mut Scratch::default(),
        true,
    )
}
