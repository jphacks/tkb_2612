//! 8つの `.bin` と `domain::routing` の型の相互変換（形式書3〜15章）。
//!
//! `encode_*` は型から [`BinFileWriter`](super::binfile::BinFileWriter) でバイト列を作る。
//! `decode_*` は [`BinFile`](super::binfile::BinFile) でバイト列を開き、マジックナンバー・
//! `format_version`・`elem_size`・件数・`DEPS` のキーの有無と、各章の「読込時の検査」を行って型にする。
//! `DEPS` のハッシュが実ファイルと一致するか、`routing.files[]` と一致するかは、読込側
//! （`load::load_routing_engine`）が確かめる。
// 骨組み：実装時に外す
#![allow(unused_variables)]

use std::collections::BTreeMap;

use crate::domain::routing::accident::{AccidentAttrTable, Assignment};
use crate::domain::routing::graph::Graph;
use crate::domain::routing::graph::restrictions::Restrictions;
use crate::domain::routing::risk::RiskModel;
use crate::domain::routing::search::alt::Landmarks;
use crate::domain::routing::snap::OdIndex;
use crate::domain::routing::topology::Topology;

use super::LoadError;
use super::release::{
    ACCIDENT_ATTRS_FILE, ASSIGNMENT_FILE, GRAPH_FILE, ODINDEX_FILE, RESTRICTIONS_FILE, RISK_FILE,
    TOPOLOGY_FILE,
};

/// `DEPS` 節：入力ファイル名→SHA-256（小文字hex）。
pub type Deps = BTreeMap<String, String>;

/// 読み込んだ型と、そのファイルの `DEPS`。
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded<T> {
    pub value: T,
    pub deps: Deps,
}

/// そのファイルの `DEPS` に必ず含めるキー（形式書3.3節の表）。
/// `alt.bin` は `graph.bin`、`graph.bin` はなし。8ファイル以外の名前は空。
pub fn required_deps(file: &str) -> &'static [&'static str] {
    match file {
        GRAPH_FILE => &[],
        RESTRICTIONS_FILE | TOPOLOGY_FILE | ODINDEX_FILE | super::release::ALT_FILE => {
            &[GRAPH_FILE]
        }
        ACCIDENT_ATTRS_FILE => &[super::release::ACCIDENTS_FILE],
        ASSIGNMENT_FILE => &[
            GRAPH_FILE,
            TOPOLOGY_FILE,
            ACCIDENT_ATTRS_FILE,
            super::release::ACCIDENTS_FILE,
        ],
        RISK_FILE => &[
            GRAPH_FILE,
            TOPOLOGY_FILE,
            ACCIDENT_ATTRS_FILE,
            ASSIGNMENT_FILE,
            super::release::ACCIDENTS_FILE,
        ],
        _ => &[],
    }
}

/// `graph.bin`（形式書6章）。`DEPS` は `{}`。
pub fn encode_graph(graph: &Graph) -> Vec<u8> {
    todo!("形式書6章")
}

pub fn decode_graph(bytes: &[u8]) -> Result<Decoded<Graph>, LoadError> {
    todo!("形式書6章")
}

/// `restrictions.bin`（形式書9章）。
pub fn encode_restrictions(restrictions: &Restrictions, deps: &Deps) -> Vec<u8> {
    todo!("形式書9章")
}

pub fn decode_restrictions(bytes: &[u8]) -> Result<Decoded<Restrictions>, LoadError> {
    todo!("形式書9章")
}

/// `topology.bin`（形式書10章）。
pub fn encode_topology(topology: &Topology, deps: &Deps) -> Vec<u8> {
    todo!("形式書10章")
}

pub fn decode_topology(bytes: &[u8]) -> Result<Decoded<Topology>, LoadError> {
    todo!("形式書10章")
}

/// `odindex.bin`（形式書11章）。
pub fn encode_odindex(index: &OdIndex, deps: &Deps) -> Vec<u8> {
    todo!("形式書11章")
}

pub fn decode_odindex(bytes: &[u8]) -> Result<Decoded<OdIndex>, LoadError> {
    todo!("形式書11章")
}

/// `accident_attrs.bin`（形式書12章）。起動時はハッシュの照合だけで、常駐させない。
pub fn encode_accident_attrs(table: &AccidentAttrTable, deps: &Deps) -> Vec<u8> {
    todo!("形式書12章")
}

pub fn decode_accident_attrs(bytes: &[u8]) -> Result<Decoded<AccidentAttrTable>, LoadError> {
    todo!("形式書12章")
}

/// `assignment.bin`（形式書13章）。
pub fn encode_assignment(assignment: &Assignment, deps: &Deps) -> Vec<u8> {
    todo!("形式書13章")
}

pub fn decode_assignment(bytes: &[u8]) -> Result<Decoded<Assignment>, LoadError> {
    todo!("形式書13章")
}

/// `risk.bin`（形式書14章）。
pub fn encode_risk(model: &RiskModel, deps: &Deps) -> Vec<u8> {
    todo!("形式書14章")
}

pub fn decode_risk(bytes: &[u8]) -> Result<Decoded<RiskModel>, LoadError> {
    todo!("形式書14章")
}

/// `alt.bin`（形式書15章）。
pub fn encode_alt(landmarks: &Landmarks, deps: &Deps) -> Vec<u8> {
    todo!("形式書15章")
}

pub fn decode_alt(bytes: &[u8]) -> Result<Decoded<Landmarks>, LoadError> {
    todo!("形式書15章")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::release::{ACCIDENTS_FILE, ROUTING_FILES};

    #[test]
    fn required_deps_follow_the_table() {
        assert!(required_deps("graph.bin").is_empty());
        for file in ["restrictions.bin", "topology.bin", "odindex.bin", "alt.bin"] {
            assert_eq!(required_deps(file), ["graph.bin"], "{file}");
        }
        assert_eq!(required_deps("accident_attrs.bin"), [ACCIDENTS_FILE]);
        assert_eq!(required_deps("assignment.bin").len(), 4);
        assert_eq!(required_deps("risk.bin").len(), 5);
        // すべての依存先は、8ファイルかaccidents.csv.gzのどれか
        for file in ROUTING_FILES {
            for dep in required_deps(file) {
                assert!(*dep == ACCIDENTS_FILE || ROUTING_FILES.contains(dep));
            }
        }
    }
}
