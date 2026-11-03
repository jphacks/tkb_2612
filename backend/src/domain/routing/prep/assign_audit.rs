//! P5：割り当ての検査と集計（形式書13章、作業書M2b.4、M0.6の「割り当ての重み」）。
//!
//! - [`verify_assignment`]：全件の検査。1件の事故の重みの合計が1以下（`MIRROR` を除く）、各重みが0.1以上、
//!   候補が3個以下、高速・自専道のコードの事故は高速・自専道の辺（クラスタ）にしか載らず、それ以外の事故は載らない、
//!   割り当てのある事故と帰属不明が排他で合わせて全事故、`MIRROR` と双子の対応、など。
//! - [`summarize`]：道路種別・道路形状ごとの割り当てた割合、帰属不明の割合と理由、最大の重みの分布、
//!   国道の `ref` 一致率（監査JSON）。

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::domain::routing::accident::{
    AccidentAttr, Assignment, MISSING_U8, RouteClass, TargetKind, UnassignedReason,
};
use crate::domain::routing::graph::{EdgeId, GraphParts};
use crate::domain::routing::prep::assign::{
    expected_number, is_intersection_shape, is_motorway_edge, is_road_edge, named_numbers,
    numeric_tokens,
};
use crate::domain::routing::prep::attrs::GraphAttrs;
use crate::domain::routing::topology::Topology;

/// 道路種別（路線区分のまとめ方）。添字は [`route_group`] の値。
pub const ROUTE_GROUPS: [&str; 6] = [
    "国道",
    "主要地方道",
    "一般県道",
    "市町村道",
    "高速・自専道",
    "その他・不明",
];

/// 道路形状のまとめ方。添字は [`shape_group`] の値。
pub const SHAPE_GROUPS: [&str; 5] = ["交差点", "交差点付近", "単路", "一般交通の場所", "不明"];

/// 事故の路線区分を [`ROUTE_GROUPS`] の添字にする。
pub fn route_group(attr: &AccidentAttr) -> usize {
    match attr.route_class_kind() {
        Some(RouteClass::NationalRoute) => 0,
        Some(RouteClass::MajorPrefecturalRoad | RouteClass::MajorMunicipalRoad) => 1,
        Some(RouteClass::PrefecturalRoad) => 2,
        Some(RouteClass::MunicipalRoad) => 3,
        Some(c) if c.is_motorway_code() => 4,
        _ => 5,
    }
}

/// 事故の道路形状を [`SHAPE_GROUPS`] の添字にする。
pub fn shape_group(attr: &AccidentAttr) -> usize {
    match attr.road_shape {
        1 | 31 => 0,
        7 | 37 => 1,
        11..=14 | 21..=23 => 2,
        0 => 3,
        _ => 4,
    }
}

/// 検査で数えた件数。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VerifyStats {
    pub accidents: usize,
    pub assigned: usize,
    pub unassigned: usize,
    pub targets: usize,
    pub edge_targets: usize,
    pub cluster_targets: usize,
    pub mirror_targets: usize,
    pub motorway_coded_assigned: usize,
    pub max_weight_sum: f64,
}

/// クラスタに接する車道の辺の種類（ビット0：高速・自専道の辺、ビット1：それ以外）。
fn cluster_kinds(graph: &GraphParts, topology: &Topology, cluster: u32) -> u8 {
    let mut kinds = 0;
    for &v in topology.nodes_of_cluster(cluster) {
        let from = graph.node_ofs[v as usize] as usize;
        let to = graph.node_ofs[v as usize + 1] as usize;
        for d in &graph.adj[from..to] {
            let record = &graph.edges[d.edge() as usize];
            if is_road_edge(record) {
                kinds |= if is_motorway_edge(record) { 1 } else { 2 };
            }
        }
    }
    kinds
}

/// 割り当てを全件検査する。違反は先頭から最大20件をまとめて返す。
///
/// `Assignment::validate`（構造と番号の範囲）に加えて、グラフ・位相・事故の属性との整合を確かめる。
pub fn verify_assignment(
    assignment: &Assignment,
    graph: &GraphParts,
    topology: &Topology,
    attrs: &[AccidentAttr],
) -> Result<VerifyStats, String> {
    assignment.validate(graph.edges.len(), topology.clusters.len())?;
    if assignment.accident_count() != attrs.len() {
        return Err(format!(
            "割り当ての事故の数{}が属性の数{}と違います",
            assignment.accident_count(),
            attrs.len()
        ));
    }
    const LIMIT: usize = 20;
    let mut problems: Vec<String> = Vec::new();
    let mut fail = |message: String| {
        if problems.len() < LIMIT {
            problems.push(message);
        }
    };
    let mut stats = VerifyStats {
        accidents: attrs.len(),
        ..VerifyStats::default()
    };
    let mut kinds_cache: Vec<u8> = vec![0xFF; topology.clusters.len()];
    let reasons: BTreeMap<u32, UnassignedReason> = assignment
        .unassigned
        .iter()
        .map(|r| (r.accident, r.reason))
        .collect();
    for (i, attr) in attrs.iter().enumerate() {
        let targets = assignment.targets_of(i);
        let motorway_coded = attr.is_motorway_code();
        if targets.is_empty() {
            stats.unassigned += 1;
            let reason = reasons.get(&(i as u32));
            // 理由4は道路形状0の事故だけで、道路形状0の事故は理由4だけ
            if (attr.road_shape == 0) != (reason == Some(&UnassignedReason::GeneralTrafficPlace)) {
                fail(format!(
                    "事故{i}：道路形状{}と帰属不明の理由{:?}が対応しません",
                    attr.road_shape, reason
                ));
            }
            continue;
        }
        stats.assigned += 1;
        stats.motorway_coded_assigned += usize::from(motorway_coded);
        if attr.road_shape == 0 {
            fail(format!("事故{i}：道路形状0なのに割り当てがあります"));
        }
        let mut max_weight = 0.0_f32;
        for t in targets {
            stats.targets += 1;
            match TargetKind::from_code(t.kind) {
                Some(TargetKind::Edge) => {
                    stats.edge_targets += 1;
                    let record = &graph.edges[t.target as usize];
                    if !is_road_edge(record) {
                        fail(format!("事故{i}：辺{}は車道の辺ではありません", t.target));
                    }
                    if t.pos_dm > record.length_dm {
                        fail(format!(
                            "事故{i}：辺{}の位置{}が長さ{}を超えています",
                            t.target, t.pos_dm, record.length_dm
                        ));
                    }
                    if is_motorway_edge(record) != motorway_coded {
                        fail(format!(
                            "事故{i}：辺{}（高速・自専道の辺={}）と路線コード（高速・自専道={motorway_coded}）が一致しません",
                            t.target,
                            is_motorway_edge(record)
                        ));
                    }
                }
                Some(TargetKind::Cluster) => {
                    stats.cluster_targets += 1;
                    if !is_intersection_shape(attr.road_shape) {
                        fail(format!(
                            "事故{i}：交差点でない道路形状{}でクラスタに割り当てています",
                            attr.road_shape
                        ));
                    }
                    let slot = &mut kinds_cache[t.target as usize];
                    if *slot == 0xFF {
                        *slot = cluster_kinds(graph, topology, t.target);
                    }
                    let ok = if motorway_coded {
                        *slot & 1 != 0
                    } else {
                        *slot & 2 != 0 || *slot == 0
                    };
                    if !ok {
                        fail(format!(
                            "事故{i}：クラスタ{}の接する辺（種類{}）と路線コードが一致しません",
                            t.target, *slot
                        ));
                    }
                }
                None => fail(format!("事故{i}：kindが不正です")),
            }
            if t.is_mirror() {
                stats.mirror_targets += 1;
                let source = topology.twin_of(t.target).and_then(|twin| {
                    targets.iter().find(|x| {
                        !x.is_mirror() && x.kind == TargetKind::Edge.code() && x.target == twin
                    })
                });
                match source {
                    Some(x) if x.weight == t.weight => {}
                    _ => fail(format!(
                        "事故{i}：写しの辺{}に、双子の元の候補（同じ重み）がありません",
                        t.target
                    )),
                }
            } else {
                max_weight = max_weight.max(t.weight);
            }
        }
        stats.max_weight_sum += f64::from(max_weight);
        // 双子のある候補は、双子が規則を満たすなら必ず写しがある
        for t in targets.iter().filter(|t| !t.is_mirror()) {
            if t.kind != TargetKind::Edge.code() {
                continue;
            }
            let Some(twin) = topology.twin_of(t.target) else {
                continue;
            };
            let record = &graph.edges[twin as usize];
            let eligible = is_road_edge(record) && is_motorway_edge(record) == motorway_coded;
            let present = targets
                .iter()
                .any(|x| x.kind == TargetKind::Edge.code() && x.target == twin);
            if eligible && !present {
                fail(format!(
                    "事故{i}：辺{}の双子{twin}への写しがありません",
                    t.target
                ));
            }
        }
    }
    if stats.assigned + stats.unassigned != stats.accidents
        || stats.unassigned != assignment.unassigned.len()
    {
        fail("割り当てのある事故と帰属不明の事故が、全事故と一致しません".to_owned());
    }
    if problems.is_empty() {
        Ok(stats)
    } else {
        Err(problems.join("\n"))
    }
}

const WEIGHT_BINS: [(&str, f32, f32); 5] = [
    ("0.1-0.3", 0.0, 0.3),
    ("0.3-0.5", 0.3, 0.5),
    ("0.5-0.8", 0.5, 0.8),
    ("0.8-0.95", 0.8, 0.95),
    ("0.95-1.0", 0.95, f32::INFINITY),
];

fn reason_slot(reason: UnassignedReason) -> usize {
    match reason {
        UnassignedReason::NoCandidate => 0,
        UnassignedReason::MotorwayCodeMismatch => 1,
        UnassignedReason::InsufficientWeight => 2,
        UnassignedReason::GeneralTrafficPlace => 3,
        UnassignedReason::Other => 4,
    }
}

const REASON_NAMES: [&str; 5] = [
    "1_no_candidate",
    "2_motorway_code_mismatch",
    "3_insufficient_weight",
    "4_general_traffic_place",
    "255_other",
];

#[derive(Debug, Clone, Default)]
struct Acc {
    total: u64,
    assigned: u64,
    reasons: [u64; 5],
    bins: [u64; 5],
    max_on_edge: u64,
    max_on_cluster: u64,
    max_weight_sum: f64,
}

impl Acc {
    fn json(&self) -> Value {
        let ratio = |n: u64, d: u64| {
            if d == 0 {
                Value::Null
            } else {
                json!(n as f64 / d as f64)
            }
        };
        let unassigned = self.total - self.assigned;
        let bins: BTreeMap<&str, u64> = WEIGHT_BINS
            .iter()
            .zip(self.bins)
            .map(|((name, _, _), n)| (*name, n))
            .collect();
        let reasons: BTreeMap<&str, u64> = REASON_NAMES.iter().copied().zip(self.reasons).collect();
        let reason_ratios: BTreeMap<&str, Value> = REASON_NAMES
            .iter()
            .copied()
            .zip(self.reasons)
            .map(|(name, n)| (name, ratio(n, self.total)))
            .collect();
        json!({
            "accidents": self.total,
            "assigned": self.assigned,
            "unassigned": unassigned,
            "assigned_ratio": ratio(self.assigned, self.total),
            "unassigned_ratio": ratio(unassigned, self.total),
            "unassigned_by_reason": reasons,
            "unassigned_ratio_by_reason": reason_ratios,
            "max_weight_histogram": bins,
            "max_weight_ge_0_8": self.bins[3] + self.bins[4],
            "max_weight_ge_0_8_ratio_of_assigned": ratio(self.bins[3] + self.bins[4], self.assigned),
            "max_weight_mean": if self.assigned == 0 { Value::Null } else { json!(self.max_weight_sum / self.assigned as f64) },
            "max_target_on_edge": self.max_on_edge,
            "max_target_on_cluster": self.max_on_cluster,
        })
    }
}

/// 番号の照合の集計（国道）。
#[derive(Debug, Clone, Default)]
struct RefAcc {
    accidents: u64,
    unassigned: u64,
    max_on_cluster: u64,
    max_on_edge: u64,
    edge_has_number: u64,
    ref_match: u64,
    name_match: u64,
    any_match: u64,
}

impl RefAcc {
    fn json(&self) -> Value {
        let ratio = |n: u64, d: u64| {
            if d == 0 {
                Value::Null
            } else {
                json!(n as f64 / d as f64)
            }
        };
        let assigned = self.accidents - self.unassigned;
        json!({
            "accidents": self.accidents,
            "assigned": assigned,
            "unassigned": self.unassigned,
            "max_on_edge": self.max_on_edge,
            "max_on_cluster": self.max_on_cluster,
            "max_edge_has_numeric_ref_or_name": self.edge_has_number,
            "max_edge_ref_matches": self.ref_match,
            "max_edge_name_matches": self.name_match,
            "max_edge_ref_or_name_matches": self.any_match,
            "ref_match_ratio_of_accidents": ratio(self.ref_match, self.accidents),
            "ref_match_ratio_of_assigned": ratio(self.ref_match, assigned),
            "ref_match_ratio_of_max_on_edge": ratio(self.ref_match, self.max_on_edge),
            "ref_match_ratio_of_max_edge_with_number": ratio(self.ref_match, self.edge_has_number),
            "ref_or_name_match_ratio_of_accidents": ratio(self.any_match, self.accidents),
        })
    }
}

/// 事故の最大の重みの割り当て（`MIRROR` を除く。同じ重みなら `(kind, target, pos_dm)` の小さい方）。
pub fn max_target(
    assignment: &Assignment,
    accident: usize,
) -> Option<&crate::domain::routing::accident::AssignTarget> {
    let mut best: Option<&crate::domain::routing::accident::AssignTarget> = None;
    for t in assignment
        .targets_of(accident)
        .iter()
        .filter(|t| !t.is_mirror())
    {
        if best.is_none_or(|b| t.weight > b.weight) {
            best = Some(t);
        }
    }
    best
}

/// 道路種別・道路形状ごとの集計（作業書M2b.4）。
pub fn summarize(
    assignment: &Assignment,
    attrs: &[AccidentAttr],
    graph: &GraphParts,
    graph_attrs: &GraphAttrs,
) -> Value {
    let reasons: BTreeMap<u32, UnassignedReason> = assignment
        .unassigned
        .iter()
        .map(|r| (r.accident, r.reason))
        .collect();
    let mut overall = Acc::default();
    let mut by_route = vec![Acc::default(); ROUTE_GROUPS.len()];
    let mut by_shape = vec![Acc::default(); SHAPE_GROUPS.len()];
    let mut by_both = vec![vec![Acc::default(); SHAPE_GROUPS.len()]; ROUTE_GROUPS.len()];
    let mut by_osm_class: BTreeMap<String, u64> = BTreeMap::new();
    let mut national = RefAcc::default();
    let mut multi_target = [0_u64; 4];
    let mut mirror_accidents = 0_u64;
    for (i, attr) in attrs.iter().enumerate() {
        let (rg, sg) = (route_group(attr), shape_group(attr));
        let targets = assignment.targets_of(i);
        let max = max_target(assignment, i);
        let is_national = attr.route_class_kind() == Some(RouteClass::NationalRoute);
        for acc in [
            &mut overall,
            &mut by_route[rg],
            &mut by_shape[sg],
            &mut by_both[rg][sg],
        ] {
            acc.total += 1;
            match max {
                None => {
                    let reason = reasons.get(&(i as u32)).copied();
                    acc.reasons[reason_slot(reason.unwrap_or(UnassignedReason::Other))] += 1;
                }
                Some(t) => {
                    acc.assigned += 1;
                    let w = t.weight;
                    let bin = WEIGHT_BINS
                        .iter()
                        .position(|&(_, lo, hi)| w >= lo && w < hi)
                        .unwrap_or(4);
                    acc.bins[bin] += 1;
                    acc.max_weight_sum += f64::from(w);
                    if t.kind == TargetKind::Edge.code() {
                        acc.max_on_edge += 1;
                    } else {
                        acc.max_on_cluster += 1;
                    }
                }
            }
        }
        national.accidents += u64::from(is_national);
        let Some(t) = max else {
            national.unassigned += u64::from(is_national);
            continue;
        };
        let direct = targets.iter().filter(|x| !x.is_mirror()).count();
        multi_target[direct.min(3)] += 1;
        mirror_accidents += u64::from(targets.iter().any(|x| x.is_mirror()));
        if t.kind == TargetKind::Edge.code() {
            let class = HighwayClassName::of(graph.edges[t.target as usize].class);
            *by_osm_class.entry(class).or_insert(0) += 1;
        }
        if !is_national {
            continue;
        }
        if t.kind != TargetKind::Edge.code() {
            national.max_on_cluster += 1;
            continue;
        }
        national.max_on_edge += 1;
        let Some(expected) = expected_number(attr) else {
            continue;
        };
        let (ref_match, name_match, has_number) =
            edge_number_match(graph_attrs, t.target, expected);
        national.edge_has_number += u64::from(has_number);
        national.ref_match += u64::from(ref_match);
        national.name_match += u64::from(name_match);
        national.any_match += u64::from(ref_match || name_match);
    }
    let route_json: BTreeMap<&str, Value> = ROUTE_GROUPS
        .iter()
        .copied()
        .zip(&by_route)
        .map(|(name, acc)| (name, acc.json()))
        .collect();
    let shape_json: BTreeMap<&str, Value> = SHAPE_GROUPS
        .iter()
        .copied()
        .zip(&by_shape)
        .map(|(name, acc)| (name, acc.json()))
        .collect();
    let mut both_json = Vec::new();
    for (r, row) in by_both.iter().enumerate() {
        for (s, acc) in row.iter().enumerate() {
            if acc.total > 0 {
                let mut value = acc.json();
                let object = value.as_object_mut().expect("オブジェクト");
                object.insert("route_group".to_owned(), json!(ROUTE_GROUPS[r]));
                object.insert("shape_group".to_owned(), json!(SHAPE_GROUPS[s]));
                both_json.push(value);
            }
        }
    }
    json!({
        "overall": overall.json(),
        "by_route_group": route_json,
        "by_shape_group": shape_json,
        "by_route_group_and_shape_group": both_json,
        "max_target_edge_osm_class": by_osm_class,
        "accidents_by_direct_target_count": {
            "1": multi_target[1], "2": multi_target[2], "3": multi_target[3],
        },
        "accidents_with_mirror": mirror_accidents,
        "route_number_match": {
            "national_route": national.json(),
            "note": "最大の重みの割り当てが辺で、その辺のrefの数字（またはnameの「国道N号」）が、国道の事故の路線コードの上4桁（国道番号）と一致した割合。主要地方道・一般都道府県道の路線コードは通し番号で県道番号ではないため、番号の照合はしない",
        },
    })
}

/// 辺の `ref`・`name` が、期待する番号と一致するか。`(refの数字と一致, nameの番号と一致, 番号を持つか)`。
fn edge_number_match(attrs: &GraphAttrs, edge: EdgeId, expected: u32) -> (bool, bool, bool) {
    let prefixes: &[&str] = &["国道"];
    let (mut ref_match, mut name_match, mut any) = (false, false, false);
    if let Some(&id) = attrs.ref_idx.get(edge as usize) {
        numeric_tokens(attrs.string(id), &mut |n| {
            any = true;
            ref_match |= n == expected;
        });
    }
    if let Some(&id) = attrs.name_idx.get(edge as usize) {
        named_numbers(attrs.string(id), prefixes, &mut |n| {
            any = true;
            name_match |= n == expected;
        });
    }
    (ref_match, name_match, any)
}

/// `class` の名前（OSMの `highway` の値）。
struct HighwayClassName;

impl HighwayClassName {
    fn of(class: u8) -> String {
        crate::domain::routing::graph::HighwayClass::from_code(class)
            .map_or_else(|| format!("class{class}"), |c| c.highway_tag().to_owned())
    }
}

/// 道路形状が欠損・未知か（属性の表のうち、道路形状だけ）。
pub fn shape_missing(attr: &AccidentAttr) -> bool {
    attr.road_shape == MISSING_U8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::routing::accident::AssignTarget;
    use crate::domain::routing::graph::HighwayClass;
    use crate::domain::routing::prep::assign::testkit::{Spec, at_m, attr, world};
    use crate::domain::routing::prep::assign::{AssignParams, SnapInput, snap_accidents};

    /// 辺0・1は北に100m離れた国道（refが16と246）。辺2は辺0から10m北の高速。
    /// 事故：0＝国道16を辺0に、1＝国道16を辺1に、2＝市町村道を辺0に、3＝国道16・道路形状0、
    /// 4＝国道16・遠い、5＝高速を辺2に。
    fn small() -> (testkit_world::World, Vec<f64>, Vec<f64>, Vec<AccidentAttr>) {
        let w = world(
            &[
                (0.0, 0.0),
                (0.0, 200.0),
                (100.0, 0.0),
                (100.0, 200.0),
                (110.0, 0.0),
                (110.0, 200.0),
            ],
            &[
                Spec::new(0, 1, HighwayClass::Trunk).with_ref("16"),
                Spec::new(2, 3, HighwayClass::Trunk).with_ref("246"),
                Spec::new(4, 5, HighwayClass::Motorway),
            ],
            &[],
            &[],
        );
        let points = [
            at_m(-2.0, 50.0),
            at_m(98.0, 50.0),
            at_m(-3.0, 60.0),
            at_m(1.0, 70.0),
            at_m(500.0, 500.0),
            at_m(112.0, 50.0),
        ];
        let lat = points.iter().map(|c| f64::from(c.lat_e7) / 1e7).collect();
        let lon = points.iter().map(|c| f64::from(c.lon_e7) / 1e7).collect();
        let attrs = vec![
            attr(16, 14),
            attr(16, 14),
            attr(3000, 14),
            attr(16, 0),
            attr(16, 14),
            attr(5100, 14),
        ];
        (w, lat, lon, attrs)
    }

    use crate::domain::routing::prep::assign::testkit as testkit_world;

    fn snapped() -> (testkit_world::World, Assignment, Vec<AccidentAttr>) {
        let (w, lat, lon, attrs) = small();
        let input = SnapInput {
            graph: w.graph.parts(),
            graph_attrs: &w.attrs,
            topology: &w.topology,
            lat: &lat,
            lon: &lon,
            accident_attrs: &attrs,
        };
        let result = snap_accidents(&input, &AssignParams::default(), &|_| {}).unwrap();
        (w, result.assignment, attrs)
    }

    #[test]
    fn verify_accepts_a_correct_assignment_and_counts_it() {
        let (w, a, attrs) = snapped();
        let stats = verify_assignment(&a, w.graph.parts(), &w.topology, &attrs).unwrap();
        assert_eq!(
            (stats.accidents, stats.assigned, stats.unassigned),
            (6, 4, 2)
        );
        assert_eq!(stats.motorway_coded_assigned, 1);
        assert_eq!(stats.edge_targets, 4);
        assert_eq!(a.targets_of(5)[0].target, 2, "高速のコードの事故は高速の辺");
        assert_eq!(a.targets_of(0)[0].target, 0);
        assert_eq!(a.targets_of(2)[0].target, 0, "高速の辺ではなく国道の辺");
    }

    #[test]
    fn verify_rejects_a_motorway_mismatch_in_either_direction() {
        let (w, a, attrs) = snapped();
        // 非高速のコードの事故を高速の辺に載せる
        let mut bad = a.clone();
        let slot = bad.offsets[2] as usize;
        bad.targets[slot].target = 2;
        let err = verify_assignment(&bad, w.graph.parts(), &w.topology, &attrs).unwrap_err();
        assert!(err.contains("一致しません"), "{err}");
        // 高速のコードの事故を普通の辺に載せる
        let mut bad = a.clone();
        let slot = bad.offsets[5] as usize;
        bad.targets[slot].target = 0;
        let err = verify_assignment(&bad, w.graph.parts(), &w.topology, &attrs).unwrap_err();
        assert!(err.contains("一致しません"), "{err}");
    }

    #[test]
    fn verify_rejects_other_violations() {
        let (w, a, attrs) = snapped();
        // 位置が辺の長さを超える
        let mut bad = a.clone();
        bad.targets[0].pos_dm = 1_000_000;
        assert!(verify_assignment(&bad, w.graph.parts(), &w.topology, &attrs).is_err());
        // 道路形状0の事故に割り当てがある
        let mut bad = a.clone();
        let target = AssignTarget {
            target: 0,
            pos_dm: 5,
            weight: 1.0,
            kind: 0,
            flags: 0,
            reserved: 0,
        };
        // 事故3（道路形状0）に割り当てを足し、UNASSGNから外す
        let at = bad.offsets[3] as usize;
        bad.targets.insert(at, target);
        for o in &mut bad.offsets[4..] {
            *o += 1;
        }
        bad.unassigned.retain(|r| r.accident != 3);
        assert!(verify_assignment(&bad, w.graph.parts(), &w.topology, &attrs).is_err());
        // 理由が道路形状と対応しない
        let mut bad = a.clone();
        for r in &mut bad.unassigned {
            if r.accident == 3 {
                r.reason = UnassignedReason::NoCandidate;
            }
        }
        assert!(verify_assignment(&bad, w.graph.parts(), &w.topology, &attrs).is_err());
        // 事故の数の不一致
        assert!(verify_assignment(&a, w.graph.parts(), &w.topology, &attrs[..5]).is_err());
        // 範囲外の辺
        let mut bad = a.clone();
        bad.targets[0].target = 99;
        assert!(verify_assignment(&bad, w.graph.parts(), &w.topology, &attrs).is_err());
    }

    #[test]
    fn verify_requires_the_mirror_of_a_twin() {
        let w = world(
            &[(0.0, 0.0), (0.0, 200.0), (12.0, 200.0), (12.0, 0.0)],
            &[
                Spec::new(0, 1, HighwayClass::Trunk).oneway(),
                Spec::new(2, 3, HighwayClass::Trunk).oneway(),
            ],
            &[],
            &[(0, 1)],
        );
        let p = at_m(2.0, 50.0);
        let (lat, lon) = ([f64::from(p.lat_e7) / 1e7], [f64::from(p.lon_e7) / 1e7]);
        let attrs = vec![attr(16, 14)];
        let input = SnapInput {
            graph: w.graph.parts(),
            graph_attrs: &w.attrs,
            topology: &w.topology,
            lat: &lat,
            lon: &lon,
            accident_attrs: &attrs,
        };
        let a = snap_accidents(&input, &AssignParams::default(), &|_| {})
            .unwrap()
            .assignment;
        assert_eq!(a.targets.len(), 2);
        let stats = verify_assignment(&a, w.graph.parts(), &w.topology, &attrs).unwrap();
        assert_eq!(stats.mirror_targets, 1);
        // 写しがない
        let mut bad = a.clone();
        bad.targets.pop();
        bad.offsets[1] = 1;
        let err = verify_assignment(&bad, w.graph.parts(), &w.topology, &attrs).unwrap_err();
        assert!(err.contains("写し"), "{err}");
        // 写しの重みが元と違う
        let mut bad = a.clone();
        bad.targets[1].weight = 0.5;
        assert!(verify_assignment(&bad, w.graph.parts(), &w.topology, &attrs).is_err());
        // 元の候補がない写し
        let mut bad = a.clone();
        bad.targets[0].flags = 1;
        assert!(verify_assignment(&bad, w.graph.parts(), &w.topology, &attrs).is_err());
    }

    #[test]
    fn summary_reports_ratios_reasons_and_national_route_matches() {
        let (w, a, attrs) = snapped();
        let s = summarize(&a, &attrs, w.graph.parts(), &w.attrs);
        let overall = &s["overall"];
        assert_eq!(overall["accidents"], 6);
        assert_eq!(overall["assigned"], 4);
        assert_eq!(overall["unassigned"], 2);
        assert_eq!(overall["unassigned_by_reason"]["1_no_candidate"], 1);
        assert_eq!(
            overall["unassigned_by_reason"]["4_general_traffic_place"],
            1
        );
        assert!((overall["assigned_ratio"].as_f64().unwrap() - 4.0 / 6.0).abs() < 1e-12);
        assert_eq!(overall["max_weight_histogram"]["0.95-1.0"], 4);
        // 道路種別：国道は事故0・1・3・4、市町村道は2、高速は5
        let national = &s["by_route_group"]["国道"];
        assert_eq!(national["accidents"], 4);
        assert_eq!(national["assigned"], 2);
        assert_eq!(s["by_route_group"]["市町村道"]["accidents"], 1);
        assert_eq!(s["by_route_group"]["高速・自専道"]["assigned"], 1);
        // 道路形状：一般交通の場所は事故3の1件、全部が帰属不明（理由4）
        let general = &s["by_shape_group"]["一般交通の場所"];
        assert_eq!(general["accidents"], 1);
        assert_eq!(
            general["unassigned_by_reason"]["4_general_traffic_place"],
            1
        );
        // 国道の番号：事故0は辺0（ref 16）で一致、事故1は辺1（ref 246）で不一致
        let m = &s["route_number_match"]["national_route"];
        assert_eq!(m["accidents"], 4);
        assert_eq!(m["max_on_edge"], 2);
        assert_eq!(m["max_edge_ref_matches"], 1);
        assert!((m["ref_match_ratio_of_accidents"].as_f64().unwrap() - 0.25).abs() < 1e-12);
        assert!((m["ref_match_ratio_of_max_on_edge"].as_f64().unwrap() - 0.5).abs() < 1e-12);
        assert_eq!(s["max_target_edge_osm_class"]["trunk"], 3);
        assert_eq!(s["max_target_edge_osm_class"]["motorway"], 1);
        // 層別の表は、事故のある組だけ
        let both = s["by_route_group_and_shape_group"].as_array().unwrap();
        let total: u64 = both.iter().map(|b| b["accidents"].as_u64().unwrap()).sum();
        assert_eq!(total, 6);
    }

    #[test]
    fn route_and_shape_groups() {
        assert_eq!(route_group(&attr(16, 14)), 0);
        assert_eq!(route_group(&attr(1010, 14)), 1);
        assert_eq!(route_group(&attr(1600, 14)), 1);
        assert_eq!(route_group(&attr(2100, 14)), 2);
        assert_eq!(route_group(&attr(3100, 14)), 3);
        assert_eq!(route_group(&attr(4100, 14)), 4);
        assert_eq!(route_group(&attr(5100, 14)), 4);
        assert_eq!(route_group(&attr(5600, 14)), 4);
        assert_eq!(route_group(&attr(9900, 14)), 5);
        assert_eq!(route_group(&AccidentAttr::MISSING), 5);
        for (shape, group) in [
            (1, 0),
            (31, 0),
            (7, 1),
            (37, 1),
            (11, 2),
            (14, 2),
            (21, 2),
            (0, 3),
            (99, 4),
        ] {
            assert_eq!(shape_group(&attr(16, shape)), group, "{shape}");
        }
        assert!(shape_missing(&AccidentAttr::MISSING));
    }
}
