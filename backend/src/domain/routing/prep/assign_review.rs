//! P5：人手判定用の抽出と、確認用の1件分のデータ（作業書M2b.5、計画書4.6節）。
//!
//! 道路種別（路線区分のまとめ）×道路形状のまとめで層別に抽出し（[`pick_review`]）、抽出した事故ごとに
//! 位置・属性・候補の辺とクラスタの形状と重みをJSONにする（[`review_record`]）。HTMLの生成は
//! `data/work/M2/make_review.py`（Gitに入れない）が、このJSONから行う。

use serde_json::{Value, json};

use crate::domain::routing::accident::{
    AccidentAttr, Assignment, MISSING_U8, RouteClass, TargetKind, UnassignedReason,
};
use crate::domain::routing::graph::{Coord, EdgeId, HighwayClass, edge_flags};
use crate::domain::routing::prep::assign::{RoadIndex, Scratch, accident_coord, expected_number};
use crate::domain::routing::prep::assign_audit::{
    ROUTE_GROUPS, SHAPE_GROUPS, max_target, route_group, shape_group,
};
use crate::domain::routing::prep::assign_tables::Tables;

/// 抽出した1件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPick {
    pub accident: u32,
    /// 層の名前（例「国道×単路」）。
    pub stratum: String,
}

/// 一般交通の場所（常に帰属不明）に割り当てる件数の割合の分母（件数/30）。
const GENERAL_PLACE_DIVISOR: usize = 30;

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_83EB);
    x ^ (x >> 31)
}

fn hash_key(accident: u32, seed: u64) -> u64 {
    splitmix64(u64::from(accident) ^ splitmix64(seed))
}

/// 層別に `count` 件を抽出する。同じ入力・同じ `seed` から同じ結果になる。
///
/// 層は「道路種別（6区分）×道路形状（交差点・交差点付近・単路）」の18層と、道路種別を問わない
/// 「一般交通の場所」の1層。一般交通の場所は常に帰属不明なので `count/30` 件（最低1件）にとどめ、
/// 残りを他の層に均等に割り振る（事故の少ない層は全件を取り、余りを他の層に回す）。
/// 層の中は、事故IDと `seed` のハッシュが小さい順に取る。返す順序もハッシュ順（層が連続しない）。
pub fn pick_review(attrs: &[AccidentAttr], count: usize, seed: u64) -> Vec<ReviewPick> {
    // 層の番号：道路種別×3 + 道路形状（0〜17）、18＝一般交通の場所
    let general = ROUTE_GROUPS.len() * 3;
    let mut members: Vec<Vec<u32>> = vec![Vec::new(); general + 1];
    for (i, attr) in attrs.iter().enumerate() {
        let slot = match shape_group(attr) {
            s @ 0..=2 => route_group(attr) * 3 + s,
            3 => general,
            _ => continue,
        };
        members[slot].push(i as u32);
    }
    for list in &mut members {
        list.sort_by_key(|&id| (hash_key(id, seed), id));
    }
    let general_quota = (count / GENERAL_PLACE_DIVISOR)
        .max(1)
        .min(members[general].len());
    let mut quota = vec![0_usize; general + 1];
    quota[general] = general_quota;
    // 残りを、事故のある層に均等に割り振る（足りない層は全件を取り、余りを他へ回す）
    let mut remaining = count - general_quota;
    let mut open: Vec<usize> = (0..general).filter(|&s| !members[s].is_empty()).collect();
    while remaining > 0 && !open.is_empty() {
        let base = remaining / open.len();
        let extra = remaining % open.len();
        let mut next_open = Vec::new();
        let mut used = 0;
        for (n, &s) in open.iter().enumerate() {
            let want = base + usize::from(n < extra);
            let room = members[s].len() - quota[s];
            let take = want.min(room);
            quota[s] += take;
            used += take;
            if quota[s] < members[s].len() {
                next_open.push(s);
            }
        }
        if used == 0 {
            break;
        }
        remaining -= used;
        open = next_open;
    }
    let mut picks: Vec<(u64, ReviewPick)> = Vec::new();
    for (slot, list) in members.iter().enumerate() {
        let stratum = if slot == general {
            "一般交通の場所".to_owned()
        } else {
            format!("{}×{}", ROUTE_GROUPS[slot / 3], SHAPE_GROUPS[slot % 3])
        };
        for &id in list.iter().take(quota[slot]) {
            picks.push((
                hash_key(id, seed ^ 0xA5A5),
                ReviewPick {
                    accident: id,
                    stratum: stratum.clone(),
                },
            ));
        }
    }
    picks.sort_by_key(|(key, pick)| (*key, pick.accident));
    picks.into_iter().map(|(_, pick)| pick).collect()
}

/// 事故の列（`accidents.csv.gz`）のうち、確認用に使う項目。
#[derive(Debug, Clone, Copy)]
pub struct AccidentInfo {
    pub lat: f64,
    pub lon: f64,
    pub modes: u8,
    pub period: u8,
    pub weather: u8,
    pub deaths: u16,
    pub injuries: u16,
    pub source_year: u16,
    pub date: u32,
}

/// 路線区分の名前。
pub fn route_class_label(class: RouteClass) -> &'static str {
    match class {
        RouteClass::NationalRoute => "一般国道",
        RouteClass::MajorPrefecturalRoad => "主要地方道－都道府県道",
        RouteClass::MajorMunicipalRoad => "主要地方道－市道",
        RouteClass::PrefecturalRoad => "一般都道府県道",
        RouteClass::MunicipalRoad => "一般市町村道",
        RouteClass::Expressway => "高速自動車国道",
        RouteClass::DesignatedMotorway => "自動車専用道－指定",
        RouteClass::OtherMotorway => "自動車専用道－その他",
        RouteClass::RoadTransportActRoad => "道路運送法上の道路",
        RouteClass::FarmRoad => "農（免）道",
        RouteClass::ForestRoad => "林道",
        RouteClass::PortRoad => "港湾道",
        RouteClass::PrivateRoad => "私道",
        RouteClass::Other => "その他",
        RouteClass::GeneralTrafficOther => "一般の交通の用に供するその他の道路",
    }
}

/// 道路形状の名前（2024年のコード表）。
pub fn road_shape_label(code: u8) -> String {
    match code {
        1 => "交差点－その他",
        31 => "交差点－環状交差点",
        7 => "交差点付近－その他",
        37 => "交差点付近－環状交差点付近",
        11 => "単路－トンネル",
        12 => "単路－橋",
        13 => "単路－カーブ・屈折",
        14 => "単路－その他",
        21 => "踏切－第一種",
        22 => "踏切－第三種",
        23 => "踏切－第四種",
        0 => "一般交通の場所",
        MISSING_U8 => "不明",
        _ => return format!("コード{code}"),
    }
    .to_owned()
}

/// 車道幅員の名前（2024年のコード表）。
pub fn carriageway_width_label(code: u8) -> String {
    match code {
        1 => "単路－3.5m未満",
        2 => "単路－3.5m以上5.5m未満",
        3 => "単路－5.5m以上9.0m未満",
        4 => "単路－9.0m以上13.0m未満",
        5 => "単路－13.0m以上19.5m未満",
        6 => "単路－19.5m以上",
        11 => "交差点－小（5.5m未満）－小",
        14 => "交差点－中（5.5m以上）－小",
        15 => "交差点－中－中",
        17 => "交差点－大（13.0m以上）－小",
        18 => "交差点－大－中",
        19 => "交差点－大－大",
        0 => "一般交通の場所",
        MISSING_U8 => "不明",
        _ => return format!("コード{code}"),
    }
    .to_owned()
}

/// 歩車道区分の名前。
pub fn sidewalk_division_label(code: u8) -> String {
    match code {
        1 => "区分あり－防護柵等",
        2 => "区分あり－縁石・ブロック等",
        3 => "区分あり－路側帯",
        4 => "区分なし",
        MISSING_U8 => "不明",
        _ => return format!("コード{code}"),
    }
    .to_owned()
}

/// 地形の名前。
pub fn terrain_label(code: u8) -> String {
    match code {
        1 => "市街地－人口集中",
        2 => "市街地－その他",
        3 => "非市街地",
        MISSING_U8 => "不明",
        _ => return format!("コード{code}"),
    }
    .to_owned()
}

fn latlon_pair(c: Coord) -> Value {
    json!([f64::from(c.lat_e7) / 1e7, f64::from(c.lon_e7) / 1e7])
}

fn edge_json(index: &RoadIndex<'_>, e: EdgeId) -> Value {
    let parts = index.parts();
    let attrs = index.graph_attrs();
    let record = &parts.edges[e as usize];
    let line: Vec<Value> = {
        let mut points = Vec::new();
        let start = parts.shape_ofs[e as usize] as usize;
        let end = parts.shape_ofs[e as usize + 1] as usize;
        let mut current = parts.node_ll[record.tail as usize];
        points.push(latlon_pair(current));
        for p in &parts.shape[start..end] {
            current = Coord::new(
                current.lat_e7 + i32::from(p.dlat_e7),
                current.lon_e7 + i32::from(p.dlon_e7),
            );
            points.push(latlon_pair(current));
        }
        points.push(latlon_pair(parts.node_ll[record.head as usize]));
        points
    };
    let i = e as usize;
    let mut flags = Vec::new();
    for (bit, name) in [
        (edge_flags::MOTORROAD, "motorroad"),
        (edge_flags::TUNNEL, "tunnel"),
        (edge_flags::BRIDGE, "bridge"),
        (edge_flags::ROUNDABOUT, "roundabout"),
    ] {
        if record.flags & bit != 0 {
            flags.push(name);
        }
    }
    let width_dm = attrs.width_dm.get(i).copied().unwrap_or(0);
    json!({
        "edge": e,
        "line": line,
        "highway": HighwayClass::from_code(record.class).map_or("?", |c| c.highway_tag()),
        "ref": attrs.ref_idx.get(i).map_or("", |&id| attrs.string(id)),
        "name": attrs.name_idx.get(i).map_or("", |&id| attrs.string(id)),
        "way_id": attrs.way_id.get(i).copied().unwrap_or(0),
        "lanes": attrs.lanes.get(i).copied().unwrap_or(0),
        "width_m": if width_dm == 0 { Value::Null } else { json!(f64::from(width_dm) / 10.0) },
        "layer": attrs.layer.get(i).copied().unwrap_or(0),
        "length_m": f64::from(record.length_dm) / 10.0,
        "flags": flags,
        "twins": index.topology().twins_of(e).collect::<Vec<_>>(),
    })
}

/// 確認用の1件分。割り当て（`assignment`）と、同じ規則で再計算した候補の一覧が食い違えばエラー。
#[allow(clippy::too_many_arguments)]
pub fn review_record(
    index: &RoadIndex<'_>,
    tables: &Tables,
    assignment: &Assignment,
    pick: &ReviewPick,
    attr: &AccidentAttr,
    info: &AccidentInfo,
) -> Result<Value, String> {
    let id = pick.accident as usize;
    let coord = accident_coord(info.lat, info.lon);
    let mut scratch = Scratch::default();
    let outcome = index.assign_one(tables, coord, attr, &mut scratch, true);
    let stored = assignment.targets_of(id);
    if outcome.targets.as_slice() != stored {
        return Err(format!(
            "事故{id}：確認用の再計算がassignment.binの割り当てと一致しません"
        ));
    }
    let topology = index.topology();
    let mut targets = Vec::new();
    for t in stored {
        let mut value = match TargetKind::from_code(t.kind) {
            Some(TargetKind::Edge) => {
                let mut v = edge_json(index, t.target);
                v["kind"] = json!("edge");
                v
            }
            _ => {
                let cluster = &topology.clusters[t.target as usize];
                let nodes: Vec<Value> = topology
                    .nodes_of_cluster(t.target)
                    .iter()
                    .map(|&n| latlon_pair(index.parts().node_ll[n as usize]))
                    .collect();
                json!({
                    "kind": "cluster",
                    "cluster": t.target,
                    "center": latlon_pair(Coord::new(cluster.lat_e7, cluster.lon_e7)),
                    "nodes": nodes,
                    "top_class": HighwayClass::from_code(cluster.top_class).map_or("?", |c| c.highway_tag()),
                    "branches": cluster.branches,
                    "signal": cluster.flags & 1 != 0,
                })
            }
        };
        value["weight"] = json!(t.weight);
        value["mirror"] = json!(t.is_mirror());
        value["pos_dm"] = if t.pos_dm == u32::MAX {
            Value::Null
        } else {
            json!(t.pos_dm)
        };
        if let Some(detail) = &outcome.detail {
            let dist = detail
                .edges
                .iter()
                .find(|d| TargetKind::Edge.code() == t.kind && d.edge == t.target)
                .map(|d| d.dist_m)
                .or_else(|| {
                    detail
                        .clusters
                        .iter()
                        .find(|d| TargetKind::Cluster.code() == t.kind && d.cluster == t.target)
                        .map(|d| d.dist_m)
                });
            value["dist_m"] = dist.map_or(Value::Null, |d| json!(d));
        }
        targets.push(value);
    }
    // 割り当てなかった候補（近い順に最大8本）と、規則で外れた候補（近い順に最大6本）
    let mut others = Vec::new();
    let mut excluded = Vec::new();
    if let Some(detail) = &outcome.detail {
        let mut rest: Vec<_> = detail
            .edges
            .iter()
            .filter(|d| d.assigned.is_none())
            .collect();
        rest.sort_by(|a, b| a.dist_m.total_cmp(&b.dist_m).then(a.edge.cmp(&b.edge)));
        for d in rest.into_iter().take(8) {
            let mut v = edge_json(index, d.edge);
            v["dist_m"] = json!(d.dist_m);
            v["share"] = json!(d.share);
            // 双子でつながる組の代表の辺（割り当て先は代表の辺に載る）
            v["group"] = json!(d.group);
            others.push(v);
        }
        let mut dropped = detail.excluded_edges.clone();
        dropped.sort_by(|a, b| a.dist_m.total_cmp(&b.dist_m).then(a.edge.cmp(&b.edge)));
        for d in dropped.into_iter().take(6) {
            let mut v = edge_json(index, d.edge);
            v["dist_m"] = json!(d.dist_m);
            excluded.push(v);
        }
    }
    let max = max_target(assignment, id);
    let reason = if stored.is_empty() {
        Some(
            assignment
                .unassigned
                .binary_search_by_key(&pick.accident, |r| r.accident)
                .ok()
                .map(|i| assignment.unassigned[i].reason),
        )
        .flatten()
    } else {
        None
    };
    let route = attr.route_class_kind();
    let route_label = route.map_or("不明", route_class_label);
    let route_code = if attr.route_upper == crate::domain::routing::accident::MISSING_U16 {
        "不明".to_owned()
    } else {
        format!("{:04}-{}", attr.route_upper, attr.route_lower)
    };
    Ok(json!({
        "id": pick.accident,
        "stratum": pick.stratum,
        "lat": info.lat,
        "lon": info.lon,
        "max_weight": max.map(|t| t.weight),
        "unassigned_reason": reason.map(|r| json!({
            "code": r.code(),
            "label": reason_label(r),
        })),
        "accident": {
            "source_year": info.source_year,
            "date": info.date,
            "period": info.period,
            "weather": info.weather,
            "modes": info.modes,
            "deaths": info.deaths,
            "injuries": info.injuries,
        },
        "attrs": {
            "route_code": route_code,
            "route_class": route_label,
            "route_number": expected_number(attr),
            "road_shape": road_shape_label(attr.road_shape),
            "road_shape_code": attr.road_shape,
            "carriageway_width": carriageway_width_label(attr.carriageway_width),
            "sidewalk_division": sidewalk_division_label(attr.sidewalk_division),
            "terrain": terrain_label(attr.terrain),
            "motorway_code": attr.is_motorway_code(),
        },
        "targets": targets,
        "others": others,
        "excluded": excluded,
    }))
}

/// 帰属不明の理由の説明。
pub fn reason_label(reason: UnassignedReason) -> &'static str {
    match reason {
        UnassignedReason::NoCandidate => {
            "候補なし（35m以内に車道の辺がなく、交差点の事故で30m以内にクラスタもない）"
        }
        UnassignedReason::MotorwayCodeMismatch => {
            "高速・自専道のコードの不一致（候補はあるが、条件で全部外れた）"
        }
        UnassignedReason::InsufficientWeight => "重みの不足（正規化後、0.1以上の候補がない）",
        UnassignedReason::GeneralTrafficPlace => "一般交通の場所（道路形状0）",
        UnassignedReason::Other => "その他",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::routing::graph::HighwayClass;
    use crate::domain::routing::graph::synthetic::Rng;
    use crate::domain::routing::prep::assign::testkit::{Spec, at_m, attr, world};
    use crate::domain::routing::prep::assign::{AssignParams, SnapInput, snap_accidents};
    use std::collections::{BTreeMap, BTreeSet};

    fn random_attrs(count: usize) -> Vec<AccidentAttr> {
        let mut rng = Rng::new(9);
        let routes = [16, 1010, 2100, 3100, 3100, 3100, 4100, 5100, 9900];
        let shapes = [1, 1, 7, 11, 12, 13, 14, 14, 0, 31, 21, 37];
        (0..count)
            .map(|_| attr(*rng.pick(&routes), *rng.pick(&shapes)))
            .collect()
    }

    #[test]
    fn picks_are_stratified_deterministic_and_unique() {
        let attrs = random_attrs(20_000);
        let picks = pick_review(&attrs, 200, 1);
        assert_eq!(picks.len(), 200);
        assert_eq!(
            picks
                .iter()
                .map(|p| p.accident)
                .collect::<BTreeSet<_>>()
                .len(),
            200
        );
        assert_eq!(picks, pick_review(&attrs, 200, 1));
        assert_ne!(picks, pick_review(&attrs, 200, 2));
        let mut by_stratum: BTreeMap<&str, usize> = BTreeMap::new();
        for p in &picks {
            *by_stratum.entry(p.stratum.as_str()).or_default() += 1;
            // 層の名前と、事故の属性が対応する
            let a = &attrs[p.accident as usize];
            if p.stratum == "一般交通の場所" {
                assert_eq!(a.road_shape, 0);
            } else {
                let (route, shape) = p.stratum.split_once('×').unwrap();
                assert_eq!(ROUTE_GROUPS[route_group(a)], route);
                assert_eq!(SHAPE_GROUPS[shape_group(a)], shape);
            }
        }
        // 一般交通の場所は count/30 = 6件。残りは事故のある層に均等（194件を18層：10か11件）
        assert_eq!(by_stratum["一般交通の場所"], 6);
        let others: Vec<usize> = by_stratum
            .iter()
            .filter(|(k, _)| **k != "一般交通の場所")
            .map(|(_, &n)| n)
            .collect();
        assert!(others.iter().all(|&n| n == 10 || n == 11), "{by_stratum:?}");
        // 存在する層はすべて入る（高速・自専道×交差点なども）
        assert!(by_stratum.keys().any(|k| k.starts_with("高速・自専道×")));
        // 返す順序は層ごとに連続しない
        let first: Vec<&str> = picks.iter().take(12).map(|p| p.stratum.as_str()).collect();
        assert!(first.iter().collect::<BTreeSet<_>>().len() > 3, "{first:?}");
    }

    #[test]
    fn small_strata_are_taken_whole_and_the_rest_is_redistributed() {
        // 高速・自専道の事故が3件しかない
        let mut attrs = random_attrs(5_000);
        for a in &mut attrs {
            if a.is_motorway_code() {
                *a = attr(3100, a.road_shape);
            }
        }
        for a in attrs.iter_mut().take(3) {
            *a = attr(5100, 14);
        }
        let picks = pick_review(&attrs, 200, 1);
        assert_eq!(picks.len(), 200);
        let motorway = picks
            .iter()
            .filter(|p| p.stratum.starts_with("高速・自専道"))
            .count();
        assert_eq!(motorway, 3);
        // 全事故が足りなければ、取れるだけ
        let few = random_attrs(40);
        let picks = pick_review(&few, 200, 1);
        assert!(picks.len() <= 40 && !picks.is_empty());
        assert_eq!(
            picks
                .iter()
                .map(|p| p.accident)
                .collect::<BTreeSet<_>>()
                .len(),
            picks.len()
        );
    }

    #[test]
    fn a_review_record_describes_targets_with_geometry_and_attributes() {
        let w = world(
            &[
                (0.0, 0.0),
                (0.0, 200.0),
                (10.0, 0.0),
                (10.0, 200.0),
                (200.0, 0.0),
                (200.0, 200.0),
            ],
            &[
                Spec::new(0, 1, HighwayClass::Trunk).with_ref("16"),
                Spec::new(2, 3, HighwayClass::Residential),
                Spec::new(4, 5, HighwayClass::Motorway),
            ],
            &[],
            &[],
        );
        let points = [at_m(2.0, 50.0), at_m(300.0, 50.0)];
        let lat: Vec<f64> = points.iter().map(|c| f64::from(c.lat_e7) / 1e7).collect();
        let lon: Vec<f64> = points.iter().map(|c| f64::from(c.lon_e7) / 1e7).collect();
        let mut first = attr(16, 14);
        first.carriageway_width = 3;
        first.sidewalk_division = 2;
        first.terrain = 1;
        let attrs = vec![first, attr(3000, 14)];
        let input = SnapInput {
            graph: w.graph.parts(),
            graph_attrs: &w.attrs,
            topology: &w.topology,
            lat: &lat,
            lon: &lon,
            accident_attrs: &attrs,
        };
        let result = snap_accidents(&input, &AssignParams::default(), &|_| {}).unwrap();
        let index = w.index();
        let info = |i: usize| AccidentInfo {
            lat: lat[i],
            lon: lon[i],
            modes: 4,
            period: 1,
            weather: 2,
            deaths: 0,
            injuries: 1,
            source_year: 2024,
            date: 20240101,
        };
        let pick = |i: u32| ReviewPick {
            accident: i,
            stratum: "国道×単路".to_owned(),
        };
        let record = review_record(
            &index,
            &result.tables,
            &result.assignment,
            &pick(0),
            &attrs[0],
            &info(0),
        )
        .unwrap();
        assert_eq!(record["id"], 0);
        assert_eq!(record["attrs"]["route_class"], "一般国道");
        assert_eq!(record["attrs"]["route_code"], "0016-0");
        assert_eq!(record["attrs"]["road_shape"], "単路－その他");
        assert_eq!(
            record["attrs"]["carriageway_width"],
            "単路－5.5m以上9.0m未満"
        );
        assert_eq!(
            record["attrs"]["sidewalk_division"],
            "区分あり－縁石・ブロック等"
        );
        assert_eq!(record["attrs"]["terrain"], "市街地－人口集中");
        let targets = record["targets"].as_array().unwrap();
        assert!(!targets.is_empty());
        assert_eq!(targets[0]["kind"], "edge");
        assert_eq!(targets[0]["highway"], "trunk");
        assert_eq!(targets[0]["ref"], "16");
        assert!(targets[0]["line"].as_array().unwrap().len() >= 2);
        assert!(targets[0]["weight"].as_f64().unwrap() > 0.5);
        assert!(record["max_weight"].as_f64().unwrap() > 0.5);
        assert!(record["unassigned_reason"].is_null());
        // 規則で外れた高速の辺が、除外の一覧に入る
        assert_eq!(
            record["excluded"].as_array().unwrap().len(),
            0,
            "200m離れている"
        );
        // 候補がない事故は、理由つき
        let record = review_record(
            &index,
            &result.tables,
            &result.assignment,
            &pick(1),
            &attrs[1],
            &info(1),
        )
        .unwrap();
        assert!(record["targets"].as_array().unwrap().is_empty());
        assert_eq!(record["unassigned_reason"]["code"], 1);
        assert!(record["max_weight"].is_null());
        // 割り当てと食い違う入力は拒否する
        let mut tampered = result.assignment.clone();
        tampered.targets[0].weight = 0.5;
        assert!(
            review_record(
                &index,
                &result.tables,
                &tampered,
                &pick(0),
                &attrs[0],
                &info(0)
            )
            .is_err()
        );
    }

    #[test]
    fn a_review_record_lists_every_twin_and_the_group_of_each_candidate() {
        // 東行きの長い辺（辺0）と、反対側の2本に分かれた辺（辺1・2：東0〜100m、100〜200m）。辺0が辺1・2の双子
        let w = world(
            &[
                (0.0, 0.0),
                (0.0, 200.0),
                (12.0, 100.0),
                (12.0, 0.0),
                (12.0, 200.0),
                (12.0, 100.0),
            ],
            &[
                Spec::new(0, 1, HighwayClass::Trunk).oneway().with_ref("16"),
                Spec::new(2, 3, HighwayClass::Trunk).oneway().with_ref("16"),
                Spec::new(4, 5, HighwayClass::Trunk).oneway().with_ref("16"),
            ],
            &[],
            &[(0, 1), (0, 2)],
        );
        let p = at_m(2.0, 90.0);
        let (lat, lon) = (
            vec![f64::from(p.lat_e7) / 1e7],
            vec![f64::from(p.lon_e7) / 1e7],
        );
        let attrs = vec![attr(16, 14)];
        let input = SnapInput {
            graph: w.graph.parts(),
            graph_attrs: &w.attrs,
            topology: &w.topology,
            lat: &lat,
            lon: &lon,
            accident_attrs: &attrs,
        };
        let result = snap_accidents(&input, &AssignParams::default(), &|_| {}).unwrap();
        let index = w.index();
        let info = AccidentInfo {
            lat: lat[0],
            lon: lon[0],
            modes: 4,
            period: 1,
            weather: 2,
            deaths: 0,
            injuries: 1,
            source_year: 2024,
            date: 20240101,
        };
        let pick = ReviewPick {
            accident: 0,
            stratum: "国道×単路".to_owned(),
        };
        let record = review_record(
            &index,
            &result.tables,
            &result.assignment,
            &pick,
            &attrs[0],
            &info,
        )
        .unwrap();
        let targets = record["targets"].as_array().unwrap();
        // 載るのは辺0（直接）。写しは辺1の1本。辺2は同じ組の候補で、割り当てなし
        assert_eq!(targets.len(), 2);
        assert_eq!(
            (targets[0]["edge"].clone(), targets[0]["mirror"].clone()),
            (json!(0), json!(false))
        );
        assert_eq!(
            (targets[1]["edge"].clone(), targets[1]["mirror"].clone()),
            (json!(1), json!(true))
        );
        assert_eq!(targets[0]["twins"], json!([1, 2]));
        assert_eq!(targets[1]["twins"], json!([0]));
        let others = record["others"].as_array().unwrap();
        assert_eq!(others.len(), 1);
        assert_eq!(others[0]["edge"], 2);
        assert_eq!(others[0]["group"], 0, "辺2は辺0の組に属する");
    }

    #[test]
    fn labels_cover_the_codebook_codes() {
        for code in [0, 1, 7, 11, 12, 13, 14, 21, 22, 23, 31, 37] {
            assert!(!road_shape_label(code).starts_with("コード"), "{code}");
        }
        for code in [0, 1, 2, 3, 4, 5, 6, 11, 14, 15, 17, 18, 19] {
            assert!(
                !carriageway_width_label(code).starts_with("コード"),
                "{code}"
            );
        }
        for code in 1..=4 {
            assert!(!sidewalk_division_label(code).starts_with("コード"));
        }
        for code in 1..=3 {
            assert!(!terrain_label(code).starts_with("コード"));
        }
        assert_eq!(road_shape_label(MISSING_U8), "不明");
        for class in RouteClass::ALL {
            assert!(!route_class_label(class).is_empty());
        }
    }
}
