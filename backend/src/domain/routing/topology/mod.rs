//! 位相情報（形式書10章）：吸着先にしてよい辺、交差点クラスタ、上下線の双子、歩道と親の車道。
use serde_json::Value;

use crate::contract::Mode;
use crate::domain::routing::NONE;
use crate::domain::routing::graph::{DirEdge, EdgeId, Graph, NodeId};

/// 交差点クラスタ番号（0〜C-1）。`NONE` は「属さない」。
pub type ClusterId = u32;

/// `CLUSTER` 節のレコード（形式書10章、20バイト）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusterRecord {
    /// クラスタの中心（属するノードの座標の平均）。
    pub lat_e7: i32,
    pub lon_e7: i32,
    /// `CLSNODE` の開始位置。
    pub node_start: u32,
    pub node_count: u16,
    /// クラスタに接する車道の最上位の `class`（`HighwayClass::code` が最小のもの）。
    pub top_class: u8,
    /// クラスタに外から接する車道の辺の数（255で頭打ち）。
    pub branches: u8,
    /// `cluster_flags` のビット。
    pub flags: u8,
    /// 予約（0）。
    pub pad: [u8; 3],
}

impl ClusterRecord {
    /// `CLUSTER` 節の `elem_size`。
    pub const SIZE: usize = 20;
}

/// `ClusterRecord::flags` のビット（形式書10章）。
pub mod cluster_flags {
    /// 信号あり。
    pub const SIGNAL: u8 = 0x01;
    /// 環状交差点。
    pub const ROUNDABOUT: u8 = 0x02;
}

/// `TWIN` 節のレコード（形式書10章、8バイト）。各組を `(a,b)` と `(b,a)` の両方の向きで入れる。
/// 1つの辺に双子は複数ありうる（同じ `a` に複数の `b`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TwinPair {
    pub a: EdgeId,
    pub b: EdgeId,
}

/// `SWPARENT` 節のレコード（形式書10章、8バイト）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidewalkParent {
    pub sidewalk: EdgeId,
    pub parent: EdgeId,
}

/// `topology.bin` の中身。
#[derive(Debug, Clone, PartialEq)]
pub struct Topology {
    /// `PARAMS`（JSON）。
    pub params: Value,
    /// `SNAPCAR`：辺eはワード `e/64` のビット `e%64`。長さ⌈E/64⌉。
    pub snap_car: Vec<u64>,
    /// `SNAPBIK`。
    pub snap_bike: Vec<u64>,
    /// `SNAPFOT`。
    pub snap_walk: Vec<u64>,
    /// `NODECLS`：ノードが属するクラスタ番号。属さなければ `NONE`。長さN。
    pub node_cluster: Vec<u32>,
    /// `CLUSTER`：長さC。
    pub clusters: Vec<ClusterRecord>,
    /// `CLSNODE`：クラスタに属するノード。クラスタiのノードは `[node_start..node_start+node_count]`（昇順）。
    pub cluster_nodes: Vec<NodeId>,
    /// `TWIN`：`(a, b)` の辞書順の狭義昇順（同じ `a` の中は `b` の昇順）。
    pub twins: Vec<TwinPair>,
    /// `SWPARENT`：`sidewalk` の昇順。
    pub sidewalk_parents: Vec<SidewalkParent>,
}

/// `Topology::validate` の失敗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyError(pub String);

impl std::fmt::Display for TopologyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "topology.binが不正です: {}", self.0)
    }
}

impl std::error::Error for TopologyError {}

impl Topology {
    /// 読込時の検査（形式書10章）：ビット列の長さ、`NODECLS` の値がC未満か `NONE`、
    /// `CLSNODE` とクラスタの対応が `NODECLS` と一致、`TWIN`・`SWPARENT` の辺番号がE未満で並び順どおり。
    ///
    /// 次も確かめる。ビット列のEビット目以降（最後のワードの余り）は0。`CLUSTER` の `pad` は0で、
    /// `flags` は定義済みのビットだけ。クラスタのノードは狭義昇順でN未満。`CLSNODE` に余りがない。
    /// `TWIN` は `(a,b)` が辞書順の狭義昇順（1つの辺に双子は複数ありうる。同じ組の重複はない）で
    /// `a ≠ b`、`(a,b)` があれば `(b,a)` もある。
    /// `SWPARENT` は `sidewalk` が狭義昇順で `sidewalk ≠ parent`。
    pub fn validate(&self, graph: &Graph) -> Result<(), TopologyError> {
        let n = graph.node_count();
        let e = graph.edge_count();
        self.validate_snap_bits(e)?;
        self.validate_clusters(n)?;
        self.validate_twins_and_parents(e)
    }

    fn validate_snap_bits(&self, edge_count: usize) -> Result<(), TopologyError> {
        let words = edge_count.div_ceil(64);
        let tail_bits = edge_count % 64;
        for (name, bits) in [
            ("SNAPCAR", &self.snap_car),
            ("SNAPBIK", &self.snap_bike),
            ("SNAPFOT", &self.snap_walk),
        ] {
            if bits.len() != words {
                return Err(TopologyError(format!(
                    "{name}の長さが{}ワードです（E={edge_count}なら{words}ワードのはず）",
                    bits.len()
                )));
            }
            if tail_bits != 0 && bits.last().is_some_and(|&w| w >> tail_bits != 0) {
                return Err(TopologyError(format!(
                    "{name}の最後のワードで、E={edge_count}以降のビットが立っています"
                )));
            }
        }
        Ok(())
    }

    fn validate_clusters(&self, node_count: usize) -> Result<(), TopologyError> {
        if self.node_cluster.len() != node_count {
            return Err(TopologyError(format!(
                "NODECLSの件数が{}です（N={node_count}のはず）",
                self.node_cluster.len()
            )));
        }
        let cluster_count = self.clusters.len();
        if cluster_count >= NONE as usize {
            return Err(TopologyError(format!(
                "クラスタ数{cluster_count}が上限を超えています"
            )));
        }
        if let Some(v) = self
            .node_cluster
            .iter()
            .position(|&c| c != NONE && c as usize >= cluster_count)
        {
            return Err(TopologyError(format!(
                "NODECLS[{v}]={}がクラスタ数C={cluster_count}以上で、NONEでもありません",
                self.node_cluster[v]
            )));
        }
        let mut listed = 0_usize;
        for (i, record) in self.clusters.iter().enumerate() {
            if record.pad != [0; 3] {
                return Err(TopologyError(format!("クラスタ{i}のpadが0ではありません")));
            }
            if record.flags & !(cluster_flags::SIGNAL | cluster_flags::ROUNDABOUT) != 0 {
                return Err(TopologyError(format!(
                    "クラスタ{i}のflags {:#04x}に未定義のビットがあります",
                    record.flags
                )));
            }
            let start = record.node_start as usize;
            let end = start + usize::from(record.node_count);
            let Some(nodes) = self.cluster_nodes.get(start..end) else {
                return Err(TopologyError(format!(
                    "クラスタ{i}のCLSNODEの範囲{start}..{end}がCLSNODEの件数{}を超えています",
                    self.cluster_nodes.len()
                )));
            };
            if nodes.windows(2).any(|w| w[0] >= w[1]) {
                return Err(TopologyError(format!(
                    "クラスタ{i}のノードが狭義昇順ではありません"
                )));
            }
            for &v in nodes {
                if v as usize >= node_count {
                    return Err(TopologyError(format!(
                        "クラスタ{i}のノード{v}がN={node_count}以上です"
                    )));
                }
                if self.node_cluster[v as usize] != i as u32 {
                    return Err(TopologyError(format!(
                        "クラスタ{i}のノード{v}のNODECLSが{}です（{i}のはず）",
                        self.node_cluster[v as usize]
                    )));
                }
            }
            listed += nodes.len();
        }
        let clustered = self.node_cluster.iter().filter(|&&c| c != NONE).count();
        if listed != clustered || self.cluster_nodes.len() != clustered {
            return Err(TopologyError(format!(
                "CLSNODEとNODECLSが対応しません（NODECLSでクラスタに属するノード{clustered}、\
                 クラスタの範囲の合計{listed}、CLSNODEの件数{}）",
                self.cluster_nodes.len()
            )));
        }
        Ok(())
    }

    fn validate_twins_and_parents(&self, edge_count: usize) -> Result<(), TopologyError> {
        for (i, t) in self.twins.iter().enumerate() {
            if t.a as usize >= edge_count || t.b as usize >= edge_count {
                return Err(TopologyError(format!(
                    "TWIN[{i}]の辺番号（a={}、b={}）がE={edge_count}以上です",
                    t.a, t.b
                )));
            }
            if t.a == t.b {
                return Err(TopologyError(format!(
                    "TWIN[{i}]が同じ辺の組です（{}）",
                    t.a
                )));
            }
        }
        if let Some(i) = self
            .twins
            .windows(2)
            .position(|w| (w[0].a, w[0].b) >= (w[1].a, w[1].b))
        {
            return Err(TopologyError(format!(
                "TWINの(a,b)が辞書順の狭義昇順ではありません（TWIN[{}]）",
                i + 1
            )));
        }
        for (i, t) in self.twins.iter().enumerate() {
            if !self.are_twins(t.b, t.a) {
                return Err(TopologyError(format!(
                    "TWIN[{i}]=({},{})の逆向き({},{})がありません",
                    t.a, t.b, t.b, t.a
                )));
            }
        }
        for (i, s) in self.sidewalk_parents.iter().enumerate() {
            if s.sidewalk as usize >= edge_count || s.parent as usize >= edge_count {
                return Err(TopologyError(format!(
                    "SWPARENT[{i}]の辺番号（sidewalk={}、parent={}）がE={edge_count}以上です",
                    s.sidewalk, s.parent
                )));
            }
            if s.sidewalk == s.parent {
                return Err(TopologyError(format!(
                    "SWPARENT[{i}]の歩道と親が同じ辺です（{}）",
                    s.sidewalk
                )));
            }
        }
        if let Some(i) = self
            .sidewalk_parents
            .windows(2)
            .position(|w| w[0].sidewalk >= w[1].sidewalk)
        {
            return Err(TopologyError(format!(
                "SWPARENTのsidewalkが狭義昇順ではありません（SWPARENT[{}]）",
                i + 1
            )));
        }
        Ok(())
    }

    fn snap_bits(&self, mode: Mode) -> &[u64] {
        match mode {
            Mode::Car => &self.snap_car,
            Mode::Bike => &self.snap_bike,
            Mode::Walk => &self.snap_walk,
        }
    }

    /// 出発地・目的地の吸着先にしてよい辺か（`SNAPCAR`・`SNAPBIK`・`SNAPFOT` のビット）。
    pub fn snappable(&self, edge: EdgeId, mode: Mode) -> bool {
        self.snap_bits(mode)
            .get(edge as usize / 64)
            .is_some_and(|word| (word >> (edge % 64)) & 1 == 1)
    }

    /// クラスタの数C。
    pub fn cluster_count(&self) -> usize {
        self.clusters.len()
    }

    /// ノードが属するクラスタ。
    pub fn cluster_of(&self, node: NodeId) -> Option<ClusterId> {
        self.node_cluster
            .get(node as usize)
            .copied()
            .filter(|&c| c != NONE)
    }

    /// クラスタに属するノード（昇順）。
    pub fn nodes_of_cluster(&self, cluster: ClusterId) -> &[NodeId] {
        let record = &self.clusters[cluster as usize];
        let start = record.node_start as usize;
        &self.cluster_nodes[start..start + usize::from(record.node_count)]
    }

    /// 辺 `edge` を `a` とする `TWIN` のレコード（`b` の昇順）。双子がなければ空。
    pub fn twin_records(&self, edge: EdgeId) -> &[TwinPair] {
        let start = self.twins.partition_point(|t| t.a < edge);
        let len = self.twins[start..].partition_point(|t| t.a == edge);
        &self.twins[start..start + len]
    }

    /// 辺 `edge` の上下線の双子の辺（昇順、複数ありうる）。長い1本の反対側が短い複数の辺に
    /// 分かれているとき、その全部が双子になる。
    pub fn twins_of(
        &self,
        edge: EdgeId,
    ) -> impl DoubleEndedIterator<Item = EdgeId> + ExactSizeIterator + Clone + '_ {
        self.twin_records(edge).iter().map(|t| t.b)
    }

    /// 辺 `edge` に双子があるか。
    pub fn has_twin(&self, edge: EdgeId) -> bool {
        !self.twin_records(edge).is_empty()
    }

    /// `a` と `b` が双子か（二分探索）。
    pub fn are_twins(&self, a: EdgeId, b: EdgeId) -> bool {
        self.twins
            .binary_search_by_key(&(a, b), |t| (t.a, t.b))
            .is_ok()
    }

    /// 歩道の辺の親の車道の辺。
    pub fn sidewalk_parent(&self, edge: EdgeId) -> Option<EdgeId> {
        let i = self.sidewalk_parents.partition_point(|s| s.sidewalk < edge);
        self.sidewalk_parents
            .get(i)
            .filter(|s| s.sidewalk == edge)
            .map(|s| s.parent)
    }

    /// 有向辺 `d` が「外から入る有向辺」（出発ノードがクラスタの外で、到着ノードがクラスタの中）なら、
    /// そのクラスタ。交差点のリスクは、この有向辺を通るときに1回だけ加える（形式書10章）。
    pub fn entering_cluster(&self, d: DirEdge, graph: &Graph) -> Option<ClusterId> {
        let into = self.cluster_of(graph.dedge_to(d))?;
        (self.cluster_of(graph.dedge_from(d)) != Some(into)).then_some(into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Topology {
        Topology {
            params: Value::Null,
            snap_car: vec![0b1010, 1],
            snap_bike: vec![0],
            snap_walk: vec![u64::MAX],
            node_cluster: vec![NONE, 0, 0, NONE],
            clusters: vec![ClusterRecord {
                lat_e7: 0,
                lon_e7: 0,
                node_start: 0,
                node_count: 2,
                top_class: 4,
                branches: 3,
                flags: cluster_flags::SIGNAL,
                pad: [0; 3],
            }],
            cluster_nodes: vec![1, 2],
            twins: vec![
                TwinPair { a: 3, b: 5 },
                TwinPair { a: 3, b: 6 },
                TwinPair { a: 5, b: 3 },
                TwinPair { a: 6, b: 3 },
            ],
            sidewalk_parents: vec![SidewalkParent {
                sidewalk: 9,
                parent: 4,
            }],
        }
    }

    #[test]
    fn bit_and_table_lookups() {
        let t = sample();
        assert!(
            !t.snappable(0, Mode::Car) && t.snappable(1, Mode::Car) && t.snappable(3, Mode::Car)
        );
        assert!(t.snappable(64, Mode::Car) && !t.snappable(65, Mode::Car));
        assert!(!t.snappable(1, Mode::Bike));
        assert!(t.snappable(63, Mode::Walk) && !t.snappable(64, Mode::Walk));
        assert_eq!(
            (t.cluster_of(0), t.cluster_of(1), t.cluster_of(9)),
            (None, Some(0), None)
        );
        assert_eq!(t.nodes_of_cluster(0), &[1, 2]);
        assert_eq!(t.twins_of(3).collect::<Vec<_>>(), vec![5, 6]);
        assert_eq!(t.twins_of(5).collect::<Vec<_>>(), vec![3]);
        assert_eq!(t.twins_of(4).len(), 0);
        assert!(t.has_twin(6) && !t.has_twin(4) && !t.has_twin(7));
        assert!(t.are_twins(3, 6) && t.are_twins(6, 3) && !t.are_twins(5, 6));
        assert_eq!(t.twin_records(3).len(), 2);
        assert_eq!(
            (t.sidewalk_parent(9), t.sidewalk_parent(4)),
            (Some(4), None)
        );
    }
}
