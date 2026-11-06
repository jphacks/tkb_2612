//! OSM PBFの読み取り（担当：②、形式書5〜9章の入力）。
//!
//! P1（`osm-graph`）は、PBFを3回読む。
//!
//! 1. wayを読み、通行規則（`graph::rules`）でグラフに入れるwayを選ぶ（`read_ways`）。
//! 2. 選んだwayが参照するノードの座標とタグを読む。同じ走査で、右左折禁止のrelationも読む
//!    （`read_nodes_and_restrictions`）。
//! 3. 表から `prep::graph_build::build_graph` が辺とCSRを作る。
//!
//! PBFのブロックを `rayon` で並列に復号する。結果の順番は、ブロックの番号（ファイル内の順）で整える
//! ので、スレッド数に依存しない。wayは最後にIDの昇順に整える。

pub mod admin;

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use osmpbf::{BlobDecode, BlobReader, PrimitiveBlock};
use rayon::prelude::*;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::data::LoadError;
use crate::domain::routing::graph::rules::{classify_node, classify_way_detailed};
use crate::domain::routing::prep::attrs::{parse_lanes, parse_layer, parse_width_dm};
use crate::domain::routing::prep::graph_build::{
    BuiltGraph, ExclusionCounts, NODE_FORCE, NODE_MISSING, NodeTable, WayMeta, WayTable,
    build_graph,
};
use crate::domain::routing::prep::restrict::{MemberKind, RelationMember, RestrictionRelation};

fn osm_error(e: osmpbf::Error) -> LoadError {
    LoadError::Invalid(format!("PBFを読めません: {e}"))
}

/// PBFファイルの識別子（`graph.bin` の `PARAMS` と `metadata.json` の `routing.inputs` に記録する）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PbfInfo {
    pub file_name: String,
    pub bytes: u64,
    pub sha256: String,
    /// ヘッダーの `osmosis_replication_timestamp`（Unix秒）。
    pub replication_timestamp: Option<i64>,
    pub writing_program: Option<String>,
}

impl PbfInfo {
    pub fn to_json(&self) -> Value {
        json!({
            "file": self.file_name,
            "bytes": self.bytes,
            "sha256": self.sha256,
            "osmosis_replication_timestamp": self.replication_timestamp,
            "writing_program": self.writing_program,
        })
    }
}

/// ファイル全体のSHA-256（小文字hex）。
pub fn sha256_file(path: &Path) -> Result<String, LoadError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 8 << 20];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// ヘッダーとファイルのハッシュを読む。
pub fn pbf_info(path: &Path) -> Result<PbfInfo, LoadError> {
    let bytes = std::fs::metadata(path)?.len();
    let mut replication_timestamp = None;
    let mut writing_program = None;
    let reader = BlobReader::from_path(path).map_err(osm_error)?;
    for blob in reader {
        let blob = blob.map_err(osm_error)?;
        if let BlobDecode::OsmHeader(header) = blob.decode().map_err(osm_error)? {
            replication_timestamp = header.osmosis_replication_timestamp();
            writing_program = header.writing_program().map(str::to_owned);
            break;
        }
    }
    Ok(PbfInfo {
        file_name: path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
        bytes,
        sha256: sha256_file(path)?,
        replication_timestamp,
        writing_program,
    })
}

/// すべてのデータブロックに `f` を並列に適用して、ファイル内の順に並べて返す。
fn scan_blocks<T: Send>(
    path: &Path,
    f: impl Fn(&PrimitiveBlock) -> T + Sync + Send,
) -> Result<Vec<T>, LoadError> {
    let reader = BlobReader::from_path(path).map_err(osm_error)?;
    let results: Result<Vec<Option<(usize, T)>>, osmpbf::Error> = reader
        .enumerate()
        .par_bridge()
        .map(|(i, blob)| {
            let blob = blob?;
            match blob.decode()? {
                BlobDecode::OsmData(block) => Ok(Some((i, f(&block)))),
                _ => Ok(None),
            }
        })
        .collect();
    let mut results: Vec<(usize, T)> = results.map_err(osm_error)?.into_iter().flatten().collect();
    results.sort_unstable_by_key(|(i, _)| *i);
    Ok(results.into_iter().map(|(_, t)| t).collect())
}

fn tag_value<'a>(tags: &[(&'a str, &'a str)], key: &str) -> Option<&'a str> {
    tags.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// 1ブロックのwayの読み取り結果。
#[derive(Default)]
struct WayChunk {
    id: Vec<i64>,
    meta: Vec<WayMeta>,
    ref_str: Vec<Option<Box<str>>>,
    name_str: Vec<Option<Box<str>>>,
    ref_ofs: Vec<u32>,
    refs: Vec<i64>,
    excluded: ExclusionCounts,
    ways_seen: u64,
}

fn collect_ways(block: &PrimitiveBlock) -> WayChunk {
    let mut chunk = WayChunk {
        ref_ofs: vec![0],
        ..WayChunk::default()
    };
    let mut tags: Vec<(&str, &str)> = Vec::new();
    for group in block.groups() {
        for way in group.ways() {
            chunk.ways_seen += 1;
            tags.clear();
            tags.extend(way.tags());
            match classify_way_detailed(&tags) {
                Err(reason) => chunk.excluded.add(reason),
                Ok(c) => {
                    chunk.id.push(way.id());
                    chunk.meta.push(WayMeta {
                        class: c.class.code(),
                        access: c.access,
                        flags: c.flags,
                        maxspeed_kmh: c.maxspeed_kmh,
                        lanes: tag_value(&tags, "lanes").map_or(0, parse_lanes),
                        width_dm: tag_value(&tags, "width").map_or(0, parse_width_dm),
                        layer: tag_value(&tags, "layer").map_or(0, parse_layer),
                    });
                    let non_empty = |key| tag_value(&tags, key).filter(|v| !v.is_empty());
                    chunk.ref_str.push(non_empty("ref").map(Box::from));
                    chunk.name_str.push(non_empty("name").map(Box::from));
                    chunk.refs.extend(way.refs());
                    chunk.ref_ofs.push(chunk.refs.len() as u32);
                }
            }
        }
    }
    chunk
}

/// 読んだwayの表（ノードはOSMノードIDのまま）。
pub struct RawWays {
    pub id: Vec<i64>,
    pub meta: Vec<WayMeta>,
    pub ref_str: Vec<Option<Box<str>>>,
    pub name_str: Vec<Option<Box<str>>>,
    pub ref_ofs: Vec<u32>,
    pub refs: Vec<i64>,
    pub excluded: ExclusionCounts,
    pub ways_seen: u64,
}

/// 1回目の走査：グラフに入れるwayを、IDの昇順で読む。
pub fn read_ways(path: &Path) -> Result<RawWays, LoadError> {
    let chunks = scan_blocks(path, collect_ways)?;
    let mut out = RawWays {
        id: Vec::new(),
        meta: Vec::new(),
        ref_str: Vec::new(),
        name_str: Vec::new(),
        ref_ofs: vec![0],
        refs: Vec::new(),
        excluded: ExclusionCounts::default(),
        ways_seen: 0,
    };
    let total_refs: usize = chunks.iter().map(|c| c.refs.len()).sum();
    let total_ways: usize = chunks.iter().map(|c| c.id.len()).sum();
    if total_refs >= u32::MAX as usize {
        return Err(LoadError::Invalid(
            "wayの構成ノードの総数がu32に収まりません".to_owned(),
        ));
    }
    out.id.reserve(total_ways);
    out.meta.reserve(total_ways);
    out.ref_str.reserve(total_ways);
    out.name_str.reserve(total_ways);
    out.ref_ofs.reserve(total_ways + 1);
    out.refs.reserve(total_refs);
    for chunk in chunks {
        let base = out.refs.len() as u32;
        out.id.extend(chunk.id);
        out.meta.extend(chunk.meta);
        out.ref_str.extend(chunk.ref_str);
        out.name_str.extend(chunk.name_str);
        out.ref_ofs
            .extend(chunk.ref_ofs[1..].iter().map(|&o| base + o));
        out.refs.extend(chunk.refs);
        out.excluded.merge(&chunk.excluded);
        out.ways_seen += chunk.ways_seen;
    }
    out.sort_by_way_id()?;
    Ok(out)
}

impl RawWays {
    /// way IDの昇順に整える（PBFが昇順なら何もしない）。IDの重複は失敗。
    fn sort_by_way_id(&mut self) -> Result<(), LoadError> {
        if self.id.windows(2).all(|w| w[0] < w[1]) {
            return Ok(());
        }
        let mut order: Vec<u32> = (0..self.id.len() as u32).collect();
        order.par_sort_unstable_by_key(|&i| self.id[i as usize]);
        if order
            .windows(2)
            .any(|w| self.id[w[0] as usize] == self.id[w[1] as usize])
        {
            return Err(LoadError::Invalid("PBFにway IDの重複があります".to_owned()));
        }
        let mut sorted = RawWays {
            id: Vec::with_capacity(self.id.len()),
            meta: Vec::with_capacity(self.id.len()),
            ref_str: Vec::with_capacity(self.id.len()),
            name_str: Vec::with_capacity(self.id.len()),
            ref_ofs: Vec::with_capacity(self.id.len() + 1),
            refs: Vec::with_capacity(self.refs.len()),
            excluded: self.excluded.clone(),
            ways_seen: self.ways_seen,
        };
        sorted.ref_ofs.push(0);
        for &i in &order {
            let i = i as usize;
            sorted.id.push(self.id[i]);
            sorted.meta.push(self.meta[i]);
            sorted.ref_str.push(self.ref_str[i].take());
            sorted.name_str.push(self.name_str[i].take());
            sorted.refs.extend_from_slice(
                &self.refs[self.ref_ofs[i] as usize..self.ref_ofs[i + 1] as usize],
            );
            sorted.ref_ofs.push(sorted.refs.len() as u32);
        }
        *self = sorted;
        Ok(())
    }
}

const COORD_MISSING: u64 = u64::MAX;

fn pack_coord(lat_e7: i32, lon_e7: i32) -> u64 {
    (u64::from(lat_e7 as u32) << 32) | u64::from(lon_e7 as u32)
}

/// 2回目の走査の、ブロックごとの出力（relation）。
fn collect_relations(block: &PrimitiveBlock) -> Vec<RestrictionRelation> {
    let mut out = Vec::new();
    for group in block.groups() {
        for relation in group.relations() {
            let mut tags: Vec<(String, String)> = Vec::new();
            let mut is_restriction = false;
            for (k, v) in relation.tags() {
                if k == "type" && (v == "restriction" || v.starts_with("restriction:")) {
                    is_restriction = true;
                }
                if k == "type" || k == "except" || k.starts_with("restriction") {
                    tags.push((k.to_owned(), v.to_owned()));
                }
            }
            if !is_restriction {
                continue;
            }
            let members = relation
                .members()
                .map(|m| RelationMember {
                    kind: match m.member_type {
                        osmpbf::RelMemberType::Node => MemberKind::Node,
                        osmpbf::RelMemberType::Way => MemberKind::Way,
                        osmpbf::RelMemberType::Relation => MemberKind::Relation,
                    },
                    id: m.member_id,
                    role: m.role().unwrap_or("").to_owned(),
                })
                .collect();
            out.push(RestrictionRelation {
                id: relation.id(),
                tags,
                members,
            });
        }
    }
    out
}

/// 右左折禁止のrelation（`type=restriction*`）を、relation IDの昇順で読む。P2が使う。
pub fn read_restriction_relations(path: &Path) -> Result<Vec<RestrictionRelation>, LoadError> {
    let mut all: Vec<RestrictionRelation> = scan_blocks(path, collect_relations)?
        .into_iter()
        .flatten()
        .collect();
    all.sort_by_key(|r| r.id);
    Ok(all)
}

/// 2回目の走査：`ids`（昇順・重複なし）の各ノードの座標とタグ（`NODEFLG` のビット）を読む。
/// 同じ走査で、右左折禁止のrelationも読む。
pub fn read_nodes_and_restrictions(
    path: &Path,
    ids: Vec<i64>,
) -> Result<(NodeTable, Vec<RestrictionRelation>), LoadError> {
    let coords: Vec<AtomicU64> = (0..ids.len())
        .map(|_| AtomicU64::new(COORD_MISSING))
        .collect();
    let flags: Vec<AtomicU8> = (0..ids.len()).map(|_| AtomicU8::new(0)).collect();
    let relation_chunks = scan_blocks(path, |block| {
        let mut tags: Vec<(&str, &str)> = Vec::new();
        for group in block.groups() {
            for node in group.dense_nodes() {
                if let Ok(i) = ids.binary_search(&node.id()) {
                    coords[i].store(
                        pack_coord(node.decimicro_lat(), node.decimicro_lon()),
                        Ordering::Relaxed,
                    );
                    tags.clear();
                    tags.extend(node.tags());
                    if !tags.is_empty() {
                        flags[i].store(classify_node(&tags), Ordering::Relaxed);
                    }
                }
            }
            for node in group.nodes() {
                if let Ok(i) = ids.binary_search(&node.id()) {
                    coords[i].store(
                        pack_coord(node.decimicro_lat(), node.decimicro_lon()),
                        Ordering::Relaxed,
                    );
                    tags.clear();
                    tags.extend(node.tags());
                    if !tags.is_empty() {
                        flags[i].store(classify_node(&tags), Ordering::Relaxed);
                    }
                }
            }
        }
        collect_relations(block)
    })?;
    let mut relations: Vec<RestrictionRelation> = relation_chunks.into_iter().flatten().collect();
    relations.sort_by_key(|r| r.id);

    // viaノードは辺の分割点にする
    let mut flag_values: Vec<u8> = flags.into_iter().map(AtomicU8::into_inner).collect();
    for relation in &relations {
        for m in &relation.members {
            if m.kind == MemberKind::Node
                && m.role == "via"
                && let Ok(i) = ids.binary_search(&m.id)
            {
                flag_values[i] |= NODE_FORCE;
            }
        }
    }
    let mut lat_e7 = Vec::with_capacity(ids.len());
    let mut lon_e7 = Vec::with_capacity(ids.len());
    for c in coords {
        let packed = c.into_inner();
        if packed == COORD_MISSING {
            lat_e7.push(NODE_MISSING);
            lon_e7.push(NODE_MISSING);
        } else {
            lat_e7.push((packed >> 32) as u32 as i32);
            lon_e7.push(packed as u32 as i32);
        }
    }
    Ok((
        NodeTable {
            id: ids,
            lat_e7,
            lon_e7,
            flags: flag_values,
        },
        relations,
    ))
}

/// P1の入出力。
pub struct OsmGraph {
    pub built: BuiltGraph,
    pub relations: Vec<RestrictionRelation>,
    pub excluded: ExclusionCounts,
}

/// PBFから共通グラフを作る（P1）。`log` に進捗を書く。
pub fn osm_to_graph(
    path: &Path,
    info: &PbfInfo,
    log: &dyn Fn(&str),
) -> Result<OsmGraph, LoadError> {
    let start = std::time::Instant::now();
    let stamp = |what: &str| log(&format!("[{:>7.1}s] {what}", start.elapsed().as_secs_f64()));

    stamp("1/3 wayを読む");
    let raw = read_ways(path)?;
    stamp(&format!(
        "  way {}本を走査し、グラフに入れるwayは{}本（構成ノード{}）",
        raw.ways_seen,
        raw.id.len(),
        raw.refs.len()
    ));

    // 参照されるノードのID（昇順・重複なし）と、wayの構成ノードの添字
    let mut ids = raw.refs.clone();
    ids.par_sort_unstable();
    ids.dedup();
    ids.shrink_to_fit();
    let refs: Vec<u32> = raw
        .refs
        .par_iter()
        .map(|id| ids.binary_search(id).expect("idsに含まれる") as u32)
        .collect();
    stamp(&format!("  参照されるノードは{}個", ids.len()));

    stamp("2/3 ノードの座標とタグ、右左折禁止のrelationを読む");
    let (nodes, relations) = read_nodes_and_restrictions(path, ids)?;
    stamp(&format!("  右左折禁止のrelationは{}件", relations.len()));

    let RawWays {
        id,
        meta,
        ref_str,
        name_str,
        ref_ofs,
        excluded,
        ..
    } = raw;
    let ways = WayTable {
        id,
        meta,
        ref_str,
        name_str,
        ref_ofs,
        refs,
    };

    stamp("3/3 辺の分割とCSR");
    let mut extra = Map::new();
    extra.insert("osm".to_owned(), info.to_json());
    extra.insert("excluded_ways".to_owned(), excluded.to_json());
    let built = build_graph(&ways, &nodes, extra).map_err(LoadError::Invalid)?;
    stamp(&format!(
        "  ノード{}、辺{}、形状点{}",
        built.parts.node_ll.len(),
        built.parts.edges.len(),
        built.parts.shape.len()
    ));
    Ok(OsmGraph {
        built,
        relations,
        excluded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsorted_ways_are_ordered_by_id_and_duplicates_fail() {
        let way = |id: i64, refs: &[i64], name: &str| (id, refs.to_vec(), name.to_owned());
        let list = [
            way(30, &[3, 4, 5], "c"),
            way(10, &[1, 2], "a"),
            way(20, &[2, 3], ""),
        ];
        let build = |list: &[(i64, Vec<i64>, String)]| {
            let mut raw = RawWays {
                id: vec![],
                meta: vec![],
                ref_str: vec![],
                name_str: vec![],
                ref_ofs: vec![0],
                refs: vec![],
                excluded: ExclusionCounts::default(),
                ways_seen: 0,
            };
            for (id, refs, name) in list {
                raw.id.push(*id);
                raw.meta.push(WayMeta {
                    layer: *id as i8,
                    ..WayMeta::default()
                });
                raw.ref_str.push(None);
                raw.name_str
                    .push((!name.is_empty()).then(|| name.as_str().into()));
                raw.refs.extend(refs);
                raw.ref_ofs.push(raw.refs.len() as u32);
            }
            raw
        };
        let mut raw = build(&list);
        raw.sort_by_way_id().unwrap();
        assert_eq!(raw.id, vec![10, 20, 30]);
        assert_eq!(raw.refs, vec![1, 2, 2, 3, 3, 4, 5]);
        assert_eq!(raw.ref_ofs, vec![0, 2, 4, 7]);
        assert_eq!(
            raw.meta.iter().map(|m| m.layer).collect::<Vec<_>>(),
            vec![10, 20, 30]
        );
        assert_eq!(raw.name_str[0].as_deref(), Some("a"));
        assert_eq!(raw.name_str[1], None);
        assert_eq!(raw.name_str[2].as_deref(), Some("c"));
        // すでに整っていれば何もしない
        raw.sort_by_way_id().unwrap();
        assert_eq!(raw.id, vec![10, 20, 30]);
        // IDの重複は失敗
        let mut dup = build(&[
            way(5, &[1, 2], ""),
            way(4, &[2, 3], ""),
            way(5, &[3, 4], ""),
        ]);
        assert!(dup.sort_by_way_id().is_err());
    }

    #[test]
    fn coordinates_pack_and_unpack() {
        for (lat, lon) in [
            (350_000_000, 1_390_000_000),
            (-1, -1),
            (0, 0),
            (459_999_999, 1_539_999_999),
        ] {
            let packed = pack_coord(lat, lon);
            assert_eq!(
                ((packed >> 32) as u32 as i32, packed as u32 as i32),
                (lat, lon)
            );
        }
        // 「座標なし」の印と衝突するのは (-1, -1) だけで、日本の範囲には現れない
        assert_eq!(pack_coord(-1, -1), COORD_MISSING);
    }
}
