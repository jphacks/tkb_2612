//! P2：turn restrictionのrelationを、「続けて通ってはいけない有向辺の列」にする（形式書9章）。
//!
//! 入力はPBFから取り出したrelation（`RestrictionRelation`）と、P1の `graph.bin`・`graph_attrs.bin`。
//! via-node型は長さ2の列、via-way型は長さ3以上の列にする。`only_*` は、同じ意味の `no_*` の集まりに
//! 変換する。対象の手段は、`restriction=*` がcarとbike（`except` で外す）、`restriction:motorcar`・
//! `restriction:motor_vehicle` がcar、`restriction:bicycle` がbike。walkには適用しない。
//! `restriction:conditional` は初版では無視して件数だけ記録する。
//!
//! 曖昧さの扱い：
//!
//! - from・toのwayが、viaノードで終わらずに通り抜けていて、両側の辺がその手段で通れるときは、
//!   どちらの辺か決められないので、解決しない（`from_ambiguous`・`to_ambiguous`）。
//! - 対象の手段のどれも、fromの有向辺・toの有向辺・viaの列を通れないときは、禁止の意味がないので
//!   捨てる（`not_traversable`）。通れる手段が一部だけなら、その手段に絞る。
//! - `only_*` の変換先は、viaノードから出るすべての有向辺（通行の可否によらない）から to を除いたもの。

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::contract::Mode;
use crate::domain::routing::graph::restrictions::{Restriction, Restrictions};
use crate::domain::routing::graph::{DirEdge, GraphParts, NodeId};
use crate::domain::routing::prep::attrs::GraphAttrs;
use crate::domain::routing::prep::view::GraphView;

const WALK: u8 = 1;
const BIKE: u8 = 2;
const CAR: u8 = 4;

/// relationのメンバーの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberKind {
    Node,
    Way,
    Relation,
}

/// relationのメンバー。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationMember {
    pub kind: MemberKind,
    pub id: i64,
    pub role: String,
}

/// PBFから取り出した、`type=restriction*` のrelation。タグは `type`・`restriction*`・`except` だけを持つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestrictionRelation {
    pub id: i64,
    pub tags: Vec<(String, String)>,
    pub members: Vec<RelationMember>,
}

impl RestrictionRelation {
    fn tag(&self, key: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// `resolve` の結果。
#[derive(Debug)]
pub struct Resolved {
    pub restrictions: Restrictions,
    /// `restrictions.params` と同じ内容。
    pub audit: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Polarity {
    No,
    Only,
}

/// 辺の選び方の結果。
enum Pick {
    /// 通れる有向辺が1つ。通れる手段のビット付き。
    Unique(DirEdge, u8),
    /// 通れる手段がない。
    NotTraversable,
    /// 通れる有向辺が2つ以上で決められない。
    Ambiguous,
}

#[derive(Default)]
struct Stats {
    relations_total: u64,
    entries_total: u64,
    conditional_ignored: u64,
    other_vehicle_ignored: u64,
    relations_without_supported_tag: u64,
    resolved: BTreeMap<&'static str, u64>,
    unresolved: BTreeMap<&'static str, u64>,
    only_derived: u64,
    sequences_before_merge: u64,
}

impl Stats {
    fn fail(&mut self, reason: &'static str) {
        *self.unresolved.entry(reason).or_default() += 1;
    }
}

fn kind_key(via_way: bool, polarity: Polarity) -> &'static str {
    match (via_way, polarity) {
        (false, Polarity::No) => "via_node_no",
        (false, Polarity::Only) => "via_node_only",
        (true, Polarity::No) => "via_way_no",
        (true, Polarity::Only) => "via_way_only",
    }
}

/// `except` に書かれた手段を、外す手段のビットにする。
fn excluded_modes(except: Option<&str>) -> u8 {
    let mut out = 0;
    for part in except.unwrap_or("").split([';', ',']) {
        match part.trim() {
            "bicycle" => out |= BIKE,
            "motorcar" | "motor_vehicle" => out |= CAR,
            _ => {}
        }
    }
    out
}

/// relationから、（値、対象の手段）の組を取り出す。数えるだけのタグは `stats` に足す。
fn entries(rel: &RestrictionRelation, stats: &mut Stats) -> Vec<(String, u8)> {
    let excluded = excluded_modes(rel.tag("except"));
    let mut out = Vec::new();
    let mut other = false;
    for (key, value) in &rel.tags {
        let modes = match key.as_str() {
            "restriction" => CAR | BIKE,
            "restriction:motorcar" | "restriction:motor_vehicle" => CAR,
            "restriction:bicycle" => BIKE,
            "restriction:conditional" => {
                stats.conditional_ignored += 1;
                other = true;
                continue;
            }
            k if k.starts_with("restriction:") => {
                stats.other_vehicle_ignored += 1;
                other = true;
                continue;
            }
            _ => continue,
        };
        out.push((value.clone(), modes & !excluded & !WALK));
    }
    if out.is_empty() && other {
        stats.relations_without_supported_tag += 1;
    }
    out
}

/// 辺の列（有向辺）が、その手段で通れる手段のビットを `modes` と共通部分にする。
fn pick(view: &GraphView<'_>, candidates: &[DirEdge], modes: u8) -> Pick {
    let passable: Vec<(DirEdge, u8)> = candidates
        .iter()
        .filter_map(|&d| {
            let m = view.passable_modes(d, modes);
            (m != 0).then_some((d, m))
        })
        .collect();
    match passable.as_slice() {
        [] => Pick::NotTraversable,
        [(d, m)] => Pick::Unique(*d, *m),
        _ => Pick::Ambiguous,
    }
}

/// wayの辺のうち、ノード `g` に入る有向辺。
fn entering(view: &GraphView<'_>, edges: std::ops::Range<usize>, g: NodeId) -> Vec<DirEdge> {
    let mut out = Vec::new();
    for e in edges {
        let record = view.edge(e as u32);
        if record.head == g {
            out.push(DirEdge::new(e as u32, false));
        }
        if record.tail == g {
            out.push(DirEdge::new(e as u32, true));
        }
    }
    out
}

/// wayの辺のうち、ノード `g` から出る有向辺。
fn leaving(view: &GraphView<'_>, edges: std::ops::Range<usize>, g: NodeId) -> Vec<DirEdge> {
    entering(view, edges, g)
        .into_iter()
        .map(DirEdge::reversed)
        .collect()
}

/// 途中の辺の列に沿って、wayを `from` ノードから `to` ノードまで通る有向辺の列にする。
/// wayの辺がつながっていない、または `from`・`to` がwayの両端でなければ `None`。
fn via_path(
    view: &GraphView<'_>,
    edges: std::ops::Range<usize>,
    from: NodeId,
    to: NodeId,
) -> Option<Vec<DirEdge>> {
    let (first, last) = way_ends(view, edges.clone())?;
    if first == last {
        return None; // 環状のway。向きを決められない
    }
    if from == first && to == last {
        Some(edges.map(|e| DirEdge::new(e as u32, false)).collect())
    } else if from == last && to == first {
        Some(edges.rev().map(|e| DirEdge::new(e as u32, true)).collect())
    } else {
        None
    }
}

/// wayの両端のノード。辺が空、またはつながっていなければ `None`。
fn way_ends(view: &GraphView<'_>, edges: std::ops::Range<usize>) -> Option<(NodeId, NodeId)> {
    if edges.is_empty() {
        return None;
    }
    for e in edges.start..edges.end - 1 {
        if view.edge(e as u32).head != view.edge(e as u32 + 1).tail {
            return None;
        }
    }
    Some((
        view.edge(edges.start as u32).tail,
        view.edge(edges.end as u32 - 1).head,
    ))
}

/// wayのどこかの辺の端点になっているノード。
fn way_nodes(view: &GraphView<'_>, edges: std::ops::Range<usize>) -> Vec<NodeId> {
    let mut out: Vec<NodeId> = edges
        .flat_map(|e| {
            let r = view.edge(e as u32);
            [r.tail, r.head]
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

type Sequence = (Vec<u32>, u8, u8);

/// 1つのrelationの1つの値を解決する。成功すれば列を `out` に足す。
#[allow(clippy::too_many_arguments)]
fn resolve_entry(
    view: &GraphView<'_>,
    attrs: &GraphAttrs,
    rel: &RestrictionRelation,
    polarity: Polarity,
    modes: u8,
    stats: &mut Stats,
    out: &mut Vec<Sequence>,
) {
    // メンバーの構造
    let by_role = |role: &str| -> Vec<&RelationMember> {
        rel.members.iter().filter(|m| m.role == role).collect()
    };
    let (froms, tos, vias) = (by_role("from"), by_role("to"), by_role("via"));
    if froms.is_empty() || tos.is_empty() {
        return stats.fail("missing_from_or_to");
    }
    if froms.len() > 1 || tos.len() > 1 {
        return stats.fail("multiple_from_or_to");
    }
    let (from, to) = (froms[0], tos[0]);
    if from.kind != MemberKind::Way || to.kind != MemberKind::Way {
        return stats.fail("from_or_to_not_way");
    }
    if vias.is_empty() {
        return stats.fail("missing_via");
    }
    let via_node = vias.len() == 1 && vias[0].kind == MemberKind::Node;
    let via_way = vias.iter().all(|m| m.kind == MemberKind::Way);
    if !via_node && !via_way {
        return stats.fail("via_unsupported");
    }
    let family = kind_key(!via_node, polarity);

    let from_edges = attrs.way_edges(from.id);
    let to_edges = attrs.way_edges(to.id);
    if from_edges.is_empty() || to_edges.is_empty() {
        return stats.fail("way_not_in_graph");
    }

    // --- 列の本体：fromの有向辺、viaの有向辺、そして最後のノード ---
    let (f_candidates_node, last_node, middle): (NodeId, NodeId, Vec<DirEdge>);
    if via_node {
        let Some(g) = attrs.node_by_osm_id(vias[0].id) else {
            return stats.fail("via_node_not_in_graph");
        };
        f_candidates_node = g;
        last_node = g;
        middle = Vec::new();
    } else {
        // via-way：from - via1 - ... - viaK - to のつなぎ目のノードを決める
        let mut via_edges = Vec::new();
        for v in &vias {
            let range = attrs.way_edges(v.id);
            if range.is_empty() {
                return stats.fail("way_not_in_graph");
            }
            if way_ends(view, range.clone()).is_none() {
                return stats.fail("via_chain_broken");
            }
            via_edges.push(range);
        }
        // つなぎ目の候補。i番目は（i番目のメンバー、i+1番目のメンバー）の共有ノード
        let from_nodes = way_nodes(view, from_edges.clone());
        let to_nodes = way_nodes(view, to_edges.clone());
        let ends: Vec<(NodeId, NodeId)> = via_edges
            .iter()
            .map(|r| way_ends(view, r.clone()).expect("上で確かめた"))
            .collect();
        let mut candidates: Vec<Vec<NodeId>> = Vec::new();
        for i in 0..=via_edges.len() {
            let left: Vec<NodeId> = if i == 0 {
                from_nodes.clone()
            } else {
                vec![ends[i - 1].0, ends[i - 1].1]
            };
            let right: Vec<NodeId> = if i == via_edges.len() {
                to_nodes.clone()
            } else {
                vec![ends[i].0, ends[i].1]
            };
            let mut shared: Vec<NodeId> = left.into_iter().filter(|n| right.contains(n)).collect();
            shared.sort_unstable();
            shared.dedup();
            if shared.is_empty() {
                return stats.fail("via_chain_broken");
            }
            candidates.push(shared);
        }
        // 各viaのwayが、両端のノードを入口と出口にしている組合せを探す（ちょうど1つなら採用）
        let mut valid: Vec<Vec<NodeId>> = Vec::new();
        let mut current = Vec::new();
        fn search(
            i: usize,
            candidates: &[Vec<NodeId>],
            ends: &[(NodeId, NodeId)],
            current: &mut Vec<NodeId>,
            valid: &mut Vec<Vec<NodeId>>,
        ) {
            if i == candidates.len() {
                valid.push(current.clone());
                return;
            }
            for &n in &candidates[i] {
                if i > 0 {
                    let (a, b) = ends[i - 1];
                    let previous = current[i - 1];
                    let ok = (previous == a && n == b) || (previous == b && n == a);
                    if !ok || a == b {
                        continue;
                    }
                }
                current.push(n);
                search(i + 1, candidates, ends, current, valid);
                current.pop();
            }
        }
        search(0, &candidates, &ends, &mut current, &mut valid);
        let [junctions] = valid.as_slice() else {
            return stats.fail("via_chain_broken");
        };
        let mut path = Vec::new();
        for (i, range) in via_edges.iter().enumerate() {
            match via_path(view, range.clone(), junctions[i], junctions[i + 1]) {
                Some(p) => path.extend(p),
                None => return stats.fail("via_chain_broken"),
            }
        }
        f_candidates_node = junctions[0];
        last_node = *junctions.last().expect("junctionsは空でない");
        middle = path;
    }

    // fromの有向辺（f_candidates_nodeに入る）
    let f_cands = entering(view, from_edges, f_candidates_node);
    if f_cands.is_empty() {
        return stats.fail("from_not_adjacent");
    }
    let (f, mut allowed) = match pick(view, &f_cands, modes) {
        Pick::Unique(d, m) => (d, m),
        Pick::NotTraversable => return stats.fail("not_traversable"),
        Pick::Ambiguous => return stats.fail("from_ambiguous"),
    };
    // viaの列は、対象の手段で通れる手段に絞る
    for &d in &middle {
        allowed &= view.passable_modes(d, modes);
    }
    // toの有向辺（last_nodeから出る）
    let t_cands = leaving(view, to_edges, last_node);
    if t_cands.is_empty() {
        return stats.fail("to_not_adjacent");
    }
    let t = match pick(view, &t_cands, modes) {
        Pick::Unique(d, m) => {
            allowed &= m;
            d
        }
        Pick::NotTraversable => return stats.fail("not_traversable"),
        Pick::Ambiguous => return stats.fail("to_ambiguous"),
    };
    if allowed == 0 {
        return stats.fail("not_traversable");
    }

    let kind = u8::from(!via_node);
    let mut prefix: Vec<u32> = Vec::with_capacity(middle.len() + 2);
    prefix.push(f.0);
    prefix.extend(middle.iter().map(|d| d.0));
    match polarity {
        Polarity::No => {
            let mut seq = prefix;
            seq.push(t.0);
            out.push((seq, allowed, kind));
        }
        Polarity::Only => {
            // last_nodeから出る、to以外のすべての有向辺を禁止する
            for &o in view.out_dedges(last_node) {
                if o == t {
                    continue;
                }
                let mut seq = prefix.clone();
                seq.push(o.0);
                out.push((seq, allowed, kind));
                stats.only_derived += 1;
            }
        }
    }
    *stats.resolved.entry(family).or_default() += 1;
}

/// relationを有向辺の列にして、`restrictions.bin` の中身にする。
pub fn resolve(
    parts: &GraphParts,
    attrs: &GraphAttrs,
    relations: &[RestrictionRelation],
) -> Resolved {
    let view = GraphView::new(parts);
    let mut stats = Stats::default();
    let mut sequences: Vec<Sequence> = Vec::new();
    let mut unknown_value = 0u64;

    for rel in relations {
        stats.relations_total += 1;
        for (value, modes) in entries(rel, &mut stats) {
            stats.entries_total += 1;
            let polarity = if value.starts_with("no_") {
                Polarity::No
            } else if value.starts_with("only_") {
                Polarity::Only
            } else {
                unknown_value += 1;
                continue;
            };
            if modes == 0 {
                stats.fail("no_target_mode");
                continue;
            }
            resolve_entry(
                &view,
                attrs,
                rel,
                polarity,
                modes,
                &mut stats,
                &mut sequences,
            );
        }
    }
    stats.sequences_before_merge = sequences.len() as u64;

    // 列の辞書順に並べ、同じ列は手段をまとめる
    sequences.sort_unstable();
    let mut items: Vec<Restriction> = Vec::new();
    for (seq, modes, kind) in sequences {
        match items.last_mut() {
            Some(last) if last.seq.iter().map(|d| d.0).eq(seq.iter().copied()) => {
                last.modes |= modes;
                last.kind = last.kind.min(kind);
            }
            _ => items.push(Restriction {
                seq: seq.into_iter().map(DirEdge).collect(),
                modes,
                kind,
            }),
        }
    }

    let seq_len: usize = items.iter().map(|r| r.seq.len()).sum();
    let count_modes = |mask: u8| items.iter().filter(|r| r.modes == mask).count();
    let by_kind = |kind: u8| items.iter().filter(|r| r.kind == kind).count();
    let mut unresolved: Map<String, Value> = stats
        .unresolved
        .iter()
        .map(|(k, v)| ((*k).to_owned(), json!(v)))
        .collect();
    if unknown_value > 0 {
        unresolved.insert("unknown_value".to_owned(), json!(unknown_value));
    }
    let unresolved_total: u64 = unresolved.values().filter_map(Value::as_u64).sum();
    let resolved: Map<String, Value> =
        ["via_node_no", "via_node_only", "via_way_no", "via_way_only"]
            .iter()
            .map(|k| {
                (
                    (*k).to_owned(),
                    json!(stats.resolved.get(k).copied().unwrap_or(0)),
                )
            })
            .collect();
    let audit = json!({
        "relations_total": stats.relations_total,
        "entries_total": stats.entries_total,
        "resolved": resolved,
        "unresolved": unresolved,
        "unresolved_total": unresolved_total,
        "conditional_ignored": stats.conditional_ignored,
        "other_vehicle_ignored": stats.other_vehicle_ignored,
        "relations_without_supported_tag": stats.relations_without_supported_tag,
        "only_derived_prohibitions": stats.only_derived,
        "sequences_before_merge": stats.sequences_before_merge,
        "items": items.len(),
        "seq_len": seq_len,
        "items_by_kind": { "via_node": by_kind(0), "via_way": by_kind(1) },
        "items_by_modes": {
            "car": count_modes(CAR),
            "bike": count_modes(BIKE),
            "car_bike": count_modes(CAR | BIKE),
        },
    });
    Resolved {
        restrictions: Restrictions {
            params: audit.clone(),
            items,
        },
        audit,
    }
}

/// 手段のビット（`Mode::mask`）が定数と一致していること（`WALK`・`BIKE`・`CAR`）。
const _: () = {
    assert!(Mode::Walk.mask() == WALK);
    assert!(Mode::Bike.mask() == BIKE);
    assert!(Mode::Car.mask() == CAR);
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::routing::graph::HighwayClass;
    use crate::domain::routing::graph::access;
    use crate::domain::routing::prep::graph_build::{
        BuiltGraph, NodeTable, WayMeta, WayTable, build_graph,
    };

    const BASE_LAT: i32 = 350_000_000;
    const BASE_LON: i32 = 1_390_000_000;
    const TWO: u8 = access::CAR_FWD | access::CAR_BWD | access::BIKE_FWD | access::BIKE_BWD;
    const ONE_WAY: u8 = access::CAR_FWD | access::BIKE_FWD;

    /// 道路網のテスト用の組み立て。ノードは `(id, 北, 東)`、wayは `(id, access, ノードID列)`。
    fn graph(nodes: &[(i64, i32, i32)], ways: &[(i64, u8, &[i64])]) -> BuiltGraph {
        let mut sorted: Vec<_> = nodes.to_vec();
        sorted.sort();
        let table = NodeTable {
            id: sorted.iter().map(|n| n.0).collect(),
            lat_e7: sorted.iter().map(|n| BASE_LAT + n.1).collect(),
            lon_e7: sorted.iter().map(|n| BASE_LON + n.2).collect(),
            flags: vec![0; sorted.len()],
        };
        let mut ways_sorted = ways.to_vec();
        ways_sorted.sort_by_key(|w| w.0);
        let mut w = WayTable {
            ref_ofs: vec![0],
            ..WayTable::default()
        };
        for (id, acc, refs) in &ways_sorted {
            w.id.push(*id);
            w.meta.push(WayMeta {
                class: HighwayClass::Residential.code(),
                access: *acc,
                ..WayMeta::default()
            });
            w.ref_str.push(None);
            w.name_str.push(None);
            for r in *refs {
                w.refs.push(table.id.binary_search(r).unwrap() as u32);
            }
            w.ref_ofs.push(w.refs.len() as u32);
        }
        build_graph(&w, &table, Map::new()).unwrap()
    }

    fn rel(
        id: i64,
        tags: &[(&str, &str)],
        members: &[(MemberKind, i64, &str)],
    ) -> RestrictionRelation {
        let mut all = vec![("type".to_owned(), "restriction".to_owned())];
        all.extend(tags.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())));
        RestrictionRelation {
            id,
            tags: all,
            members: members
                .iter()
                .map(|(kind, id, role)| RelationMember {
                    kind: *kind,
                    id: *id,
                    role: (*role).to_owned(),
                })
                .collect(),
        }
    }

    /// 有向辺を（辺の元のway ID、向き）で読みやすくする。
    fn name(attrs: &GraphAttrs, d: DirEdge) -> (i64, bool) {
        (attrs.way_id[d.edge() as usize], d.is_reverse())
    }

    fn seqs(attrs: &GraphAttrs, r: &Resolved) -> Vec<(Vec<(i64, bool)>, u8, u8)> {
        r.restrictions
            .items
            .iter()
            .map(|i| {
                (
                    i.seq.iter().map(|&d| name(attrs, d)).collect(),
                    i.modes,
                    i.kind,
                )
            })
            .collect()
    }

    /// T字路：way1 は 1(西)→2(交差点)、way2 は 2→3(東)、way3 は 2→4(北)。2が交差点
    ///
    /// ```text
    ///        4
    ///        |
    ///  1 --- 2 --- 3
    /// ```
    fn t_junction() -> BuiltGraph {
        graph(
            &[(1, 0, 0), (2, 0, 1000), (3, 0, 2000), (4, 1000, 1000)],
            &[(1, TWO, &[1, 2]), (2, TWO, &[2, 3]), (3, TWO, &[2, 4])],
        )
    }

    #[test]
    fn via_node_no_turn_becomes_a_two_edge_sequence() {
        let g = t_junction();
        let relation = rel(
            1,
            &[("restriction", "no_left_turn")],
            &[
                (MemberKind::Way, 1, "from"),
                (MemberKind::Node, 2, "via"),
                (MemberKind::Way, 3, "to"),
            ],
        );
        let r = resolve(&g.parts, &g.attrs, &[relation]);
        // way1（西→交差点）から way3（交差点→北）へ入る。way1 は辺0の順向き、way3 は辺2の順向き
        assert_eq!(
            seqs(&g.attrs, &r),
            vec![(vec![(1, false), (3, false)], CAR | BIKE, 0)]
        );
        assert_eq!(r.audit["resolved"]["via_node_no"], 1);
        assert_eq!(r.audit["unresolved_total"], 0);
        assert_eq!(r.restrictions.params["items"], 1);
        // 列の隣り合う有向辺はつながっている
        let seq = &r.restrictions.items[0].seq;
        let view = GraphView::new(&g.parts);
        assert_eq!(view.dedge_to(seq[0]), view.dedge_from(seq[1]));
    }

    #[test]
    fn u_turn_uses_the_same_way_in_opposite_directions() {
        let g = t_junction();
        let relation = rel(
            1,
            &[("restriction", "no_u_turn")],
            &[
                (MemberKind::Way, 1, "from"),
                (MemberKind::Node, 2, "via"),
                (MemberKind::Way, 1, "to"),
            ],
        );
        let r = resolve(&g.parts, &g.attrs, &[relation]);
        assert_eq!(
            seqs(&g.attrs, &r),
            vec![(vec![(1, false), (1, true)], CAR | BIKE, 0)]
        );
    }

    #[test]
    fn only_straight_on_forbids_every_other_outgoing_edge() {
        let g = t_junction();
        let relation = rel(
            1,
            &[("restriction", "only_straight_on")],
            &[
                (MemberKind::Way, 1, "from"),
                (MemberKind::Node, 2, "via"),
                (MemberKind::Way, 2, "to"),
            ],
        );
        let r = resolve(&g.parts, &g.attrs, &[relation]);
        // 交差点から出る有向辺は way1 の逆向き（Uターン）、way2 の順向き（to）、way3 の順向き。to 以外の2つを禁止
        assert_eq!(
            seqs(&g.attrs, &r),
            vec![
                (vec![(1, false), (1, true)], CAR | BIKE, 0),
                (vec![(1, false), (3, false)], CAR | BIKE, 0),
            ]
        );
        assert_eq!(r.audit["only_derived_prohibitions"], 2);
        assert_eq!(r.audit["resolved"]["via_node_only"], 1);
    }

    #[test]
    fn except_bicycle_limits_to_car_and_motor_tags_set_the_mode() {
        let g = t_junction();
        let members = [
            (MemberKind::Way, 1, "from"),
            (MemberKind::Node, 2, "via"),
            (MemberKind::Way, 3, "to"),
        ];
        let cases = [
            (
                vec![("restriction", "no_left_turn"), ("except", "bicycle")],
                CAR,
            ),
            (
                vec![("restriction", "no_left_turn"), ("except", "motorcar")],
                BIKE,
            ),
            (
                vec![("restriction", "no_left_turn"), ("except", "bicycle;psv")],
                CAR,
            ),
            (vec![("restriction:motorcar", "no_left_turn")], CAR),
            (vec![("restriction:motor_vehicle", "no_left_turn")], CAR),
            (vec![("restriction:bicycle", "no_left_turn")], BIKE),
        ];
        for (tags, expected) in cases {
            let r = resolve(&g.parts, &g.attrs, &[rel(1, &tags, &members)]);
            assert_eq!(r.restrictions.items.len(), 1, "{tags:?}");
            assert_eq!(r.restrictions.items[0].modes, expected, "{tags:?}");
        }
        // 全部の手段を除外すると何も作らない
        let r = resolve(
            &g.parts,
            &g.attrs,
            &[rel(
                1,
                &[
                    ("restriction", "no_left_turn"),
                    ("except", "bicycle;motor_vehicle"),
                ],
                &members,
            )],
        );
        assert!(r.restrictions.items.is_empty());
        assert_eq!(r.audit["unresolved"]["no_target_mode"], 1);
        // walkには適用しない（restriction:foot などは数えるだけ）
        let r = resolve(
            &g.parts,
            &g.attrs,
            &[rel(1, &[("restriction:hgv", "no_left_turn")], &members)],
        );
        assert!(r.restrictions.items.is_empty());
        assert_eq!(r.audit["other_vehicle_ignored"], 1);
        // restriction:conditional は件数だけ
        let r = resolve(
            &g.parts,
            &g.attrs,
            &[rel(
                1,
                &[(
                    "restriction:conditional",
                    "no_left_turn @ (Mo-Fr 07:00-09:00)",
                )],
                &members,
            )],
        );
        assert!(r.restrictions.items.is_empty());
        assert_eq!(r.audit["conditional_ignored"], 1);
        assert_eq!(r.audit["relations_without_supported_tag"], 1);
    }

    #[test]
    fn duplicate_sequences_are_merged_and_sorted() {
        let g = t_junction();
        let members = [
            (MemberKind::Way, 1, "from"),
            (MemberKind::Node, 2, "via"),
            (MemberKind::Way, 3, "to"),
        ];
        let relations = [
            rel(1, &[("restriction:bicycle", "no_left_turn")], &members),
            rel(2, &[("restriction:motorcar", "no_left_turn")], &members),
            rel(
                3,
                &[("restriction", "no_u_turn")],
                &[
                    (MemberKind::Way, 1, "from"),
                    (MemberKind::Node, 2, "via"),
                    (MemberKind::Way, 1, "to"),
                ],
            ),
        ];
        let r = resolve(&g.parts, &g.attrs, &relations);
        // 同じ列（way1→way3）は手段をまとめ、列の辞書順（Uターンの方が小さい）に並べる
        assert_eq!(
            seqs(&g.attrs, &r),
            vec![
                (vec![(1, false), (1, true)], CAR | BIKE, 0),
                (vec![(1, false), (3, false)], CAR | BIKE, 0),
            ]
        );
        assert_eq!(r.audit["sequences_before_merge"], 3);
        assert_eq!(r.audit["items"], 2);
    }

    /// via-way用：way1(A) 1→2、way2(C) 2→3、way3(V) 2→7→4（7でwayEが交わり2辺に分かれる）、
    /// way4(B) 4→5、way5(E) 7→8。
    ///
    /// ```text
    ///  1 --A-- 2 --C-- 3
    ///          |
    ///          V       8
    ///          |       |
    ///          7 --E---+   （7-8 は way5）
    ///          |
    ///          4 --B-- 5
    /// ```
    fn via_way_network() -> BuiltGraph {
        graph(
            &[
                (1, 0, 0),
                (2, 0, 1000),
                (3, 0, 2000),
                (7, -500, 1000),
                (4, -1000, 1000),
                (5, -1000, 2000),
                (8, -500, 2000),
            ],
            &[
                (1, TWO, &[1, 2]),
                (2, TWO, &[2, 3]),
                (3, TWO, &[2, 7, 4]),
                (4, TWO, &[4, 5]),
                (5, TWO, &[7, 8]),
            ],
        )
    }

    #[test]
    fn via_way_no_turn_chains_the_edges_of_the_via_way() {
        let g = via_way_network();
        // way3 は 2つの辺（2→7、7→4）になっている
        assert_eq!(g.attrs.way_edges(3).len(), 2);
        let relation = rel(
            1,
            &[("restriction", "no_right_turn")],
            &[
                (MemberKind::Way, 1, "from"),
                (MemberKind::Way, 3, "via"),
                (MemberKind::Way, 4, "to"),
            ],
        );
        let r = resolve(&g.parts, &g.attrs, &[relation]);
        assert_eq!(
            seqs(&g.attrs, &r),
            vec![(
                vec![(1, false), (3, false), (3, false), (4, false)],
                CAR | BIKE,
                1
            )]
        );
        assert_eq!(r.audit["resolved"]["via_way_no"], 1);
        let view = GraphView::new(&g.parts);
        let seq = &r.restrictions.items[0].seq;
        for pair in seq.windows(2) {
            assert_eq!(view.dedge_to(pair[0]), view.dedge_from(pair[1]));
        }
        // 辺の番号は way3 が 2辺：先の辺が先
        assert!(seq[1].edge() < seq[2].edge());
    }

    #[test]
    fn via_way_in_the_opposite_direction_reverses_the_via_edges() {
        let g = via_way_network();
        let relation = rel(
            1,
            &[("restriction", "no_left_turn")],
            &[
                (MemberKind::Way, 4, "from"),
                (MemberKind::Way, 3, "via"),
                (MemberKind::Way, 1, "to"),
            ],
        );
        let r = resolve(&g.parts, &g.attrs, &[relation]);
        let s = seqs(&g.attrs, &r);
        assert_eq!(
            s,
            vec![(
                vec![(4, true), (3, true), (3, true), (1, true)],
                CAR | BIKE,
                1
            )]
        );
        // via の辺は逆順（7→4 側から 2 側へ）
        let seq = &r.restrictions.items[0].seq;
        assert!(seq[1].edge() > seq[2].edge());
    }

    #[test]
    fn via_way_only_forbids_other_exits_after_the_via_way() {
        let g = via_way_network();
        let relation = rel(
            1,
            &[("restriction", "only_straight_on")],
            &[
                (MemberKind::Way, 1, "from"),
                (MemberKind::Way, 3, "via"),
                (MemberKind::Way, 4, "to"),
            ],
        );
        let r = resolve(&g.parts, &g.attrs, &[relation]);
        // 最後のノード 4 から出る有向辺は way3 の逆向き（戻る）と way4 の順向き（to）。to 以外の1本を禁止
        assert_eq!(
            seqs(&g.attrs, &r),
            vec![(
                vec![(1, false), (3, false), (3, false), (3, true)],
                CAR | BIKE,
                1
            )]
        );
        assert_eq!(r.audit["only_derived_prohibitions"], 1);
        assert_eq!(r.audit["resolved"]["via_way_only"], 1);
    }

    #[test]
    fn via_way_chain_of_two_ways() {
        // way3 の途中の辺ではなく、2本のviaのwayをつなぐ：A(1→2) - V1(2→7) - V2(7→4) - B(4→5)
        let g = graph(
            &[
                (1, 0, 0),
                (2, 0, 1000),
                (7, -500, 1000),
                (4, -1000, 1000),
                (5, -1000, 2000),
            ],
            &[
                (1, TWO, &[1, 2]),
                (2, TWO, &[2, 7]),
                (3, TWO, &[7, 4]),
                (4, TWO, &[4, 5]),
            ],
        );
        let relation = rel(
            1,
            &[("restriction", "no_straight_on")],
            &[
                (MemberKind::Way, 1, "from"),
                (MemberKind::Way, 2, "via"),
                (MemberKind::Way, 3, "via"),
                (MemberKind::Way, 4, "to"),
            ],
        );
        let r = resolve(&g.parts, &g.attrs, &[relation]);
        assert_eq!(
            seqs(&g.attrs, &r),
            vec![(
                vec![(1, false), (2, false), (3, false), (4, false)],
                CAR | BIKE,
                1
            )]
        );
    }

    #[test]
    fn one_way_streets_narrow_the_modes_and_decide_ambiguity() {
        // F(1→2→3) が 2 を通り抜ける。2で G(2→4) が分かれる
        let nodes = [(1, 0, 0), (2, 0, 1000), (3, 0, 2000), (4, 1000, 1000)];
        let members = [
            (MemberKind::Way, 10, "from"),
            (MemberKind::Node, 2, "via"),
            (MemberKind::Way, 11, "to"),
        ];
        let relation = rel(1, &[("restriction", "no_left_turn")], &members);
        // 両方向に通れる F は、2の両側の辺がfromの候補になるので決められない
        let g = graph(&nodes, &[(10, TWO, &[1, 2, 3]), (11, TWO, &[2, 4])]);
        let r = resolve(&g.parts, &g.attrs, &[relation.clone()]);
        assert!(r.restrictions.items.is_empty());
        assert_eq!(r.audit["unresolved"]["from_ambiguous"], 1);
        // 一方通行（1→2→3）なら、2に入る有向辺は1つだけ
        let g = graph(&nodes, &[(10, ONE_WAY, &[1, 2, 3]), (11, TWO, &[2, 4])]);
        let r = resolve(&g.parts, &g.attrs, &[relation.clone()]);
        assert_eq!(
            seqs(&g.attrs, &r),
            vec![(vec![(10, false), (11, false)], CAR | BIKE, 0)]
        );
        // toが一方通行で、viaに向かう向き（4→2）だけなら、carもbikeも通れず捨てる
        let g = graph(&nodes, &[(10, TWO, &[1, 2]), (11, ONE_WAY, &[4, 2])]);
        let r = resolve(&g.parts, &g.attrs, &[relation.clone()]);
        assert!(r.restrictions.items.is_empty());
        assert_eq!(r.audit["unresolved"]["not_traversable"], 1);
        // bikeだけ通れないtoなら、carに絞る
        let car_only = access::CAR_FWD | access::CAR_BWD;
        let g = graph(&nodes, &[(10, TWO, &[1, 2]), (11, car_only, &[2, 4])]);
        let r = resolve(&g.parts, &g.attrs, &[relation]);
        assert_eq!(r.restrictions.items.len(), 1);
        assert_eq!(r.restrictions.items[0].modes, CAR);
    }

    #[test]
    fn unresolvable_relations_are_counted_by_reason() {
        let g = t_junction();
        let way = |id, role| (MemberKind::Way, id, role);
        let relations = [
            // from がない
            rel(
                1,
                &[("restriction", "no_left_turn")],
                &[(MemberKind::Node, 2, "via"), way(3, "to")],
            ),
            // via がない
            rel(
                2,
                &[("restriction", "no_left_turn")],
                &[way(1, "from"), way(3, "to")],
            ),
            // 複数の from
            rel(
                3,
                &[("restriction", "no_left_turn")],
                &[
                    way(1, "from"),
                    way(2, "from"),
                    (MemberKind::Node, 2, "via"),
                    way(3, "to"),
                ],
            ),
            // グラフにないway
            rel(
                4,
                &[("restriction", "no_left_turn")],
                &[way(999, "from"), (MemberKind::Node, 2, "via"), way(3, "to")],
            ),
            // グラフにないviaノード
            rel(
                5,
                &[("restriction", "no_left_turn")],
                &[way(1, "from"), (MemberKind::Node, 999, "via"), way(3, "to")],
            ),
            // viaノードに接しないfrom（way2は2-3、viaは1）
            rel(
                6,
                &[("restriction", "no_left_turn")],
                &[way(2, "from"), (MemberKind::Node, 1, "via"), way(1, "to")],
            ),
            // 値が不明
            rel(
                7,
                &[("restriction", "forbidden")],
                &[way(1, "from"), (MemberKind::Node, 2, "via"), way(3, "to")],
            ),
            // viaにrelation
            rel(
                8,
                &[("restriction", "no_left_turn")],
                &[
                    way(1, "from"),
                    (MemberKind::Relation, 5, "via"),
                    way(3, "to"),
                ],
            ),
        ];
        let r = resolve(&g.parts, &g.attrs, &relations);
        assert!(r.restrictions.items.is_empty());
        let u = &r.audit["unresolved"];
        assert_eq!(u["missing_from_or_to"], 1);
        assert_eq!(u["missing_via"], 1);
        assert_eq!(u["multiple_from_or_to"], 1);
        assert_eq!(u["way_not_in_graph"], 1);
        assert_eq!(u["via_node_not_in_graph"], 1);
        assert_eq!(u["from_not_adjacent"], 1);
        assert_eq!(u["unknown_value"], 1);
        assert_eq!(u["via_unsupported"], 1);
        assert_eq!(r.audit["unresolved_total"], 8);
        assert_eq!(r.audit["relations_total"], 8);
        assert_eq!(r.audit["sequences_before_merge"], 0);
    }
}
