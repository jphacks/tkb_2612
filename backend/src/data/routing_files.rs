//! 8つの `.bin` と `domain::routing` の型の相互変換（形式書3〜15章）。
//!
//! `encode_*` は型から [`BinFileWriter`](super::binfile::BinFileWriter) でバイト列を作る。
//! `decode_*` は [`BinFile`](super::binfile::BinFile) でバイト列を開き、マジックナンバー・
//! `format_version`・`elem_size`・件数・`DEPS` のキーの有無と、各章の「読込時の検査」を行って型にする。
//! `DEPS` のハッシュが実ファイルと一致するか、`routing.files[]` と一致するかは、読込側
//! （`load::load_routing_engine`）が確かめる。
//!
//! 実装済みなのは `graph.bin`・`restrictions.bin`・`topology.bin`・`odindex.bin`（M1.1〜M1.4）と、
//! 中間生成物 `graph_attrs.bin`（形式書16章）。残りは骨組みのまま（別の作業役が埋める）。
// 骨組みの関数（accident_attrs・assignment・risk・alt）が残っている間は外せない
#![allow(unused_variables)]

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};

use crate::domain::routing::accident::{AccidentAttrTable, Assignment};
use crate::domain::routing::graph::restrictions::{Restriction, Restrictions};
use crate::domain::routing::graph::time::TimeModel;
use crate::domain::routing::graph::{Coord, DirEdge, EdgeRecord, Graph, GraphParts, ShapePoint};
use crate::domain::routing::prep::attrs::GraphAttrs;
use crate::domain::routing::risk::{CoefRecord, RiskModel, RiskParams, TABLE_LEN};
use crate::domain::routing::search::alt::Landmarks;
use crate::domain::routing::snap::{OdIndex, OdIndexParams};
use crate::domain::routing::topology::{ClusterRecord, SidewalkParent, Topology, TwinPair};

use super::LoadError;
use super::binfile::{BinFile, BinFileReader, BinFileWriter, DEPS_SECTION, PARAMS_SECTION};
use super::release::{
    ACCIDENT_ATTRS_FILE, ALT_FILE, ALT_MAGIC, ASSIGNMENT_FILE, ASSIGNMENT_MAGIC, GRAPH_ATTRS_MAGIC,
    GRAPH_FILE, GRAPH_MAGIC, ODINDEX_FILE, ODINDEX_MAGIC, RESTRICTIONS_FILE, RESTRICTIONS_MAGIC,
    RISK_FILE, RISK_MAGIC, TOPOLOGY_FILE, TOPOLOGY_MAGIC, routing_file_spec,
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

fn invalid(message: impl Into<String>) -> LoadError {
    LoadError::Invalid(message.into())
}

/// `Deps` をJSONオブジェクトにする。
fn deps_json(deps: &Deps) -> Value {
    Value::Object(
        deps.iter()
            .map(|(file, hash)| (file.clone(), Value::String(hash.clone())))
            .collect::<Map<String, Value>>(),
    )
}

/// `PARAMS` に書くJSON。オブジェクトでなければ空のオブジェクトにする。
fn params_json(params: &Value) -> Value {
    match params {
        Value::Object(_) => params.clone(),
        _ => Value::Object(Map::new()),
    }
}

/// ファイルを開き、マジックナンバー・`format_version` と `DEPS` の必須キーを確かめる。
fn open<'a>(
    bytes: &'a [u8],
    file: &str,
    magic: [u8; 8],
    format_version: u32,
    required: &[&str],
) -> Result<(BinFile<'a>, Deps), LoadError> {
    let parsed = BinFile::parse(bytes)?;
    parsed.expect(magic, format_version)?;
    let deps = parsed.deps()?;
    for key in required {
        if !deps.contains_key(*key) {
            return Err(invalid(format!("{file}のDEPSに{key}がない")));
        }
    }
    Ok((parsed, deps))
}

/// 登録済みのファイル（`ROUTING_FILES`）の `format_version`。
fn format_version_of(file: &str) -> u32 {
    routing_file_spec(file).map_or(1, |(_, version)| version)
}

/// `PARAMS` の整数。キーがなければ `None`、整数でなければ失敗。
fn param_u64(params: &Value, key: &str) -> Result<Option<u64>, LoadError> {
    match params.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or_else(|| invalid(format!("PARAMSの{key}が非負の整数でない"))),
    }
}

/// `PARAMS` の整数（必須）。
fn require_param_u64(params: &Value, key: &str) -> Result<u64, LoadError> {
    param_u64(params, key)?.ok_or_else(|| invalid(format!("PARAMSに{key}がない")))
}

fn le_u16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le_i16(b: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([b[at], b[at + 1]])
}

fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn le_i32(b: &[u8], at: usize) -> i32 {
    i32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// `graph.bin`（形式書6章）。`DEPS` は `{}`。
pub fn encode_graph(graph: &Graph) -> Vec<u8> {
    encode_graph_parts(graph.parts())
}

/// `graph.bin` を節（`GraphParts`）から作る。P1が `Graph` を作らずに書き出すのに使う。
pub fn encode_graph_parts(parts: &GraphParts) -> Vec<u8> {
    let mut w = BinFileWriter::new(GRAPH_MAGIC, format_version_of(GRAPH_FILE));
    w.add_json(DEPS_SECTION, &deps_json(&Deps::new()));
    w.add_json(PARAMS_SECTION, &params_json(&parts.params));
    w.add_json(
        "TIMEMODL",
        &serde_json::to_value(&parts.time_model).expect("TimeModelはJSONに書ける"),
    );
    w.add_values("NODEOFS", &parts.node_ofs);
    let mut adj = Vec::with_capacity(parts.adj.len() * 4);
    for d in &parts.adj {
        put_u32(&mut adj, d.0);
    }
    w.add_section("ADJ", 4, parts.adj.len() as u64, adj);
    let mut node_ll = Vec::with_capacity(parts.node_ll.len() * Coord::SIZE);
    for c in &parts.node_ll {
        put_i32(&mut node_ll, c.lat_e7);
        put_i32(&mut node_ll, c.lon_e7);
    }
    w.add_section(
        "NODELL",
        Coord::SIZE as u32,
        parts.node_ll.len() as u64,
        node_ll,
    );
    w.add_values("NODEFLG", &parts.node_flags);
    let mut edges = Vec::with_capacity(parts.edges.len() * EdgeRecord::SIZE);
    for e in &parts.edges {
        put_u32(&mut edges, e.tail);
        put_u32(&mut edges, e.head);
        put_u32(&mut edges, e.length_dm);
        edges.extend_from_slice(&[e.class, e.access, e.flags, e.maxspeed_kmh]);
    }
    w.add_section(
        "EDGE",
        EdgeRecord::SIZE as u32,
        parts.edges.len() as u64,
        edges,
    );
    w.add_values("SHPOFS", &parts.shape_ofs);
    let mut shape = Vec::with_capacity(parts.shape.len() * ShapePoint::SIZE);
    for p in &parts.shape {
        shape.extend_from_slice(&p.dlat_e7.to_le_bytes());
        shape.extend_from_slice(&p.dlon_e7.to_le_bytes());
    }
    w.add_section(
        "SHAPE",
        ShapePoint::SIZE as u32,
        parts.shape.len() as u64,
        shape,
    );
    w.finish()
}

/// `graph.bin` を読み、節の件数の整合（`PARAMS` の `nodes`・`edges`・`shape_points` との一致）を確かめて
/// `GraphParts` にする。6.6節のグラフの検査（`NODEOFS` の単調性など）は `Graph::from_parts` が行う。
pub fn decode_graph_parts(bytes: &[u8]) -> Result<Decoded<GraphParts>, LoadError> {
    let (file, deps) = open(
        bytes,
        GRAPH_FILE,
        GRAPH_MAGIC,
        format_version_of(GRAPH_FILE),
        required_deps(GRAPH_FILE),
    )?;
    let params = file.json(PARAMS_SECTION)?;
    let nodes = require_param_u64(&params, "nodes")?;
    let edges = require_param_u64(&params, "edges")?;
    let shape_points = require_param_u64(&params, "shape_points")?;
    let time_model = TimeModel::from_json(&file.json("TIMEMODL")?)
        .map_err(|_| invalid("TIMEMODLが所要時間のモデルとして読めない"))?;

    let node_ofs_section = file.require("NODEOFS")?;
    node_ofs_section.expect_count(nodes + 1)?;
    let node_ofs = node_ofs_section.u32s()?;

    let adj_section = file.require("ADJ")?;
    adj_section.expect_count(edges * 2)?;
    let adj: Vec<DirEdge> = adj_section.u32s()?.into_iter().map(DirEdge).collect();

    let ll_section = file.require("NODELL")?;
    ll_section.expect_count(nodes)?;
    let node_ll: Vec<Coord> = ll_section
        .records(Coord::SIZE as u32)?
        .map(|r| Coord::new(le_i32(r, 0), le_i32(r, 4)))
        .collect();

    let flags_section = file.require("NODEFLG")?;
    flags_section.expect_count(nodes)?;
    let node_flags = flags_section.u8s()?;

    let edge_section = file.require("EDGE")?;
    edge_section.expect_count(edges)?;
    let edge_records: Vec<EdgeRecord> = edge_section
        .records(EdgeRecord::SIZE as u32)?
        .map(|r| EdgeRecord {
            tail: le_u32(r, 0),
            head: le_u32(r, 4),
            length_dm: le_u32(r, 8),
            class: r[12],
            access: r[13],
            flags: r[14],
            maxspeed_kmh: r[15],
        })
        .collect();

    let shape_ofs_section = file.require("SHPOFS")?;
    shape_ofs_section.expect_count(edges + 1)?;
    let shape_ofs = shape_ofs_section.u32s()?;

    let shape_section = file.require("SHAPE")?;
    shape_section.expect_count(shape_points)?;
    let shape: Vec<ShapePoint> = shape_section
        .records(ShapePoint::SIZE as u32)?
        .map(|r| ShapePoint {
            dlat_e7: le_i16(r, 0),
            dlon_e7: le_i16(r, 2),
        })
        .collect();

    Ok(Decoded {
        value: GraphParts {
            params,
            time_model,
            node_ofs,
            adj,
            node_ll,
            node_flags,
            edges: edge_records,
            shape_ofs,
            shape,
        },
        deps,
    })
}

/// `graph.bin` を読んで `Graph` にする（節の整合は `decode_graph_parts`、グラフの検査は
/// `Graph::from_parts`）。
pub fn decode_graph(bytes: &[u8]) -> Result<Decoded<Graph>, LoadError> {
    let Decoded { value, deps } = decode_graph_parts(bytes)?;
    let graph = Graph::from_parts(value).map_err(|e| invalid(e.to_string()))?;
    Ok(Decoded { value: graph, deps })
}

/// `graph.bin` を、ファイル全体を `Vec<u8>` に読まずに節ごとに読む。
///
/// 容器・マジックナンバー・`format_version`・`DEPS`・`PARAMS` と各節の件数の検査は
/// [`decode_graph_parts`] と同じ。節の本体は必要な型の `Vec` に直接読み込む。
pub fn decode_graph_from_file(path: &Path) -> Result<Decoded<Graph>, LoadError> {
    let mut reader = BinFileReader::open(path)?;
    reader.expect(GRAPH_MAGIC, format_version_of(GRAPH_FILE))?;
    let deps = reader.deps()?;
    for key in required_deps(GRAPH_FILE) {
        if !deps.contains_key(*key) {
            return Err(invalid(format!("{GRAPH_FILE}のDEPSに{key}がない")));
        }
    }
    let params = reader.json(PARAMS_SECTION)?;
    let nodes = require_param_u64(&params, "nodes")?;
    let edges = require_param_u64(&params, "edges")?;
    let shape_points = require_param_u64(&params, "shape_points")?;
    let time_model = TimeModel::from_json(&reader.json("TIMEMODL")?)
        .map_err(|_| invalid("TIMEMODLが所要時間のモデルとして読めない"))?;

    let node_ofs_section = reader.require("NODEOFS")?;
    node_ofs_section.expect_count(nodes + 1)?;
    let node_ofs = reader.read_values::<u32>(&node_ofs_section)?;

    let adj_section = reader.require("ADJ")?;
    adj_section.expect_count(edges * 2)?;
    adj_section.expect_elem_size(4)?;
    let adj: Vec<DirEdge> = reader
        .read_values::<u32>(&adj_section)?
        .into_iter()
        .map(DirEdge)
        .collect();

    let ll_section = reader.require("NODELL")?;
    ll_section.expect_count(nodes)?;
    let node_ll: Vec<Coord> = reader.read_mapped(&ll_section, Coord::SIZE as u32, |r| {
        Coord::new(le_i32(r, 0), le_i32(r, 4))
    })?;

    let flags_section = reader.require("NODEFLG")?;
    flags_section.expect_count(nodes)?;
    let node_flags = reader.read_values::<u8>(&flags_section)?;

    let edge_section = reader.require("EDGE")?;
    edge_section.expect_count(edges)?;
    let edge_records: Vec<EdgeRecord> =
        reader.read_mapped(&edge_section, EdgeRecord::SIZE as u32, |r| EdgeRecord {
            tail: le_u32(r, 0),
            head: le_u32(r, 4),
            length_dm: le_u32(r, 8),
            class: r[12],
            access: r[13],
            flags: r[14],
            maxspeed_kmh: r[15],
        })?;

    let shape_ofs_section = reader.require("SHPOFS")?;
    shape_ofs_section.expect_count(edges + 1)?;
    let shape_ofs = reader.read_values::<u32>(&shape_ofs_section)?;

    let shape_section = reader.require("SHAPE")?;
    shape_section.expect_count(shape_points)?;
    let shape: Vec<ShapePoint> =
        reader.read_mapped(&shape_section, ShapePoint::SIZE as u32, |r| ShapePoint {
            dlat_e7: le_i16(r, 0),
            dlon_e7: le_i16(r, 2),
        })?;

    let parts = GraphParts {
        params,
        time_model,
        node_ofs,
        adj,
        node_ll,
        node_flags,
        edges: edge_records,
        shape_ofs,
        shape,
    };
    let graph = Graph::from_parts(parts).map_err(|e| invalid(e.to_string()))?;
    Ok(Decoded { value: graph, deps })
}

/// `alt.bin` を、ファイル全体を `Vec<u8>` に読まずに節ごとに読む。
///
/// `PARAMS` と節の有無・件数の検査は [`decode_alt`] と同じ。
pub fn decode_alt_from_file(path: &Path) -> Result<Decoded<Landmarks>, LoadError> {
    use crate::contract::Mode;

    let invalid = |message: String| LoadError::Invalid(format!("alt.bin: {message}"));
    let mut reader = BinFileReader::open(path)?;
    reader.expect(ALT_MAGIC, format_version_of(ALT_FILE))?;
    let deps = reader.deps()?;
    for key in required_deps(ALT_FILE) {
        if !deps.contains_key(*key) {
            return Err(invalid(format!("DEPSに{key}がない")));
        }
    }
    let raw_params = reader.json(PARAMS_SECTION)?;
    let params: crate::domain::routing::search::alt::AltParams =
        serde_json::from_value(raw_params.clone())
            .map_err(|_| invalid("PARAMSが距離表の形として読めない".to_owned()))?;
    if params.unit_ds == 0 {
        return Err(invalid("PARAMSのunit_dsが1未満".to_owned()));
    }
    for key in params.landmarks.keys().chain(params.directions.keys()) {
        if Mode::parse(key).is_none() {
            return Err(invalid(format!(
                "PARAMSの{key}が手段（car・bike・walk）でない"
            )));
        }
    }

    let mut tables: [Option<crate::domain::routing::search::alt::ModeLandmarks>; 3] =
        [None, None, None];
    let mut expected_nodes: Option<u64> = None;
    for mode in Mode::ALL {
        let key = mode.as_str();
        let name = alt_mode_name(mode);
        let (lm_name, fr_name, to_name) = (
            format!("LM_{name}"),
            format!("FR_{name}"),
            format!("TO_{name}"),
        );
        let count = params.landmarks.get(key).copied();
        let directions = params.directions.get(key);
        let (Some(count), Some(directions)) = (count, directions) else {
            if count.is_some() || directions.is_some() {
                return Err(invalid(format!(
                    "PARAMSの{key}のlandmarksとdirectionsの有無が一致しない"
                )));
            }
            for section in [&lm_name, &fr_name, &to_name] {
                if reader.section(section).is_some() {
                    return Err(invalid(format!("PARAMSに{key}がないのに節{section}がある")));
                }
            }
            continue;
        };
        let with_to = match directions.as_slice() {
            [from] if from == "from" => false,
            [from, to] if from == "from" && to == "to" => true,
            _ => {
                return Err(invalid(format!(
                    "PARAMSの{key}のdirectionsが[\"from\"]か[\"from\",\"to\"]でない"
                )));
            }
        };

        let lm_section = reader.require(&lm_name)?;
        lm_section.expect_count(u64::from(count))?;
        let nodes = reader.read_values::<u32>(&lm_section)?;

        let fr_section = reader.require(&fr_name)?;
        if count == 0 {
            fr_section.expect_count(0)?;
        } else {
            if fr_section.count % u64::from(count) != 0 {
                return Err(invalid(format!(
                    "節{fr_name}の件数がランドマークの数{count}の倍数でない"
                )));
            }
            let n = fr_section.count / u64::from(count);
            if n == 0 {
                return Err(invalid(format!(
                    "節{fr_name}のノード数が0（ランドマークがあるのに表が空）"
                )));
            }
            match expected_nodes {
                Some(expected) if expected != n => {
                    return Err(invalid(format!(
                        "節{fr_name}のノード数{n}が他の手段の{expected}と一致しない"
                    )));
                }
                None => expected_nodes = Some(n),
                _ => {}
            }
        }
        let from = reader.read_values::<u16>(&fr_section)?;

        let to = if with_to {
            let to_section = reader.require(&to_name)?;
            to_section.expect_count(fr_section.count)?;
            Some(reader.read_values::<u16>(&to_section)?)
        } else {
            if reader.section(&to_name).is_some() {
                return Err(invalid(format!(
                    "PARAMSの{key}のdirectionsにtoがないのに節{to_name}がある"
                )));
            }
            None
        };
        tables[crate::domain::routing::risk::mode_index(mode)] =
            Some(crate::domain::routing::search::alt::ModeLandmarks { nodes, from, to });
    }
    Ok(Decoded {
        value: Landmarks {
            params,
            raw_params,
            tables,
        },
        deps,
    })
}

/// `graph_attrs.bin`（形式書16章、中間生成物）。節は `WAYID`・`NODEID`（形式書16章への追加）・`REF`・
/// `NAME`・`STRTAB`・`LANES`・`WIDTHDM`・`LAYER`。`DEPS` は `graph.bin` のハッシュ。
pub fn encode_graph_attrs(attrs: &GraphAttrs, deps: &Deps) -> Vec<u8> {
    let mut w = BinFileWriter::new(GRAPH_ATTRS_MAGIC, 1);
    w.add_json(DEPS_SECTION, &deps_json(deps));
    w.add_json(PARAMS_SECTION, &params_json(&attrs.params));
    w.add_values("WAYID", &attrs.way_id);
    w.add_values("NODEID", &attrs.node_id);
    w.add_values("REF", &attrs.ref_idx);
    w.add_values("NAME", &attrs.name_idx);
    w.add_values("STRTAB", &attrs.strtab);
    w.add_values("LANES", &attrs.lanes);
    w.add_values("WIDTHDM", &attrs.width_dm);
    w.add_values("LAYER", &attrs.layer);
    w.finish()
}

pub fn decode_graph_attrs(bytes: &[u8]) -> Result<Decoded<GraphAttrs>, LoadError> {
    let (file, deps) = open(
        bytes,
        "graph_attrs.bin",
        GRAPH_ATTRS_MAGIC,
        1,
        &[GRAPH_FILE],
    )?;
    let params = file.json(PARAMS_SECTION)?;
    let edges = require_param_u64(&params, "edges")?;
    let nodes = require_param_u64(&params, "nodes")?;
    let way_id = file.require("WAYID")?;
    way_id.expect_count(edges)?;
    let node_id = file.require("NODEID")?;
    node_id.expect_count(nodes)?;
    let ref_idx = file.require("REF")?;
    ref_idx.expect_count(edges)?;
    let name_idx = file.require("NAME")?;
    name_idx.expect_count(edges)?;
    let lanes = file.require("LANES")?;
    lanes.expect_count(edges)?;
    let width = file.require("WIDTHDM")?;
    width.expect_count(edges)?;
    let layer = file.require("LAYER")?;
    layer.expect_count(edges)?;
    let attrs = GraphAttrs {
        params,
        way_id: way_id.values()?,
        node_id: node_id.values()?,
        ref_idx: ref_idx.u32s()?,
        name_idx: name_idx.u32s()?,
        strtab: file.require("STRTAB")?.u8s()?,
        lanes: lanes.u8s()?,
        width_dm: width.u16s()?,
        layer: layer.values()?,
    };
    if attrs.way_id.windows(2).any(|w| w[0] > w[1]) {
        return Err(invalid("graph_attrs.binのWAYIDが昇順でない"));
    }
    if attrs.node_id.windows(2).any(|w| w[0] >= w[1]) {
        return Err(invalid("graph_attrs.binのNODEIDが昇順でない"));
    }
    let strtab_len = attrs.strtab.len() as u64;
    if attrs
        .ref_idx
        .iter()
        .chain(&attrs.name_idx)
        .any(|&i| u64::from(i) >= strtab_len)
    {
        return Err(invalid("graph_attrs.binのREF・NAMEがSTRTABの範囲外"));
    }
    Ok(Decoded { value: attrs, deps })
}

/// `restrictions.bin`（形式書9章）。`items` は呼び出し側が列の辞書順・重複なしに並べておく
/// （`SEQ` はその順に隙間なく詰める）。
pub fn encode_restrictions(restrictions: &Restrictions, deps: &Deps) -> Vec<u8> {
    let mut w = BinFileWriter::new(RESTRICTIONS_MAGIC, format_version_of(RESTRICTIONS_FILE));
    w.add_json(DEPS_SECTION, &deps_json(deps));
    w.add_json(PARAMS_SECTION, &params_json(&restrictions.params));
    let mut records = Vec::with_capacity(restrictions.items.len() * Restriction::RECORD_SIZE);
    let mut seq: Vec<u8> = Vec::new();
    let mut seq_count = 0u64;
    for item in &restrictions.items {
        let seq_len = u16::try_from(item.seq.len()).expect("禁止の列が65535以下");
        put_u32(&mut records, seq_count as u32);
        records.extend_from_slice(&seq_len.to_le_bytes());
        records.extend_from_slice(&[item.modes, item.kind]);
        for d in &item.seq {
            put_u32(&mut seq, d.0);
        }
        seq_count += u64::from(seq_len);
    }
    w.add_section(
        "RESTR",
        Restriction::RECORD_SIZE as u32,
        restrictions.items.len() as u64,
        records,
    );
    w.add_section("SEQ", 4, seq_count, seq);
    w.finish()
}

pub fn decode_restrictions(bytes: &[u8]) -> Result<Decoded<Restrictions>, LoadError> {
    let (file, deps) = open(
        bytes,
        RESTRICTIONS_FILE,
        RESTRICTIONS_MAGIC,
        format_version_of(RESTRICTIONS_FILE),
        required_deps(RESTRICTIONS_FILE),
    )?;
    let params = file.json(PARAMS_SECTION)?;
    let restr = file.require("RESTR")?;
    let records = restr.records(Restriction::RECORD_SIZE as u32)?;
    let seq: Vec<u32> = file.require("SEQ")?.u32s()?;
    if let Some(expected) = param_u64(&params, "items")? {
        restr.expect_count(expected)?;
    }
    let mut items = Vec::with_capacity(restr.count as usize);
    let mut next = 0usize;
    for r in records {
        let start = le_u32(r, 0) as usize;
        let len = usize::from(le_u16(r, 4));
        let (modes, kind) = (r[6], r[7]);
        if len < 2 {
            return Err(invalid("restrictions.binの列が2未満"));
        }
        if modes == 0 || modes & !7 != 0 {
            return Err(invalid("restrictions.binのmodesが不正"));
        }
        if kind > 1 {
            return Err(invalid("restrictions.binのkindが不正"));
        }
        if start != next {
            return Err(invalid("restrictions.binのSEQが隙間なく並んでいない"));
        }
        let end = start + len;
        if end > seq.len() {
            return Err(invalid("restrictions.binのSEQが範囲外"));
        }
        items.push(Restriction {
            seq: seq[start..end].iter().map(|&d| DirEdge(d)).collect(),
            modes,
            kind,
        });
        next = end;
    }
    if next != seq.len() {
        return Err(invalid(
            "restrictions.binのSEQにRESTRから参照されない要素がある",
        ));
    }
    Ok(Decoded {
        value: Restrictions { params, items },
        deps,
    })
}

/// `topology.bin`（形式書10章）。
pub fn encode_topology(topology: &Topology, deps: &Deps) -> Vec<u8> {
    let mut w = BinFileWriter::new(TOPOLOGY_MAGIC, format_version_of(TOPOLOGY_FILE));
    w.add_json(DEPS_SECTION, &deps_json(deps));
    w.add_json(PARAMS_SECTION, &params_json(&topology.params));
    w.add_values("SNAPCAR", &topology.snap_car);
    w.add_values("SNAPBIK", &topology.snap_bike);
    w.add_values("SNAPFOT", &topology.snap_walk);
    w.add_values("NODECLS", &topology.node_cluster);
    let mut clusters = Vec::with_capacity(topology.clusters.len() * ClusterRecord::SIZE);
    for c in &topology.clusters {
        put_i32(&mut clusters, c.lat_e7);
        put_i32(&mut clusters, c.lon_e7);
        put_u32(&mut clusters, c.node_start);
        clusters.extend_from_slice(&c.node_count.to_le_bytes());
        clusters.extend_from_slice(&[c.top_class, c.branches, c.flags]);
        clusters.extend_from_slice(&c.pad);
    }
    w.add_section(
        "CLUSTER",
        ClusterRecord::SIZE as u32,
        topology.clusters.len() as u64,
        clusters,
    );
    w.add_values("CLSNODE", &topology.cluster_nodes);
    let mut twins = Vec::with_capacity(topology.twins.len() * 8);
    for t in &topology.twins {
        put_u32(&mut twins, t.a);
        put_u32(&mut twins, t.b);
    }
    w.add_section("TWIN", 8, topology.twins.len() as u64, twins);
    let mut parents = Vec::with_capacity(topology.sidewalk_parents.len() * 8);
    for s in &topology.sidewalk_parents {
        put_u32(&mut parents, s.sidewalk);
        put_u32(&mut parents, s.parent);
    }
    w.add_section(
        "SWPARENT",
        8,
        topology.sidewalk_parents.len() as u64,
        parents,
    );
    w.finish()
}

/// `topology.bin` を読む。`PARAMS` の `nodes`・`edges`（あれば）との件数の整合と、クラスタ・双子・
/// 歩道の対応の内部の整合を確かめる。グラフとの照合（辺番号の範囲など）は `Topology::validate`。
pub fn decode_topology(bytes: &[u8]) -> Result<Decoded<Topology>, LoadError> {
    let (file, deps) = open(
        bytes,
        TOPOLOGY_FILE,
        TOPOLOGY_MAGIC,
        format_version_of(TOPOLOGY_FILE),
        required_deps(TOPOLOGY_FILE),
    )?;
    let params = file.json(PARAMS_SECTION)?;
    let snap_car = file.require("SNAPCAR")?.u64s()?;
    let snap_bike = file.require("SNAPBIK")?.u64s()?;
    let snap_walk = file.require("SNAPFOT")?.u64s()?;
    if snap_car.len() != snap_bike.len() || snap_car.len() != snap_walk.len() {
        return Err(invalid("topology.binのSNAP*の長さが一致しない"));
    }
    if let Some(edges) = param_u64(&params, "edges")?
        && snap_car.len() as u64 != edges.div_ceil(64)
    {
        return Err(invalid(
            "topology.binのSNAP*の長さがPARAMSのedgesと矛盾する",
        ));
    }
    let node_cluster_section = file.require("NODECLS")?;
    if let Some(nodes) = param_u64(&params, "nodes")? {
        node_cluster_section.expect_count(nodes)?;
    }
    let node_cluster = node_cluster_section.u32s()?;

    let cluster_section = file.require("CLUSTER")?;
    let clusters: Vec<ClusterRecord> = cluster_section
        .records(ClusterRecord::SIZE as u32)?
        .map(|r| ClusterRecord {
            lat_e7: le_i32(r, 0),
            lon_e7: le_i32(r, 4),
            node_start: le_u32(r, 8),
            node_count: le_u16(r, 12),
            top_class: r[14],
            branches: r[15],
            flags: r[16],
            pad: [r[17], r[18], r[19]],
        })
        .collect();
    let cluster_nodes = file.require("CLSNODE")?.u32s()?;

    // NODECLS の値と、CLSNODE・CLUSTER の対応
    let cluster_count = clusters.len() as u64;
    let mut expected_start = 0u64;
    for (i, c) in clusters.iter().enumerate() {
        if u64::from(c.node_start) != expected_start || c.node_count == 0 {
            return Err(invalid(
                "topology.binのCLUSTERのnode_startが隙間なく並んでいない",
            ));
        }
        let end = expected_start + u64::from(c.node_count);
        if end > cluster_nodes.len() as u64 {
            return Err(invalid("topology.binのCLSNODEがCLUSTERより短い"));
        }
        let members = &cluster_nodes[expected_start as usize..end as usize];
        if members.windows(2).any(|w| w[0] >= w[1]) {
            return Err(invalid("topology.binのCLSNODEがクラスタ内で昇順でない"));
        }
        for &node in members {
            if node_cluster.get(node as usize) != Some(&(i as u32)) {
                return Err(invalid("topology.binのCLSNODEとNODECLSが一致しない"));
            }
        }
        expected_start = end;
    }
    if expected_start != cluster_nodes.len() as u64 {
        return Err(invalid(
            "topology.binのCLSNODEにCLUSTERから参照されない要素がある",
        ));
    }
    let assigned = node_cluster
        .iter()
        .filter(|&&c| c != crate::domain::routing::NONE)
        .count() as u64;
    if assigned != cluster_nodes.len() as u64
        || node_cluster
            .iter()
            .any(|&c| c != crate::domain::routing::NONE && u64::from(c) >= cluster_count)
    {
        return Err(invalid("topology.binのNODECLSの値がクラスタの数と矛盾する"));
    }

    let edge_limit = param_u64(&params, "edges")?;
    let twins: Vec<TwinPair> = file
        .require("TWIN")?
        .records(8)?
        .map(|r| TwinPair {
            a: le_u32(r, 0),
            b: le_u32(r, 4),
        })
        .collect();
    if twins
        .windows(2)
        .any(|w| (w[0].a, w[0].b) >= (w[1].a, w[1].b))
    {
        return Err(invalid("topology.binのTWINが(a,b)の辞書順の狭義昇順でない"));
    }
    for t in &twins {
        if edge_limit.is_some_and(|e| u64::from(t.a) >= e || u64::from(t.b) >= e) {
            return Err(invalid("topology.binのTWINの辺番号が範囲外"));
        }
        if t.a == t.b {
            return Err(invalid("topology.binのTWINが同じ辺の組"));
        }
        if twins
            .binary_search_by_key(&(t.b, t.a), |x| (x.a, x.b))
            .is_err()
        {
            return Err(invalid("topology.binのTWINに逆向きの組がない"));
        }
    }
    let sidewalk_parents: Vec<SidewalkParent> = file
        .require("SWPARENT")?
        .records(8)?
        .map(|r| SidewalkParent {
            sidewalk: le_u32(r, 0),
            parent: le_u32(r, 4),
        })
        .collect();
    if sidewalk_parents
        .windows(2)
        .any(|w| w[0].sidewalk >= w[1].sidewalk)
    {
        return Err(invalid("topology.binのSWPARENTがsidewalkの昇順でない"));
    }
    if sidewalk_parents
        .iter()
        .any(|s| edge_limit.is_some_and(|e| u64::from(s.sidewalk) >= e || u64::from(s.parent) >= e))
    {
        return Err(invalid("topology.binのSWPARENTの辺番号が範囲外"));
    }
    Ok(Decoded {
        value: Topology {
            params,
            snap_car,
            snap_bike,
            snap_walk,
            node_cluster,
            clusters,
            cluster_nodes,
            twins,
            sidewalk_parents,
        },
        deps,
    })
}

/// `odindex.bin`（形式書11章）。`PARAMS` は `raw_params` に、型付きの `params` を上書きして書く。
pub fn encode_odindex(index: &OdIndex, deps: &Deps) -> Vec<u8> {
    let mut params = match &index.raw_params {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    };
    if let Value::Object(typed) = serde_json::to_value(&index.params).expect("JSONに書ける") {
        params.extend(typed);
    }
    let mut w = BinFileWriter::new(ODINDEX_MAGIC, format_version_of(ODINDEX_FILE));
    w.add_json(DEPS_SECTION, &deps_json(deps));
    w.add_json(PARAMS_SECTION, &Value::Object(params));
    w.add_values("CELLKEY", &index.cell_keys);
    w.add_values("CELLOFS", &index.cell_ofs);
    w.add_values("CELLEDG", &index.cell_edges);
    w.finish()
}

pub fn decode_odindex(bytes: &[u8]) -> Result<Decoded<OdIndex>, LoadError> {
    let (file, deps) = open(
        bytes,
        ODINDEX_FILE,
        ODINDEX_MAGIC,
        format_version_of(ODINDEX_FILE),
        required_deps(ODINDEX_FILE),
    )?;
    let raw_params = file.json(PARAMS_SECTION)?;
    let params: OdIndexParams = serde_json::from_value(raw_params.clone())
        .map_err(|_| invalid("odindex.binのPARAMSが格子の形として読めない"))?;
    if params.cell_lat_e7 <= 0 || params.cell_lon_e7 <= 0 || params.rows == 0 || params.cols == 0 {
        return Err(invalid("odindex.binの格子の形が不正"));
    }
    let key_section = file.require("CELLKEY")?;
    let cell_keys = key_section.u32s()?;
    let ofs_section = file.require("CELLOFS")?;
    ofs_section.expect_count(key_section.count + 1)?;
    let cell_ofs = ofs_section.u32s()?;
    let cell_edges = file.require("CELLEDG")?.u32s()?;

    let cells = u64::from(params.rows) * u64::from(params.cols);
    if cell_keys.windows(2).any(|w| w[0] >= w[1])
        || cell_keys.last().is_some_and(|&k| u64::from(k) >= cells)
    {
        return Err(invalid("odindex.binのCELLKEYが昇順でない、または格子の外"));
    }
    if cell_ofs.first() != Some(&0)
        || cell_ofs.windows(2).any(|w| w[0] >= w[1])
        || cell_ofs.last().map(|&o| o as usize) != Some(cell_edges.len())
    {
        return Err(invalid(
            "odindex.binのCELLOFSが空でないセルの区切りになっていない",
        ));
    }
    for w in cell_ofs.windows(2) {
        let cell = &cell_edges[w[0] as usize..w[1] as usize];
        if cell.windows(2).any(|e| e[0] >= e[1]) {
            return Err(invalid("odindex.binのCELLEDGがセル内で昇順でない"));
        }
    }
    Ok(Decoded {
        value: OdIndex {
            params,
            raw_params,
            cell_keys,
            cell_ofs,
            cell_edges,
        },
        deps,
    })
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

/// `assignment.bin`（形式書13章）。節は `DEPS`・`PARAMS`・`ACCOFS`・`TARGET`・`UNASSGN` の順。
/// `PARAMS` は `assignment.params` をそのまま書く（`accidents` を含めると、読込側が `ACCOFS` の件数と
/// 照合する）。`assignment` は事故IDの順・候補の昇順に並べてあること。
pub fn encode_assignment(assignment: &Assignment, deps: &Deps) -> Vec<u8> {
    use crate::domain::routing::accident::{AssignTarget, UnassignedRecord};

    let mut w = BinFileWriter::new(ASSIGNMENT_MAGIC, format_version_of(ASSIGNMENT_FILE));
    w.add_json(DEPS_SECTION, &deps_json(deps));
    w.add_json(PARAMS_SECTION, &params_json(&assignment.params));
    w.add_values("ACCOFS", &assignment.offsets);
    let mut targets = Vec::with_capacity(assignment.targets.len() * AssignTarget::SIZE);
    for t in &assignment.targets {
        put_u32(&mut targets, t.target);
        put_u32(&mut targets, t.pos_dm);
        targets.extend_from_slice(&t.weight.to_le_bytes());
        targets.push(t.kind);
        targets.push(t.flags);
        targets.extend_from_slice(&t.reserved.to_le_bytes());
    }
    w.add_section(
        "TARGET",
        AssignTarget::SIZE as u32,
        assignment.targets.len() as u64,
        targets,
    );
    let mut unassigned = Vec::with_capacity(assignment.unassigned.len() * UnassignedRecord::SIZE);
    for r in &assignment.unassigned {
        put_u32(&mut unassigned, r.accident);
        unassigned.extend_from_slice(&[r.reason.code(), 0, 0, 0]);
    }
    w.add_section(
        "UNASSGN",
        UnassignedRecord::SIZE as u32,
        assignment.unassigned.len() as u64,
        unassigned,
    );
    w.finish()
}

/// `assignment.bin` を読む。マジックナンバー・`format_version`・`DEPS` の必須キー・`elem_size`（`TARGET` は16、
/// `UNASSGN` は8）を確かめ、`PARAMS` の `accidents`（あれば）と `ACCOFS` の件数、`targets` と `unassigned`
/// （あれば）と各節の件数の整合、`reason` の値、`pad` が0であることを検査する。
/// 続けて `Assignment::validate_structure`（重みの範囲・合計・並び・排他性）で内容を検査する。
/// 辺番号・クラスタ番号の範囲は、`PARAMS` の `edges`・`clusters`（あれば）に対して確かめる。
pub fn decode_assignment(bytes: &[u8]) -> Result<Decoded<Assignment>, LoadError> {
    use crate::domain::routing::accident::{AssignTarget, UnassignedReason, UnassignedRecord};

    let invalid = |message: String| LoadError::Invalid(format!("assignment.bin: {message}"));
    let (file, deps) = open(
        bytes,
        ASSIGNMENT_FILE,
        ASSIGNMENT_MAGIC,
        format_version_of(ASSIGNMENT_FILE),
        required_deps(ASSIGNMENT_FILE),
    )?;
    let params = file.json(PARAMS_SECTION)?;
    let offsets_section = file.require("ACCOFS")?;
    if offsets_section.count == 0 {
        return Err(invalid("ACCOFSが空です".to_owned()));
    }
    if let Some(accidents) = param_u64(&params, "accidents")? {
        offsets_section.expect_count(accidents + 1)?;
    }
    let offsets = offsets_section.u32s()?;
    let target_section = file.require("TARGET")?;
    if let Some(targets) = param_u64(&params, "targets")? {
        target_section.expect_count(targets)?;
    }
    let targets: Vec<AssignTarget> = target_section
        .records(AssignTarget::SIZE as u32)?
        .map(|r| AssignTarget {
            target: le_u32(r, 0),
            pos_dm: le_u32(r, 4),
            weight: f32::from_le_bytes([r[8], r[9], r[10], r[11]]),
            kind: r[12],
            flags: r[13],
            reserved: le_u16(r, 14),
        })
        .collect();
    let unassigned_section = file.require("UNASSGN")?;
    if let Some(unassigned) = param_u64(&params, "unassigned")? {
        unassigned_section.expect_count(unassigned)?;
    }
    let mut unassigned = Vec::with_capacity(unassigned_section.count as usize);
    for r in unassigned_section.records(UnassignedRecord::SIZE as u32)? {
        let Some(reason) = UnassignedReason::from_code(r[4]) else {
            return Err(invalid(format!("UNASSGNのreason {}が不正です", r[4])));
        };
        if r[5..8] != [0, 0, 0] {
            return Err(invalid("UNASSGNのpadが0ではありません".to_owned()));
        }
        unassigned.push(UnassignedRecord {
            accident: le_u32(r, 0),
            reason,
        });
    }
    let assignment = Assignment {
        params,
        offsets,
        targets,
        unassigned,
    };
    assignment.validate_structure().map_err(invalid)?;
    if let (Some(edges), Some(clusters)) = (
        param_u64(&assignment.params, "edges")?,
        param_u64(&assignment.params, "clusters")?,
    ) {
        assignment
            .validate(edges as usize, clusters as usize)
            .map_err(invalid)?;
    }
    Ok(Decoded {
        value: assignment,
        deps,
    })
}

fn le_f32(bytes: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(bytes[at..at + 4].try_into().expect("4バイト"))
}

fn le_f64(bytes: &[u8], at: usize) -> f64 {
    f64::from_le_bytes(bytes[at..at + 8].try_into().expect("8バイト"))
}

/// `CoefRecord`（形式書14.1節、48バイト）をリトルエンディアンで並べる。
fn coef_record_bytes(record: &CoefRecord) -> [u8; CoefRecord::SIZE] {
    let mut out = [0_u8; CoefRecord::SIZE];
    out[0..8].copy_from_slice(&record.r.to_le_bytes());
    out[8..16].copy_from_slice(&record.s.to_le_bytes());
    out[16..24].copy_from_slice(&record.a.to_le_bytes());
    for (i, value) in record.rho.iter().enumerate() {
        let at = 24 + 4 * i;
        out[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    out[44..48].copy_from_slice(&record.pad.to_le_bytes());
    out
}

fn coef_record_from_bytes(bytes: &[u8]) -> CoefRecord {
    CoefRecord {
        r: le_f64(bytes, 0),
        s: le_f64(bytes, 8),
        a: le_f64(bytes, 16),
        rho: std::array::from_fn(|i| le_f32(bytes, 24 + 4 * i)),
        pad: le_f32(bytes, 44),
    }
}

/// `risk.bin`（形式書14章）。節は `DEPS`・`PARAMS`・`EDGEK`・`CLUSJ`・`KTAB`・`JTAB` の順。
/// `PARAMS` は `raw_params` に型付きの `params` を上書きして書く（`beta_cluster` なども含める）。
pub fn encode_risk(model: &RiskModel, deps: &Deps) -> Vec<u8> {
    let mut params = match &model.raw_params {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    };
    if let Value::Object(typed) = serde_json::to_value(&model.params).expect("JSONに書ける") {
        params.extend(typed);
    }
    let mut writer = BinFileWriter::new(RISK_MAGIC, format_version_of(RISK_FILE));
    writer.add_json(DEPS_SECTION, &deps_json(deps));
    writer.add_json(PARAMS_SECTION, &Value::Object(params));
    writer.add_values("EDGEK", &model.edge_k);
    writer.add_values("CLUSJ", &model.cluster_j);
    let mut k_table = Vec::with_capacity(model.k_table.len() * CoefRecord::SIZE);
    for record in &model.k_table {
        k_table.extend_from_slice(&coef_record_bytes(record));
    }
    writer.add_section(
        "KTAB",
        CoefRecord::SIZE as u32,
        model.k_table.len() as u64,
        k_table,
    );
    let mut j_table = Vec::with_capacity(model.j_table.len() * CoefRecord::SIZE);
    for record in &model.j_table {
        j_table.extend_from_slice(&coef_record_bytes(record));
    }
    writer.add_section(
        "JTAB",
        CoefRecord::SIZE as u32,
        model.j_table.len() as u64,
        j_table,
    );
    writer.finish()
}

/// `risk.bin` を読む。マジックナンバー・`format_version`・`DEPS` の必須キー・`KTAB`・`JTAB` の件数
/// （`TABLE_LEN`）と `elem_size`（48）を検査する。`PARAMS` の `beta_cluster` がなければ `beta` を使う。
pub fn decode_risk(bytes: &[u8]) -> Result<Decoded<RiskModel>, LoadError> {
    let (file, deps) = open(
        bytes,
        RISK_FILE,
        RISK_MAGIC,
        format_version_of(RISK_FILE),
        required_deps(RISK_FILE),
    )?;
    let raw_params = file.json(PARAMS_SECTION)?;
    let params: RiskParams = serde_json::from_value(raw_params.clone())
        .map_err(|_| invalid("risk.binのPARAMSがリスクの形として読めない"))?;
    let edge_k = file.require("EDGEK")?.u8s()?;
    let cluster_j = file.require("CLUSJ")?.u8s()?;
    let k_section = file.require("KTAB")?;
    k_section.expect_count(TABLE_LEN as u64)?;
    let k_table: Vec<CoefRecord> = k_section
        .records(CoefRecord::SIZE as u32)?
        .map(coef_record_from_bytes)
        .collect();
    let j_section = file.require("JTAB")?;
    j_section.expect_count(TABLE_LEN as u64)?;
    let j_table: Vec<CoefRecord> = j_section
        .records(CoefRecord::SIZE as u32)?
        .map(coef_record_from_bytes)
        .collect();
    Ok(Decoded {
        value: RiskModel {
            params,
            raw_params,
            edge_k,
            cluster_j,
            k_table,
            j_table,
        },
        deps,
    })
}

/// `alt.bin` の節名に使う手段の記号（形式書15章。`walk`→`FOT`・`bike`→`BIK`・`car`→`CAR`）。
fn alt_mode_name(mode: crate::contract::Mode) -> &'static str {
    match mode {
        crate::contract::Mode::Walk => "FOT",
        crate::contract::Mode::Bike => "BIK",
        crate::contract::Mode::Car => "CAR",
    }
}

/// `alt.bin`（形式書15章）。節は `DEPS`・`PARAMS`・手段ごとの `LM_<M>`・`FR_<M>`（と `TO_<M>`）の順。
///
/// `PARAMS` は `landmarks.raw_params` を書き、`AltParams` の値（`landmarks.params`）で上書きする。
/// こうすると、`AltParams` に入らないキー（選び方の始点など）も保ちつつ、型付きの値を変えた結果が
/// 必ず反映される。`LM_<M>` は `u32`、`FR_<M>`・`TO_<M>` は `u16`。
pub fn encode_alt(landmarks: &Landmarks, deps: &Deps) -> Vec<u8> {
    use crate::contract::Mode;

    let mut params = match &landmarks.raw_params {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    };
    if let Value::Object(typed) = serde_json::to_value(&landmarks.params).expect("JSONに書ける")
    {
        params.extend(typed);
    }
    let mut w = BinFileWriter::new(ALT_MAGIC, format_version_of(ALT_FILE));
    w.add_json(DEPS_SECTION, &deps_json(deps));
    w.add_json(PARAMS_SECTION, &Value::Object(params));
    for mode in Mode::ALL {
        let Some(table) = landmarks.tables[crate::domain::routing::risk::mode_index(mode)].as_ref()
        else {
            continue;
        };
        let name = alt_mode_name(mode);
        w.add_values(&format!("LM_{name}"), &table.nodes);
        w.add_values(&format!("FR_{name}"), &table.from);
        if let Some(to) = &table.to {
            w.add_values(&format!("TO_{name}"), to);
        }
    }
    w.finish()
}

/// `alt.bin` を読む。形式書15章のとおり、マジックナンバー・`format_version`・`DEPS` の
/// `graph.bin` を確かめ、`PARAMS` の `landmarks`・`directions` と節の有無の一致、`unit_ds ≥ 1`、
/// `FR_<M>` の件数（Lの倍数・全手段で同じノード数N）、`TO_<M>` の件数（`FR_<M>` と同じ）を検査する。
pub fn decode_alt(bytes: &[u8]) -> Result<Decoded<Landmarks>, LoadError> {
    use crate::contract::Mode;

    let invalid = |message: String| LoadError::Invalid(format!("alt.bin: {message}"));
    let (file, deps) = open(
        bytes,
        ALT_FILE,
        ALT_MAGIC,
        format_version_of(ALT_FILE),
        required_deps(ALT_FILE),
    )?;
    let raw_params = file.json(PARAMS_SECTION)?;
    let params: crate::domain::routing::search::alt::AltParams =
        serde_json::from_value(raw_params.clone())
            .map_err(|_| invalid("PARAMSが距離表の形として読めない".to_owned()))?;
    if params.unit_ds == 0 {
        return Err(invalid("PARAMSのunit_dsが1未満".to_owned()));
    }
    for key in params.landmarks.keys().chain(params.directions.keys()) {
        if Mode::parse(key).is_none() {
            return Err(invalid(format!(
                "PARAMSの{key}が手段（car・bike・walk）でない"
            )));
        }
    }

    let mut tables: [Option<crate::domain::routing::search::alt::ModeLandmarks>; 3] =
        [None, None, None];
    // 表を持つ手段で共通のノード数N。L=0の表はNを決めない。
    let mut expected_nodes: Option<u64> = None;
    for mode in Mode::ALL {
        let key = mode.as_str();
        let name = alt_mode_name(mode);
        let (lm_name, fr_name, to_name) = (
            format!("LM_{name}"),
            format!("FR_{name}"),
            format!("TO_{name}"),
        );
        let count = params.landmarks.get(key).copied();
        let directions = params.directions.get(key);
        let (Some(count), Some(directions)) = (count, directions) else {
            if count.is_some() || directions.is_some() {
                return Err(invalid(format!(
                    "PARAMSの{key}のlandmarksとdirectionsの有無が一致しない"
                )));
            }
            for section in [&lm_name, &fr_name, &to_name] {
                if file.section(section).is_some() {
                    return Err(invalid(format!("PARAMSに{key}がないのに節{section}がある")));
                }
            }
            continue;
        };
        let with_to = match directions.as_slice() {
            [from] if from == "from" => false,
            [from, to] if from == "from" && to == "to" => true,
            _ => {
                return Err(invalid(format!(
                    "PARAMSの{key}のdirectionsが[\"from\"]か[\"from\",\"to\"]でない"
                )));
            }
        };

        let lm_section = file.require(&lm_name)?;
        lm_section.expect_count(u64::from(count))?;
        let nodes = lm_section.u32s()?;

        let fr_section = file.require(&fr_name)?;
        if count == 0 {
            fr_section.expect_count(0)?;
        } else {
            if fr_section.count % u64::from(count) != 0 {
                return Err(invalid(format!(
                    "節{fr_name}の件数がランドマークの数{count}の倍数でない"
                )));
            }
            let n = fr_section.count / u64::from(count);
            if n == 0 {
                return Err(invalid(format!(
                    "節{fr_name}のノード数が0（ランドマークがあるのに表が空）"
                )));
            }
            match expected_nodes {
                Some(expected) if expected != n => {
                    return Err(invalid(format!(
                        "節{fr_name}のノード数{n}が他の手段の{expected}と一致しない"
                    )));
                }
                None => expected_nodes = Some(n),
                _ => {}
            }
        }
        let from = fr_section.u16s()?;

        let to = if with_to {
            let to_section = file.require(&to_name)?;
            to_section.expect_count(fr_section.count)?;
            Some(to_section.u16s()?)
        } else {
            if file.section(&to_name).is_some() {
                return Err(invalid(format!(
                    "PARAMSの{key}のdirectionsにtoがないのに節{to_name}がある"
                )));
            }
            None
        };
        tables[crate::domain::routing::risk::mode_index(mode)] =
            Some(crate::domain::routing::search::alt::ModeLandmarks { nodes, from, to });
    }
    Ok(Decoded {
        value: Landmarks {
            params,
            raw_params,
            tables,
        },
        deps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::Mode;
    use crate::data::release::{ACCIDENTS_FILE, ROUTING_FILES};
    const NONE_U32: u32 = u32::MAX;

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

    // --- encode → decode の往復と、壊れたファイルの拒否 ---

    use crate::data::binfile::BinFile;
    use crate::data::release::{GRAPH_FILE, sha256_hex};
    use crate::domain::routing::graph::synthetic::{
        EdgeSpec, GraphBuilder, RandomGraphConfig, random_graph,
    };
    use crate::domain::routing::prep::odindex::build_odindex;
    use crate::domain::routing::prep::topo::{TopologyConfig, build_topology};
    use serde_json::json;

    fn deps_of(graph_bytes: &[u8]) -> Deps {
        Deps::from([(GRAPH_FILE.to_owned(), sha256_hex(graph_bytes))])
    }

    fn sample() -> (Graph, Restrictions) {
        random_graph(7, &RandomGraphConfig::rich(60)).build_all()
    }

    #[test]
    fn graph_round_trips() {
        let (graph, _) = sample();
        assert!(graph.edge_count() > 30);
        let bytes = encode_graph(&graph);
        let decoded = decode_graph(&bytes).unwrap();
        assert_eq!(decoded.value.parts(), graph.parts());
        assert!(decoded.deps.is_empty());
        // 同じ入力から同じバイト列
        assert_eq!(bytes, encode_graph(&decoded.value));
        // 節の件数がPARAMSと一致する
        let file = BinFile::parse(&bytes).unwrap();
        file.expect(*b"TKRGRAPH", 1).unwrap();
        assert_eq!(file.section_counts()["EDGE"], graph.edge_count() as u64);
        assert_eq!(file.section_counts()["NODELL"], graph.node_count() as u64);
        assert_eq!(file.section_counts()["ADJ"], 2 * graph.edge_count() as u64);
    }

    #[test]
    fn graph_decode_rejects_inconsistent_files() {
        let (graph, _) = sample();
        // マジックナンバーの違い
        let mut bytes = encode_graph(&graph);
        bytes[0] ^= 1;
        assert!(decode_graph(&bytes).is_err());
        // PARAMSの件数と節の件数の矛盾
        let mut parts = graph.parts().clone();
        parts.params["edges"] = json!(graph.edge_count() + 1);
        assert!(decode_graph_parts(&encode_graph_parts(&parts)).is_err());
        let mut parts = graph.parts().clone();
        parts.params = json!({});
        assert!(decode_graph_parts(&encode_graph_parts(&parts)).is_err());
        // 検査（Graph::from_parts）に通らない中身
        let mut parts = graph.parts().clone();
        parts.edges[0].length_dm = 0;
        assert!(decode_graph(&encode_graph_parts(&parts)).is_err());
        // 切り詰め
        let bytes = encode_graph(&graph);
        assert!(decode_graph(&bytes[..bytes.len() - 8]).is_err());
        assert!(decode_graph(&[]).is_err());
    }

    #[test]
    fn restrictions_round_trip() {
        let mut b = GraphBuilder::new();
        let n: Vec<u32> = (0..5)
            .map(|i| b.add_node_deg(35.0, 139.0 + f64::from(i) * 0.001))
            .collect();
        for w in n.windows(2) {
            b.add_edge(EdgeSpec::new(w[0], w[1]));
        }
        b.forbid_nodes(&[n[0], n[1], n[2]], 4);
        b.forbid_nodes(&[n[1], n[2], n[3], n[4]], 2);
        b.forbid_nodes(&[n[0], n[1], n[2]], 2);
        let (graph, restrictions) = b.build_all();
        assert_eq!(restrictions.items.len(), 2);
        let deps = deps_of(b"graph");
        let bytes = encode_restrictions(&restrictions, &deps);
        let decoded = decode_restrictions(&bytes).unwrap();
        assert_eq!(decoded.value, restrictions);
        assert_eq!(decoded.deps, deps);
        decoded.value.validate(&graph).unwrap();
        // 空でも往復できる
        let empty = Restrictions::empty();
        let decoded = decode_restrictions(&encode_restrictions(&empty, &deps)).unwrap();
        assert!(decoded.value.items.is_empty());
        // DEPSにgraph.binがなければ拒否する
        assert!(decode_restrictions(&encode_restrictions(&restrictions, &Deps::new())).is_err());
    }

    #[test]
    fn restrictions_decode_rejects_malformed_sections() {
        let item = |seq: &[u32], modes: u8, kind: u8| Restriction {
            seq: seq.iter().map(|&d| DirEdge(d)).collect(),
            modes,
            kind,
        };
        let deps = deps_of(b"graph");
        for items in [
            vec![item(&[0, 2], 0, 0)], // modes が0
            vec![item(&[0, 2], 8, 0)], // modes に未知のビット
            vec![item(&[0, 2], 4, 2)], // kind が不正
        ] {
            let r = Restrictions {
                params: Value::Object(Map::new()),
                items,
            };
            assert!(decode_restrictions(&encode_restrictions(&r, &deps)).is_err());
        }
        // PARAMSのitemsとRESTRの件数の矛盾
        let r = Restrictions {
            params: json!({"items": 5}),
            items: vec![item(&[0, 2], 4, 0)],
        };
        assert!(decode_restrictions(&encode_restrictions(&r, &deps)).is_err());
    }

    #[test]
    fn topology_round_trips_and_odindex_round_trips() {
        let (graph, _) = sample();
        let attrs = {
            let e = graph.edge_count();
            GraphAttrs {
                params: json!({"nodes": graph.node_count(), "edges": e}),
                way_id: (0..e as i64).collect(),
                node_id: (0..graph.node_count() as i64).collect(),
                ref_idx: vec![0; e],
                name_idx: vec![0; e],
                strtab: vec![0],
                lanes: vec![0; e],
                width_dm: vec![0; e],
                layer: vec![0; e],
            }
        };
        let graph_bytes = encode_graph(&graph);
        let deps = deps_of(&graph_bytes);
        let config = TopologyConfig {
            scc_min_nodes: 2,
            ..TopologyConfig::default()
        };
        let topology = build_topology(graph.parts(), &attrs, &config).unwrap();
        topology.validate(&graph).unwrap();
        let bytes = encode_topology(&topology, &deps);
        let decoded = decode_topology(&bytes).unwrap();
        assert_eq!(decoded.value, topology);
        assert_eq!(decoded.deps, deps);
        decoded.value.validate(&graph).unwrap();
        assert!(topology.cluster_count() > 0);

        let index = build_odindex(graph.parts());
        let bytes = encode_odindex(&index, &deps);
        let decoded = decode_odindex(&bytes).unwrap();
        assert_eq!(decoded.value, index);
        assert_eq!(decoded.deps, deps);
        assert!(decoded.value.raw_params["edges"] == json!(graph.edge_count()));
        // 型付きのparamsを変えると、書き出すPARAMSに反映される
        let mut changed = index.clone();
        changed.params.rows += 1;
        let decoded = decode_odindex(&encode_odindex(&changed, &deps)).unwrap();
        assert_eq!(decoded.value.params.rows, index.params.rows + 1);
        assert_eq!(
            decoded.value.raw_params["rows"],
            json!(index.params.rows + 1)
        );
    }

    #[test]
    fn topology_and_odindex_decode_reject_inconsistent_sections() {
        let deps = deps_of(b"graph");
        let base = Topology {
            params: json!({"nodes": 4, "edges": 70}),
            snap_car: vec![0, 0],
            snap_bike: vec![0, 0],
            snap_walk: vec![0, 0],
            node_cluster: vec![NONE_U32, 0, 0, NONE_U32],
            clusters: vec![ClusterRecord {
                lat_e7: 1,
                lon_e7: 2,
                node_start: 0,
                node_count: 2,
                top_class: 4,
                branches: 3,
                flags: 1,
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
        };
        assert_eq!(
            decode_topology(&encode_topology(&base, &deps))
                .unwrap()
                .value,
            base
        );
        let rejects: Vec<(&str, Topology)> = vec![
            (
                "SNAP*の長さがedgesと矛盾",
                Topology {
                    snap_car: vec![0],
                    snap_bike: vec![0],
                    snap_walk: vec![0],
                    ..base.clone()
                },
            ),
            (
                "CLSNODEとNODECLSの不一致",
                Topology {
                    node_cluster: vec![NONE_U32, 0, NONE_U32, NONE_U32],
                    ..base.clone()
                },
            ),
            (
                "NODECLSの値がクラスタ数以上",
                Topology {
                    node_cluster: vec![NONE_U32, 0, 1, NONE_U32],
                    ..base.clone()
                },
            ),
            (
                "TWINに逆向きがない",
                Topology {
                    twins: vec![TwinPair { a: 3, b: 5 }],
                    ..base.clone()
                },
            ),
            (
                "TWINの複数のbの片方に逆向きがない",
                Topology {
                    twins: vec![
                        TwinPair { a: 3, b: 5 },
                        TwinPair { a: 3, b: 6 },
                        TwinPair { a: 5, b: 3 },
                    ],
                    ..base.clone()
                },
            ),
            (
                "TWINが昇順でない",
                Topology {
                    twins: vec![TwinPair { a: 5, b: 3 }, TwinPair { a: 3, b: 5 }],
                    ..base.clone()
                },
            ),
            (
                "TWINの同じaでbが昇順でない",
                Topology {
                    twins: vec![
                        TwinPair { a: 3, b: 6 },
                        TwinPair { a: 3, b: 5 },
                        TwinPair { a: 5, b: 3 },
                        TwinPair { a: 6, b: 3 },
                    ],
                    ..base.clone()
                },
            ),
            (
                "TWINの同じ組が重複",
                Topology {
                    twins: vec![
                        TwinPair { a: 3, b: 5 },
                        TwinPair { a: 3, b: 5 },
                        TwinPair { a: 5, b: 3 },
                        TwinPair { a: 5, b: 3 },
                    ],
                    ..base.clone()
                },
            ),
            (
                "TWINの辺番号が範囲外",
                Topology {
                    twins: vec![TwinPair { a: 70, b: 5 }, TwinPair { a: 5, b: 70 }],
                    ..base.clone()
                },
            ),
            (
                "SWPARENTが昇順でない",
                Topology {
                    sidewalk_parents: vec![
                        SidewalkParent {
                            sidewalk: 9,
                            parent: 4,
                        },
                        SidewalkParent {
                            sidewalk: 2,
                            parent: 1,
                        },
                    ],
                    ..base.clone()
                },
            ),
            (
                "CLSNODEがクラスタ内で昇順でない",
                Topology {
                    cluster_nodes: vec![2, 1],
                    ..base.clone()
                },
            ),
        ];
        for (what, topology) in rejects {
            assert!(
                decode_topology(&encode_topology(&topology, &deps)).is_err(),
                "{what}"
            );
        }

        let index = OdIndex {
            params: OdIndexParams {
                lat0_e7: 200_000_000,
                lon0_e7: 1_220_000_000,
                cell_lat_e7: 25_000,
                cell_lon_e7: 25_000,
                rows: 10,
                cols: 10,
            },
            raw_params: Value::Null,
            cell_keys: vec![0, 11],
            cell_ofs: vec![0, 1, 3],
            cell_edges: vec![4, 5, 9],
        };
        assert_eq!(
            decode_odindex(&encode_odindex(&index, &deps))
                .unwrap()
                .value
                .cell_edges,
            index.cell_edges
        );
        let rejects: Vec<(&str, OdIndex)> = vec![
            (
                "CELLKEYが昇順でない",
                OdIndex {
                    cell_keys: vec![11, 0],
                    ..index.clone()
                },
            ),
            (
                "CELLKEYが格子の外",
                OdIndex {
                    cell_keys: vec![0, 100],
                    ..index.clone()
                },
            ),
            (
                "CELLOFSの末尾が件数と違う",
                OdIndex {
                    cell_ofs: vec![0, 1, 2],
                    ..index.clone()
                },
            ),
            (
                "空のセル",
                OdIndex {
                    cell_ofs: vec![0, 0, 3],
                    ..index.clone()
                },
            ),
            (
                "CELLEDGがセル内で昇順でない",
                OdIndex {
                    cell_edges: vec![4, 9, 5],
                    ..index.clone()
                },
            ),
        ];
        for (what, bad) in rejects {
            assert!(
                decode_odindex(&encode_odindex(&bad, &deps)).is_err(),
                "{what}"
            );
        }
    }

    #[test]
    fn graph_attrs_round_trip() {
        let attrs = GraphAttrs {
            params: json!({"nodes": 2, "edges": 3}),
            way_id: vec![10, 10, 20],
            node_id: vec![5, 9],
            ref_idx: vec![0, 1, 1],
            name_idx: vec![0, 0, 4],
            strtab: b"\0R1\0name\0".to_vec(),
            lanes: vec![0, 2, 4],
            width_dm: vec![0, 55, 0],
            layer: vec![0, -1, 1],
        };
        let deps = deps_of(b"graph");
        let bytes = encode_graph_attrs(&attrs, &deps);
        let decoded = decode_graph_attrs(&bytes).unwrap();
        assert_eq!(decoded.value, attrs);
        assert_eq!(decoded.deps, deps);
        assert_eq!(decoded.value.string(1), "R1");
        assert_eq!(decoded.value.string(4), "name");
        // 範囲外の文字列表の添字、昇順でないway ID
        let mut bad = attrs.clone();
        bad.ref_idx[1] = 99;
        assert!(decode_graph_attrs(&encode_graph_attrs(&bad, &deps)).is_err());
        let mut bad = attrs.clone();
        bad.way_id = vec![20, 10, 10];
        assert!(decode_graph_attrs(&encode_graph_attrs(&bad, &deps)).is_err());
    }

    // --- assignment.bin ---

    fn sample_assignment() -> Assignment {
        use crate::domain::routing::accident::{
            AssignTarget, TARGET_MIRROR, UnassignedReason, UnassignedRecord,
        };
        let edge = |target, pos_dm, weight, flags| AssignTarget {
            target,
            pos_dm,
            weight,
            kind: 0,
            flags,
            reserved: 0,
        };
        Assignment {
            params: json!({"accidents": 4, "targets": 5, "unassigned": 1, "edges": 6, "clusters": 3,
                           "sigma0_m": 3.5}),
            offsets: vec![0, 2, 2, 4, 5],
            targets: vec![
                edge(3, 50, 0.6, 0),
                edge(5, 20, 0.4, 0),
                edge(3, 10, 1.0, 0),
                edge(4, 30, 1.0, TARGET_MIRROR),
                AssignTarget {
                    target: 2,
                    pos_dm: u32::MAX,
                    weight: 0.9,
                    kind: 1,
                    flags: 0,
                    reserved: 0,
                },
            ],
            unassigned: vec![UnassignedRecord {
                accident: 1,
                reason: UnassignedReason::MotorwayCodeMismatch,
            }],
        }
    }

    fn assignment_deps() -> Deps {
        required_deps("assignment.bin")
            .iter()
            .map(|file| ((*file).to_owned(), sha256_hex(file.as_bytes())))
            .collect()
    }

    #[test]
    fn assignment_round_trips_with_the_documented_layout() {
        let assignment = sample_assignment();
        let deps = assignment_deps();
        let bytes = encode_assignment(&assignment, &deps);
        let decoded = decode_assignment(&bytes).unwrap();
        assert_eq!(decoded.value, assignment);
        assert_eq!(decoded.deps, deps);
        assert_eq!(bytes, encode_assignment(&decoded.value, &decoded.deps));
        let file = BinFile::parse(&bytes).unwrap();
        file.expect(*b"TKRASSGN", 1).unwrap();
        let counts = file.section_counts();
        assert_eq!(
            (counts["ACCOFS"], counts["TARGET"], counts["UNASSGN"]),
            (5, 5, 1)
        );
        // 形式書13章：TARGETは16バイト、UNASSGNは8バイト。先頭の要素の並びを確かめる
        let target = file.require("TARGET").unwrap();
        assert_eq!(target.elem_size, 16);
        assert_eq!(&target.bytes[0..4], &3u32.to_le_bytes());
        assert_eq!(&target.bytes[4..8], &50u32.to_le_bytes());
        assert_eq!(&target.bytes[8..12], &0.6f32.to_le_bytes());
        assert_eq!(&target.bytes[12..16], &[0, 0, 0, 0]);
        assert_eq!(target.bytes[16 * 3 + 13], 1, "4つ目はMIRROR");
        let unassigned = file.require("UNASSGN").unwrap();
        assert_eq!(unassigned.elem_size, 8);
        assert_eq!(&unassigned.bytes[0..4], &1u32.to_le_bytes());
        assert_eq!(&unassigned.bytes[4..8], &[2, 0, 0, 0]);
    }

    #[test]
    fn assignment_decode_rejects_broken_files() {
        let deps = assignment_deps();
        let good = encode_assignment(&sample_assignment(), &deps);
        assert!(decode_assignment(&good).is_ok());
        let section_offset = |name: &str| {
            let file = BinFile::parse(&good).unwrap();
            file.require(name).unwrap().bytes.as_ptr() as usize - good.as_ptr() as usize
        };
        // マジックナンバー、切り詰め
        let mut bytes = good.clone();
        bytes[0] ^= 1;
        assert!(decode_assignment(&bytes).is_err());
        assert!(decode_assignment(&good[..good.len() - 8]).is_err());
        assert!(decode_assignment(&[]).is_err());
        // DEPSの必須キー
        let mut fewer = deps.clone();
        fewer.remove("topology.bin");
        assert!(decode_assignment(&encode_assignment(&sample_assignment(), &fewer)).is_err());
        // 他の章のファイルは読めない
        assert!(decode_topology(&good).is_err());
        // reasonの値、padが0でない
        let mut bytes = good.clone();
        bytes[section_offset("UNASSGN") + 4] = 7;
        assert!(decode_assignment(&bytes).is_err());
        let mut bytes = good.clone();
        bytes[section_offset("UNASSGN") + 6] = 1;
        assert!(decode_assignment(&bytes).is_err());
        // 重みが0.1未満、MIRRORの定義外のflags
        let mut bytes = good.clone();
        bytes[section_offset("TARGET") + 8..section_offset("TARGET") + 12]
            .copy_from_slice(&0.05f32.to_le_bytes());
        assert!(decode_assignment(&bytes).is_err());
        let mut bytes = good.clone();
        bytes[section_offset("TARGET") + 13] = 4;
        assert!(decode_assignment(&bytes).is_err());
        // PARAMSの件数との矛盾
        for key in ["accidents", "targets", "unassigned"] {
            let mut a = sample_assignment();
            a.params[key] = json!(99);
            assert!(
                decode_assignment(&encode_assignment(&a, &deps)).is_err(),
                "{key}"
            );
        }
        // 辺・クラスタの番号の範囲（PARAMSのedges・clusters）
        let mut a = sample_assignment();
        a.params["edges"] = json!(5);
        assert!(decode_assignment(&encode_assignment(&a, &deps)).is_err());
        let mut a = sample_assignment();
        a.params["clusters"] = json!(2);
        assert!(decode_assignment(&encode_assignment(&a, &deps)).is_err());
        // 割り当てのない事故がUNASSGNにない
        let mut a = sample_assignment();
        a.unassigned.clear();
        assert!(decode_assignment(&encode_assignment(&a, &deps)).is_err());
    }

    // --- alt.bin ---

    fn alt_car_table(nodes: Vec<u32>, from: Vec<u16>, to: Option<Vec<u16>>) -> Landmarks {
        use crate::domain::routing::search::alt::ModeLandmarks;

        let mut tables: [Option<ModeLandmarks>; 3] = [None, None, None];
        tables[2] = Some(ModeLandmarks { nodes, from, to });
        Landmarks::from_tables(40, tables)
    }

    #[test]
    fn alt_round_trips_with_no_table_car_only_and_to() {
        let deps = deps_of(b"graph");
        let cases = [
            ("表なし", Landmarks::empty()),
            ("carだけ", alt_car_table(vec![1], vec![0, 1, 2, 3], None)),
            (
                "toあり",
                alt_car_table(
                    vec![0, 3],
                    vec![0, 1, 2, 3, 4, 5, 6, 7],
                    Some(vec![7, 6, 5, 4, 3, 2, 1, 0]),
                ),
            ),
        ];
        for (what, mut landmarks) in cases {
            // `AltParams` に入らないキーも `PARAMS` に残る
            if let Value::Object(params) = &mut landmarks.raw_params {
                params.insert(
                    "selection".to_owned(),
                    json!({"method": "farthest", "start_nodes": {"car": 0}}),
                );
            }
            let bytes = encode_alt(&landmarks, &deps);
            let decoded = decode_alt(&bytes).unwrap();
            assert_eq!(decoded.value.params, landmarks.params, "{what}");
            assert_eq!(decoded.value.tables, landmarks.tables, "{what}");
            assert_eq!(decoded.deps, deps, "{what}");
            assert_eq!(bytes, encode_alt(&decoded.value, &decoded.deps), "{what}");
            // 型付きの `AltParams` と、それに入らないキーの両方が `PARAMS` にある
            assert_eq!(
                decoded.value.raw_params["selection"], landmarks.raw_params["selection"],
                "{what}"
            );
            assert_eq!(
                decoded.value.raw_params["unit_ds"],
                json!(landmarks.params.unit_ds),
                "{what}"
            );

            let file = BinFile::parse(&bytes).unwrap();
            file.expect(*b"TKRALT__", 1).unwrap();
            let counts = file.section_counts();
            match what {
                "表なし" => {
                    assert!(!counts.contains_key("LM_CAR"), "{what}");
                    assert!(!counts.contains_key("FR_CAR"), "{what}");
                }
                "carだけ" => {
                    assert_eq!(counts["LM_CAR"], 1, "{what}");
                    assert_eq!(counts["FR_CAR"], 4, "{what}");
                    assert!(!counts.contains_key("TO_CAR"), "{what}");
                    assert!(!counts.contains_key("LM_BIK"), "{what}");
                }
                "toあり" => {
                    assert_eq!(counts["LM_CAR"], 2, "{what}");
                    assert_eq!(counts["FR_CAR"], 8, "{what}");
                    assert_eq!(counts["TO_CAR"], 8, "{what}");
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn alt_decode_rejects_inconsistent_sections_and_params() {
        use crate::domain::routing::search::alt::ModeLandmarks;

        let deps = deps_of(b"graph");
        let good = alt_car_table(
            vec![0, 3],
            vec![0, 1, 2, 3, 4, 5, 6, 7],
            Some(vec![7, 6, 5, 4, 3, 2, 1, 0]),
        );
        assert!(decode_alt(&encode_alt(&good, &deps)).is_ok());

        // DEPSにgraph.binがない
        assert!(decode_alt(&encode_alt(&good, &Deps::new())).is_err());

        // unit_dsが0
        let mut bad = good.clone();
        bad.params.unit_ds = 0;
        assert!(decode_alt(&encode_alt(&bad, &deps)).is_err());

        // PARAMSにcarがあるのに節がない
        let mut bad = good.clone();
        bad.tables[2] = None;
        assert!(decode_alt(&encode_alt(&bad, &deps)).is_err());

        // directionsにtoがないのにTO_CARがある
        let mut bad = good.clone();
        bad.params
            .directions
            .insert("car".to_owned(), vec!["from".to_owned()]);
        assert!(decode_alt(&encode_alt(&bad, &deps)).is_err());

        // FR_CARの件数がLの倍数でない
        let bad = alt_car_table(vec![0, 3], vec![0, 1, 2], None);
        assert!(decode_alt(&encode_alt(&bad, &deps)).is_err());

        // 手段ごとにNが違う
        let mut tables: [Option<ModeLandmarks>; 3] = [None, None, None];
        tables[1] = Some(ModeLandmarks {
            nodes: vec![1],
            from: vec![0, 1, 2],
            to: None,
        });
        tables[2] = good.tables[2].clone();
        let bad = Landmarks::from_tables(40, tables);
        assert!(decode_alt(&encode_alt(&bad, &deps)).is_err());

        // TO_CARの件数がFR_CARと違う
        let mut table = good.tables[2].clone().unwrap();
        table.to = Some(vec![0, 1, 2]);
        let mut tables = [None, None, None];
        tables[2] = Some(table);
        let bad = Landmarks::from_tables(40, tables);
        assert!(decode_alt(&encode_alt(&bad, &deps)).is_err());

        // 表を持つ手段がPARAMSにない（directionsだけある）
        let mut bad = Landmarks::empty();
        bad.params
            .directions
            .insert("car".to_owned(), vec!["from".to_owned()]);
        assert!(decode_alt(&encode_alt(&bad, &deps)).is_err());
    }

    // --- risk.bin ---

    fn sample_risk_model() -> RiskModel {
        use crate::domain::routing::graph::time::PerMode;
        use crate::domain::routing::risk::table_index;
        use crate::domain::routing::risk::{WeatherMultiplier, road_class_k};

        let mut k_table = vec![CoefRecord::default(); TABLE_LEN];
        let mut j_table = vec![CoefRecord::default(); TABLE_LEN];
        let k = road_class_k(6, 1, true);
        k_table[table_index(Mode::Car, k, 0)] = CoefRecord {
            r: 1.25e-5,
            s: 12.5,
            a: 0.75,
            rho: [0.1, 0.2, 0.3, 0.15, 0.25],
            pad: 0.0,
        };
        j_table[table_index(Mode::Car, 35, 7)] = CoefRecord {
            r: 0.0125,
            s: 21.0,
            a: 0.5,
            rho: [0.05, 0.1, 0.4, 0.2, 0.25],
            pad: 0.0,
        };
        RiskModel {
            params: RiskParams {
                train_source_years: vec![2019, 2020, 2021, 2022],
                years: 4,
                c: 1.5,
                c_groups: vec![5, 6, 7, 8],
                car_exposure: None,
                beta: PerMode {
                    walk: 0.1,
                    bike: 0.2,
                    car: 0.3,
                },
                beta_cluster: PerMode {
                    walk: 0.4,
                    bike: 0.5,
                    car: 0.6,
                },
                weather_multiplier: WeatherMultiplier {
                    rain: 2.0,
                    fog: 4.0,
                    snow: 4.0,
                },
                urban: json!({"cell_deg": 0.005}),
                pooling: json!({"version": 1}),
                fit: json!({"deviance": 1.25}),
            },
            raw_params: json!({"custom": "keep", "years": 999}),
            edge_k: vec![k, 0xFE, 0xFF],
            cluster_j: vec![35],
            k_table,
            j_table,
        }
    }

    fn risk_deps() -> Deps {
        required_deps("risk.bin")
            .iter()
            .map(|file| ((*file).to_owned(), sha256_hex(file.as_bytes())))
            .collect()
    }

    #[test]
    fn risk_round_trips_with_the_documented_layout() {
        let model = sample_risk_model();
        let deps = risk_deps();
        let bytes = encode_risk(&model, &deps);
        let decoded = decode_risk(&bytes).unwrap();
        assert_eq!(decoded.value.params, model.params);
        assert_eq!(decoded.value.edge_k, model.edge_k);
        assert_eq!(decoded.value.cluster_j, model.cluster_j);
        assert_eq!(decoded.value.k_table, model.k_table);
        assert_eq!(decoded.value.j_table, model.j_table);
        assert_eq!(decoded.deps, deps);
        // 型付きのparamsで上書きしつつ、paramsに入らないキーも保つ
        assert_eq!(decoded.value.raw_params["custom"], json!("keep"));
        assert_eq!(decoded.value.raw_params["years"], json!(4));
        assert_eq!(
            bytes,
            encode_risk(&decoded.value, &decoded.deps),
            "同じ値から同じバイト列"
        );
        let file = BinFile::parse(&bytes).unwrap();
        file.expect(*b"TKRRISK_", 1).unwrap();
        let counts = file.section_counts();
        assert_eq!(
            (
                counts["EDGEK"],
                counts["CLUSJ"],
                counts["KTAB"],
                counts["JTAB"]
            ),
            (3, 1, TABLE_LEN as u64, TABLE_LEN as u64)
        );
        // 形式書14.1節：CoefRecordは48バイトで、r・s・a・rho・padの順
        let k_section = file.require("KTAB").unwrap();
        assert_eq!(k_section.elem_size, 48);
        use crate::domain::routing::risk::{road_class_k, table_index};
        let index = table_index(Mode::Car, road_class_k(6, 1, true), 0);
        let record = &k_section.bytes[index * 48..(index + 1) * 48];
        assert_eq!(&record[0..8], &1.25e-5_f64.to_le_bytes());
        assert_eq!(&record[8..16], &12.5_f64.to_le_bytes());
        assert_eq!(&record[16..24], &0.75_f64.to_le_bytes());
        assert_eq!(&record[24..28], &0.1_f32.to_le_bytes());
        assert_eq!(&record[44..48], &0.0_f32.to_le_bytes());
    }

    #[test]
    fn risk_params_without_beta_cluster_use_beta() {
        let params: RiskParams = serde_json::from_value(json!({
            "train_source_years": [2019, 2020],
            "years": 2,
            "c": 1.25,
            "c_groups": [5, 6, 7, 8],
            "beta": {"walk": 0.1, "bike": 0.2, "car": 0.3},
            "weather_multiplier": {"rain": 2.0, "fog": 4.0, "snow": 4.0},
            "urban": {"cell_deg": 0.005},
            "pooling": {},
            "fit": {}
        }))
        .unwrap();
        assert_eq!(params.beta_cluster, params.beta);
    }

    #[test]
    fn risk_decode_rejects_broken_files() {
        let deps = risk_deps();
        let good = encode_risk(&sample_risk_model(), &deps);
        assert!(decode_risk(&good).is_ok());
        // マジックナンバー、切り詰め、空
        let mut bytes = good.clone();
        bytes[0] ^= 1;
        assert!(decode_risk(&bytes).is_err());
        assert!(decode_risk(&good[..good.len() - 8]).is_err());
        assert!(decode_risk(&[]).is_err());
        // DEPSの必須キー
        let mut fewer = deps.clone();
        fewer.remove("assignment.bin");
        assert!(decode_risk(&encode_risk(&sample_risk_model(), &fewer)).is_err());
        // KTAB・JTABの件数がTABLE_LENでない
        let mut model = sample_risk_model();
        model.k_table.pop();
        assert!(decode_risk(&encode_risk(&model, &deps)).is_err());
        let mut model = sample_risk_model();
        model.j_table.pop();
        assert!(decode_risk(&encode_risk(&model, &deps)).is_err());
    }
}
