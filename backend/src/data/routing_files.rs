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
///
/// 節は `DEPS`・`PARAMS`・`ATTR` の順。`PARAMS` は `table.params` をそのまま書く
/// （件数 `rows` を含むこと。`decode_accident_attrs` が `ATTR` の件数と照合する）。
pub fn encode_accident_attrs(table: &AccidentAttrTable, deps: &Deps) -> Vec<u8> {
    use super::binfile::{BinFileWriter, DEPS_SECTION, PARAMS_SECTION};
    use super::release::routing_file_spec;
    use crate::domain::routing::accident::AccidentAttr;

    let (magic, format_version) =
        routing_file_spec(ACCIDENT_ATTRS_FILE).expect("accident_attrs.binは8ファイルの1つ");
    let mut writer = BinFileWriter::new(magic, format_version);
    writer.add_json(
        DEPS_SECTION,
        &serde_json::to_value(deps).expect("JSONに書ける値"),
    );
    writer.add_json(PARAMS_SECTION, &table.params);
    let mut records = Vec::with_capacity(table.attrs.len() * AccidentAttr::SIZE);
    for attr in &table.attrs {
        records.extend_from_slice(&attr.to_bytes());
    }
    writer.add_section(
        "ATTR",
        AccidentAttr::SIZE as u32,
        table.attrs.len() as u64,
        records,
    );
    writer.finish()
}

/// `accident_attrs.bin` を読む。マジックナンバー・`format_version`・`DEPS` に `accidents.csv.gz` が
/// あること・`ATTR` の `elem_size`（8）と件数（`PARAMS.rows` と一致）・各レコードの値の範囲と
/// 列どうしの整合（`AccidentAttr::validate`）を検査する。
pub fn decode_accident_attrs(bytes: &[u8]) -> Result<Decoded<AccidentAttrTable>, LoadError> {
    use super::binfile::{BinFile, PARAMS_SECTION};
    use super::release::routing_file_spec;
    use crate::domain::routing::accident::AccidentAttr;

    let invalid = |message: String| LoadError::Invalid(format!("accident_attrs.bin: {message}"));
    let (magic, format_version) =
        routing_file_spec(ACCIDENT_ATTRS_FILE).expect("accident_attrs.binは8ファイルの1つ");
    let file = BinFile::parse(bytes)?;
    file.expect(magic, format_version)?;
    let deps = file.deps()?;
    for key in required_deps(ACCIDENT_ATTRS_FILE) {
        if !deps.contains_key(*key) {
            return Err(invalid(format!("DEPSに{key}がない")));
        }
    }
    let params = file.json(PARAMS_SECTION)?;
    let rows = params
        .get("rows")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| invalid("PARAMSのrowsが整数でない".to_owned()))?;
    let section = file.require("ATTR")?;
    section.expect_elem_size(AccidentAttr::SIZE as u32)?;
    section.expect_count(rows)?;
    let mut attrs = Vec::with_capacity(rows as usize);
    for (id, record) in section.records(AccidentAttr::SIZE as u32)?.enumerate() {
        let attr = AccidentAttr::from_bytes(record.try_into().expect("elem_sizeは8"));
        attr.validate()
            .map_err(|e| invalid(format!("事故{id}: {e}")))?;
        attrs.push(attr);
    }
    Ok(Decoded {
        value: AccidentAttrTable { params, attrs },
        deps,
    })
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
