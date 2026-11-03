//! 行政界（`boundary=administrative`）のrelation・way・nodeをPBFから読む（M5.1）。
//!
//! 全国PBFを3回走査する。1回目でrelation、2回目で必要なwayのノードID列、3回目で必要なノードの
//! 座標を読む。全ノードの座標は保持せず、必要なIDだけを覚える。

use std::collections::BTreeMap;
use std::path::Path;

use osmpbf::{PrimitiveBlock, RelMemberType};

use crate::data::LoadError;
use crate::domain::routing::graph::Coord;

use super::scan_blocks;

/// PBFから読んだ `boundary=administrative` のrelation。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminRelation {
    pub id: i64,
    pub name: Option<String>,
    pub admin_level: Option<String>,
    /// roleが `outer` か空のway。
    pub outer_ways: Vec<i64>,
    /// roleが `inner` のway。
    pub inner_ways: Vec<i64>,
}

fn collect_admin_relations(block: &PrimitiveBlock) -> Vec<AdminRelation> {
    let mut out = Vec::new();
    for group in block.groups() {
        for relation in group.relations() {
            let mut boundary = false;
            let mut name = None;
            let mut admin_level = None;
            for (key, value) in relation.tags() {
                match key {
                    "boundary" if value == "administrative" => boundary = true,
                    "name" => name = Some(value.to_owned()),
                    "admin_level" => admin_level = Some(value.to_owned()),
                    _ => {}
                }
            }
            if !boundary {
                continue;
            }
            let mut outer_ways = Vec::new();
            let mut inner_ways = Vec::new();
            for member in relation.members() {
                if member.member_type != RelMemberType::Way {
                    continue;
                }
                match member.role().unwrap_or("") {
                    "" | "outer" => outer_ways.push(member.member_id),
                    "inner" => inner_ways.push(member.member_id),
                    _ => {}
                }
            }
            if outer_ways.is_empty() && inner_ways.is_empty() {
                continue;
            }
            out.push(AdminRelation {
                id: relation.id(),
                name,
                admin_level,
                outer_ways,
                inner_ways,
            });
        }
    }
    out
}

/// `boundary=administrative` のrelationを、IDの昇順で読む。
pub fn read_admin_relations(path: &Path) -> Result<Vec<AdminRelation>, LoadError> {
    let mut all: Vec<AdminRelation> = scan_blocks(path, collect_admin_relations)?
        .into_iter()
        .flatten()
        .collect();
    all.sort_by_key(|relation| relation.id);
    Ok(all)
}

fn collect_ways(block: &PrimitiveBlock, wanted: &[i64]) -> Vec<(i64, Vec<i64>)> {
    let mut out = Vec::new();
    for group in block.groups() {
        for way in group.ways() {
            let id = way.id();
            if wanted.binary_search(&id).is_ok() {
                out.push((id, way.refs().collect()));
            }
        }
    }
    out
}

/// `ids` のwayのノードID列を、way IDの昇順の表で返す。
pub fn read_ways_by_id(path: &Path, ids: &[i64]) -> Result<BTreeMap<i64, Vec<i64>>, LoadError> {
    let mut wanted = ids.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    let chunks = scan_blocks(path, |block| collect_ways(block, &wanted))?;
    let mut ways = BTreeMap::new();
    for (id, nodes) in chunks.into_iter().flatten() {
        ways.insert(id, nodes);
    }
    Ok(ways)
}

fn collect_nodes(block: &PrimitiveBlock, wanted: &[i64]) -> Vec<(i64, i32, i32)> {
    let mut out = Vec::new();
    for group in block.groups() {
        for node in group.dense_nodes() {
            let id = node.id();
            if wanted.binary_search(&id).is_ok() {
                out.push((id, node.decimicro_lat(), node.decimicro_lon()));
            }
        }
        for node in group.nodes() {
            let id = node.id();
            if wanted.binary_search(&id).is_ok() {
                out.push((id, node.decimicro_lat(), node.decimicro_lon()));
            }
        }
    }
    out
}

/// `ids` のノードの座標を、ノードIDの昇順の表で返す。
pub fn read_node_coords(path: &Path, ids: &[i64]) -> Result<BTreeMap<i64, Coord>, LoadError> {
    let mut wanted = ids.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    let chunks = scan_blocks(path, |block| collect_nodes(block, &wanted))?;
    let mut coords = BTreeMap::new();
    for (id, lat, lon) in chunks.into_iter().flatten() {
        coords.insert(id, Coord::new(lat, lon));
    }
    Ok(coords)
}
