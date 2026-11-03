//! `GraphParts` の読み取り用の薄い見方（前処理用）。
//!
//! `Graph` は読込時の検査を終えたものだけを持つ型で、前処理は検査の前の節（`GraphParts`）を
//! そのまま扱う。ここは `Graph` の読み取りの関数と同じ意味を、`GraphParts` に対して提供する。

use crate::contract::Mode;
use crate::domain::routing::graph::{
    Coord, DirEdge, EdgeId, EdgeRecord, GraphParts, NodeId, access,
};

#[derive(Debug, Clone, Copy)]
pub struct GraphView<'a> {
    pub parts: &'a GraphParts,
}

impl<'a> GraphView<'a> {
    pub fn new(parts: &'a GraphParts) -> Self {
        Self { parts }
    }

    pub fn node_count(&self) -> usize {
        self.parts.node_ll.len()
    }

    pub fn edge_count(&self) -> usize {
        self.parts.edges.len()
    }

    pub fn edge(&self, e: EdgeId) -> &'a EdgeRecord {
        &self.parts.edges[e as usize]
    }

    pub fn node_coord(&self, v: NodeId) -> Coord {
        self.parts.node_ll[v as usize]
    }

    pub fn node_flags(&self, v: NodeId) -> u8 {
        self.parts.node_flags[v as usize]
    }

    /// ノードから出る有向辺（通行の可否によらない）。
    pub fn out_dedges(&self, v: NodeId) -> &'a [DirEdge] {
        let start = self.parts.node_ofs[v as usize] as usize;
        let end = self.parts.node_ofs[v as usize + 1] as usize;
        &self.parts.adj[start..end]
    }

    pub fn dedge_from(&self, d: DirEdge) -> NodeId {
        let e = self.edge(d.edge());
        if d.is_reverse() { e.head } else { e.tail }
    }

    pub fn dedge_to(&self, d: DirEdge) -> NodeId {
        let e = self.edge(d.edge());
        if d.is_reverse() { e.tail } else { e.head }
    }

    /// その手段で有向辺の向きに通れるか。
    pub fn passable(&self, d: DirEdge, mode: Mode) -> bool {
        self.edge(d.edge()).access & access::bit(mode, d.is_reverse()) != 0
    }

    /// 有向辺 `d` を、`modes`（walk=1、bike=2、car=4のビット）のうち通れる手段のビットにする。
    pub fn passable_modes(&self, d: DirEdge, modes: u8) -> u8 {
        let mut out = 0;
        for mode in [Mode::Walk, Mode::Bike, Mode::Car] {
            if modes & mode.mask() != 0 && self.passable(d, mode) {
                out |= mode.mask();
            }
        }
        out
    }

    /// その手段で、辺が少なくとも一方向に通れるか。
    pub fn edge_has_mode(&self, e: EdgeId, mode: Mode) -> bool {
        self.edge(e).access & access::both(mode) != 0
    }

    /// 辺の折れ線（始点ノード、中間形状点、終点ノードの順）。
    pub fn polyline(&self, e: EdgeId) -> Vec<Coord> {
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
        let mut points = self.polyline(d.edge());
        if d.is_reverse() {
            points.reverse();
        }
        points
    }
}
