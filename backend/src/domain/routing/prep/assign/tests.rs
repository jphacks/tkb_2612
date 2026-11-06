use super::testkit::{Spec, assign, at_m, attr, world};
use super::*;
use crate::domain::routing::graph::synthetic::{RandomGraphConfig, random_graph};
use crate::domain::routing::graph::{Graph, HighwayClass, edge_flags};
use crate::domain::routing::prep::assign_audit::verify_assignment;
use crate::domain::routing::prep::assign_tables::{
    REF_MATCH, REF_MISMATCH, TableCounts, Tables, ref_group,
};
use crate::domain::routing::prep::attrs::GraphAttrs;
use crate::domain::routing::prep::geom::distance_m;
use crate::domain::routing::prep::topo::{TopologyConfig, build_topology};
use serde_json::Value;

const TRUNK: HighwayClass = HighwayClass::Trunk;
const RES: HighwayClass = HighwayClass::Residential;

/// 水平な道路2本（北に10m離れる）。辺0が南、辺1が北。
fn parallel_world(a: HighwayClass, b: HighwayClass) -> testkit::World {
    world(
        &[(0.0, 0.0), (0.0, 200.0), (10.0, 0.0), (10.0, 200.0)],
        &[Spec::new(0, 1, a), Spec::new(2, 3, b)],
        &[],
        &[],
    )
}

fn weights(outcome: &Outcome) -> Vec<(u8, u32, f32, bool)> {
    outcome
        .targets
        .iter()
        .map(|t| (t.kind, t.target, t.weight, t.is_mirror()))
        .collect()
}

fn direct_sum(outcome: &Outcome) -> f64 {
    outcome
        .targets
        .iter()
        .filter(|t| !t.is_mirror())
        .map(|t| f64::from(t.weight))
        .sum()
}

#[test]
fn a_single_road_takes_the_whole_weight_at_the_projected_position() {
    let w = world(
        &[(0.0, 0.0), (0.0, 200.0)],
        &[Spec::new(0, 1, TRUNK)],
        &[],
        &[],
    );
    let out = assign(&w, at_m(4.0, 50.0), &attr(3000, 14));
    assert_eq!(out.unassigned, None);
    assert_eq!(out.targets.len(), 1);
    let t = out.targets[0];
    assert_eq!((t.kind, t.target, t.weight, t.flags), (0, 0, 1.0, 0));
    // 始点から50m（500dm）、辺の長さは約2000dm
    let length = w.graph.parts().edges[0].length_dm;
    assert!((1995..=2005).contains(&length), "{length}");
    assert!((495..=505).contains(&t.pos_dm), "{}", t.pos_dm);
    let nearest = out.nearest_m.unwrap();
    assert!((nearest - 4.0).abs() < 0.1, "{nearest}");
}

#[test]
fn the_nearer_road_gets_the_larger_weight_and_the_total_is_one() {
    let w = parallel_world(RES, RES);
    let out = assign(&w, at_m(2.2, 100.0), &attr(3000, 14));
    // σ² = 3.5² + 2²（半幅は既定の4mの半分）
    let sigma2 = 3.5_f64 * 3.5 + 4.0;
    let (l0, l1) = (
        -(2.2_f64.powi(2)) / (2.0 * sigma2),
        -(7.8_f64.powi(2)) / (2.0 * sigma2),
    );
    let share0 = 1.0 / (1.0 + (l1 - l0).exp());
    let got = weights(&out);
    assert_eq!(got.len(), 2);
    assert!(
        (f64::from(got[0].2) - share0).abs() < 2e-3,
        "{got:?} {share0}"
    );
    assert!((direct_sum(&out) - 1.0).abs() < 1e-6);
    assert!(
        out.targets
            .windows(2)
            .all(|p| (p[0].kind, p[0].target) < (p[1].kind, p[1].target))
    );
}

#[test]
fn weights_below_the_floor_are_dropped_without_renormalizing() {
    // 北の道路が約0.05の割合になる位置（距離の比で作る）
    let w = parallel_world(RES, RES);
    let out = assign(&w, at_m(0.5, 100.0), &attr(3000, 14));
    // 南の道路 0.5m、北の道路 9.5m：σ²=16.25 → 北の割合は exp(-(90.25-0.25)/32.5)=0.06
    assert_eq!(out.targets.len(), 1);
    assert!(
        out.targets[0].weight > 0.9 && out.targets[0].weight < 1.0,
        "{}",
        out.targets[0].weight
    );
    // 切り捨てた分を戻さない（合計は1未満のまま）
    assert!(direct_sum(&out) < 1.0);
}

#[test]
fn route_class_likelihood_can_overrule_distance() {
    let w = parallel_world(TRUNK, RES);
    let mut counts = TableCounts::default();
    counts.route_by_kind[HighwayClass::Trunk.code() as usize][0] = 1000;
    counts.route_by_kind[HighwayClass::Residential.code() as usize][4] = 1000;
    let tables = Tables::from_counts(counts, 10.0);
    let index = w.index();
    let mut scratch = Scratch::default();
    // 北の道路（住宅街、辺1）に2.2m、南の道路（国道、辺0）に7.8m
    let national = index.assign_one(
        &tables,
        at_m(7.8, 100.0),
        &attr(16, 14),
        &mut scratch,
        false,
    );
    assert_eq!(national.targets[0].target, 0, "{:?}", weights(&national));
    assert!(national.targets[0].weight > 0.9);
    // 同じ位置で市町村道の事故なら、近い住宅街の道路が勝つ
    let municipal = index.assign_one(
        &tables,
        at_m(7.8, 100.0),
        &attr(3000, 14),
        &mut scratch,
        false,
    );
    assert_eq!(municipal.targets[0].target, 1);
    assert!(municipal.targets[0].weight > 0.9);
}

#[test]
fn matching_national_route_number_wins_against_a_nearer_other_route() {
    let w = world(
        &[(0.0, 0.0), (0.0, 200.0), (10.0, 0.0), (10.0, 200.0)],
        &[
            Spec::new(0, 1, TRUNK).with_ref("246"),
            Spec::new(2, 3, TRUNK).with_ref("1;16"),
        ],
        &[],
        &[],
    );
    let mut counts = TableCounts::default();
    counts.ref_states[0][1] = [1, 990, 9];
    let tables = Tables::from_counts(counts, 10.0);
    let mut scratch = Scratch::default();
    let out = w.index().assign_one(
        &tables,
        at_m(2.2, 100.0),
        &attr(16, 14),
        &mut scratch,
        false,
    );
    assert_eq!(out.targets[0].target, 1, "{:?}", weights(&out));
    // 国道以外の事故には番号の照合を使わない（近い辺246が勝つ）
    let out = w.index().assign_one(
        &tables,
        at_m(2.2, 100.0),
        &attr(3000, 14),
        &mut scratch,
        false,
    );
    assert_eq!(out.targets[0].target, 0);
}

#[test]
fn motorway_code_must_agree_with_the_edge_in_both_directions() {
    let w = parallel_world(HighwayClass::Motorway, RES);
    // 高速のコード：高速の辺だけ
    let out = assign(&w, at_m(2.0, 100.0), &attr(5000, 14));
    assert_eq!(weights(&out), vec![(0, 0, 1.0, false)]);
    let out = assign(&w, at_m(2.0, 100.0), &attr(4100, 14));
    assert_eq!(weights(&out), vec![(0, 0, 1.0, false)]);
    // 高速でないコード：近い高速の辺は載せず、住宅街の道路（辺1）だけ
    let out = assign(&w, at_m(2.0, 100.0), &attr(3000, 14));
    assert_eq!(weights(&out), vec![(0, 1, 1.0, false)]);
    // 高速のコードの事故で、高速の辺が35m以内にない
    let w2 = world(
        &[(0.0, 0.0), (0.0, 200.0)],
        &[Spec::new(0, 1, RES)],
        &[],
        &[],
    );
    let out = assign(&w2, at_m(2.0, 100.0), &attr(5000, 14));
    assert!(out.targets.is_empty());
    assert_eq!(out.unassigned, Some(UnassignedReason::MotorwayCodeMismatch));
    // 高速でないコードの事故で、候補が自専道（motorroad）だけ
    let w3 = world(
        &[(0.0, 0.0), (0.0, 200.0)],
        &[Spec::new(0, 1, TRUNK).flags(edge_flags::MOTORROAD)],
        &[],
        &[],
    );
    let out = assign(&w3, at_m(2.0, 100.0), &attr(16, 14));
    assert_eq!(out.unassigned, Some(UnassignedReason::MotorwayCodeMismatch));
    let out = assign(&w3, at_m(2.0, 100.0), &attr(5100, 14));
    assert_eq!(weights(&out), vec![(0, 0, 1.0, false)]);
    // 路線コードが欠損・未知の事故は、高速でないコードの扱い
    let out = assign(&w3, at_m(2.0, 100.0), &AccidentAttr::MISSING);
    assert_eq!(out.unassigned, Some(UnassignedReason::MotorwayCodeMismatch));
}

#[test]
fn no_road_nearby_is_reason_1_and_general_traffic_place_is_reason_4() {
    let w = world(
        &[(0.0, 0.0), (0.0, 200.0)],
        &[Spec::new(0, 1, RES)],
        &[],
        &[],
    );
    let far = assign(&w, at_m(40.0, 100.0), &attr(3000, 14));
    assert_eq!(far.unassigned, Some(UnassignedReason::NoCandidate));
    let near_edge = assign(&w, at_m(34.0, 100.0), &attr(3000, 14));
    assert!(near_edge.unassigned.is_none(), "35m以内");
    let general = assign(&w, at_m(2.0, 100.0), &attr(3000, 0));
    assert_eq!(
        general.unassigned,
        Some(UnassignedReason::GeneralTrafficPlace)
    );
    assert!(general.targets.is_empty());
    // 歩道・横断歩道の辺と、車道でない種類は候補にならない
    let foot = world(
        &[(0.0, 0.0), (0.0, 200.0), (10.0, 0.0), (10.0, 200.0)],
        &[
            Spec::new(0, 1, HighwayClass::Footway),
            Spec::new(2, 3, RES).flags(edge_flags::SIDEWALK),
        ],
        &[],
        &[],
    );
    let out = assign(&foot, at_m(2.0, 100.0), &attr(3000, 14));
    assert_eq!(out.unassigned, Some(UnassignedReason::NoCandidate));
}

#[test]
fn at_most_three_candidates_and_too_little_weight_is_reason_3() {
    // 放射状の道路（ハブに事故がある）。距離がすべて0なので、重みは等しい
    let star = |arms: usize| {
        let mut nodes = vec![(0.0, 0.0)];
        let mut edges = Vec::new();
        for k in 0..arms {
            let a = std::f64::consts::TAU * k as f64 / arms as f64;
            nodes.push((100.0 * a.sin(), 100.0 * a.cos()));
            edges.push(Spec::new(0, k + 1, RES));
        }
        world(&nodes, &edges, &[], &[])
    };
    let out = assign(&star(4), at_m(0.0, 0.0), &attr(3000, 14));
    assert_eq!(out.targets.len(), 3);
    for t in &out.targets {
        assert!((t.weight - 0.25).abs() < 1e-6);
    }
    // 重みが同じなら辺番号の小さい順
    assert_eq!(
        out.targets.iter().map(|t| t.target).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert!((direct_sum(&out) - 0.75).abs() < 1e-6);
    let out = assign(&star(11), at_m(0.0, 0.0), &attr(3000, 14));
    assert!(out.targets.is_empty());
    assert_eq!(out.unassigned, Some(UnassignedReason::InsufficientWeight));
    // 10本なら、重みはちょうど0.1で残る
    let out = assign(&star(10), at_m(0.0, 0.0), &attr(3000, 14));
    assert_eq!(out.targets.len(), 3);
    assert!(out.targets.iter().all(|t| t.weight >= MIN_WEIGHT));
}

#[test]
fn twin_edges_are_one_candidate_and_the_far_twin_gets_a_mirror() {
    // 上りと下りの一方通行（12m離れる）。辺0は東向き、辺1は西向き
    let nodes = [(0.0, 0.0), (0.0, 200.0), (12.0, 200.0), (12.0, 0.0)];
    let edges = [
        Spec::new(0, 1, TRUNK).oneway().with_ref("16"),
        Spec::new(2, 3, TRUNK).oneway().with_ref("16"),
    ];
    let w = world(&nodes, &edges, &[], &[(0, 1)]);
    let out = assign(&w, at_m(2.0, 50.0), &attr(16, 14));
    assert_eq!(out.targets.len(), 2, "{:?}", weights(&out));
    let (direct, mirror) = (out.targets[0], out.targets[1]);
    assert!(!direct.is_mirror() && mirror.is_mirror());
    assert_eq!((direct.target, mirror.target), (0, 1));
    // 組で1つの候補：重みは1で、写しも同じ重み。MIRRORを除いた合計は1以下
    assert_eq!((direct.weight, mirror.weight), (1.0, 1.0));
    assert!((direct_sum(&out) - 1.0).abs() < 1e-9);
    assert!((495..=505).contains(&direct.pos_dm), "{}", direct.pos_dm);
    // 写しの位置は、事故を双子の辺に投影した位置（西向きの辺の始点は東端なので、東端から150m）
    assert!((1495..=1505).contains(&mirror.pos_dm), "{}", mirror.pos_dm);
    // 事故が2本の間（双子のどちらも35m以内）でも、組の重みは1
    let out = assign(&w, at_m(6.0, 50.0), &attr(16, 14));
    assert_eq!(out.targets.len(), 2);
    assert_eq!(out.targets[0].weight, 1.0);
    // 双子が35mより遠くても写す
    let far = world(
        &[(0.0, 0.0), (0.0, 200.0), (50.0, 200.0), (50.0, 0.0)],
        &edges,
        &[],
        &[(0, 1)],
    );
    let out = assign(&far, at_m(2.0, 50.0), &attr(16, 14));
    assert_eq!(out.targets.len(), 2);
    assert!(out.targets[1].is_mirror() && out.targets[1].target == 1);
    // 双子が高速・自専道の規則を満たさないときは写さない
    let mixed = world(
        &nodes,
        &[
            Spec::new(0, 1, TRUNK).oneway(),
            Spec::new(2, 3, HighwayClass::Motorway).oneway(),
        ],
        &[],
        &[(0, 1)],
    );
    let out = assign(&mixed, at_m(2.0, 50.0), &attr(16, 14));
    assert_eq!(out.targets.len(), 1);
    assert!(!out.targets[0].is_mirror());
}

/// 東行きの長い辺（辺0。北0m、東0〜300m）と、その北12mの西行きが3本に分かれた辺（辺1〜3：東0〜100m、
/// 100〜200m、200〜300m）。辺0と辺1・2・3が双子。
fn long_and_cut_twins() -> testkit::World {
    world(
        &[
            (0.0, 0.0),
            (0.0, 300.0),
            (12.0, 100.0),
            (12.0, 0.0),
            (12.0, 200.0),
            (12.0, 100.0),
            (12.0, 300.0),
            (12.0, 200.0),
        ],
        &[
            Spec::new(0, 1, TRUNK).oneway().with_ref("16"),
            Spec::new(2, 3, TRUNK).oneway().with_ref("16"),
            Spec::new(4, 5, TRUNK).oneway().with_ref("16"),
            Spec::new(6, 7, TRUNK).oneway().with_ref("16"),
        ],
        &[],
        &[(0, 1), (0, 2), (0, 3)],
    )
}

#[test]
fn a_mirror_goes_to_the_one_twin_at_the_accident_position() {
    let w = long_and_cut_twins();
    // 長い辺の上（2m）。反対側は辺2（100〜200m）だけが35m以内。写しは辺2の1本（辺1・3には載らない）
    let out = assign(&w, at_m(2.0, 150.0), &attr(16, 14));
    assert_eq!(
        weights(&out),
        vec![(0, 0, 1.0, false), (0, 2, 1.0, true)],
        "{:?}",
        weights(&out)
    );
    // 写しの位置は、辺2の上での位置。辺2は東200mから西へ向かうので、始点（東端）から50m
    let mirror = out.targets[1];
    assert!((495..=505).contains(&mirror.pos_dm), "{}", mirror.pos_dm);
    // 西行きの辺の上（2m）。載るのは近い辺2で、写しは長い辺0の1本
    let out = assign(&w, at_m(10.0, 150.0), &attr(16, 14));
    assert_eq!(
        weights(&out),
        vec![(0, 0, 1.0, true), (0, 2, 1.0, false)],
        "{:?}",
        weights(&out)
    );
    let mirror = out.targets[0];
    assert!((1495..=1505).contains(&mirror.pos_dm), "{}", mirror.pos_dm);
    // 辺1と辺2の継ぎ目（東100m）の近く。辺1・辺2のどちらも35m以内の候補で、1つの組になる。
    // 写しは、事故の投影点が端の外にはみ出さない辺1の1本だけ（辺2は端に丸められる）
    let out = assign(&w, at_m(2.0, 90.0), &attr(16, 14));
    assert_eq!(
        weights(&out),
        vec![(0, 0, 1.0, false), (0, 1, 1.0, true)],
        "{:?}",
        weights(&out)
    );
    // 辺1の端のさらに外（東−30m）でも、写し先は1本（いちばん近い辺1）
    let out = assign(&w, at_m(2.0, -20.0), &attr(16, 14));
    assert_eq!(
        weights(&out),
        vec![(0, 0, 1.0, false), (0, 1, 1.0, true)],
        "{:?}",
        weights(&out)
    );
}

#[test]
fn candidates_joined_through_twins_are_one_group() {
    // 西行き1本（辺1、北12m）を、東行き2本（辺0＝北0m、辺2＝北24m）がはさむ。どちらの東行きも西行きの双子。
    // 東行きどうしは双子でない。3本は1つの組になり、近い辺0（事故は北4m）に載る。写しは西行きの1本
    let nodes = [
        (0.0, 0.0),
        (0.0, 200.0),
        (12.0, 200.0),
        (12.0, 0.0),
        (24.0, 0.0),
        (24.0, 200.0),
        // 住宅街の道路（辺3）：北4m
        (4.0, 0.0),
        (4.0, 200.0),
    ];
    let edges = [
        Spec::new(0, 1, TRUNK).oneway().with_ref("16"),
        Spec::new(2, 3, TRUNK).oneway().with_ref("16"),
        Spec::new(4, 5, TRUNK).oneway().with_ref("16"),
        Spec::new(6, 7, RES),
    ];
    let w = world(&nodes, &edges, &[], &[(0, 1), (1, 2)]);
    let out = assign(&w, at_m(4.0, 100.0), &attr(3000, 14));
    // 辺0〜2の3本の組のスコアの和と、辺3（0m）のスコアで配分する
    let detail = out.detail.as_ref().unwrap();
    let share = |e: u32| detail.edges.iter().find(|d| d.edge == e).unwrap().share;
    let group = share(0) + share(1) + share(2);
    let direct: Vec<_> = out.targets.iter().filter(|t| !t.is_mirror()).collect();
    assert_eq!(direct.len(), 2, "{:?}", weights(&out));
    let road = direct.iter().find(|t| t.target == 3).unwrap();
    assert!((f64::from(road.weight) - share(3)).abs() < 1e-6);
    let pair = direct.iter().find(|t| t.target != 3).unwrap();
    assert_eq!(pair.target, 0, "組の位置は近い辺0");
    assert!((f64::from(pair.weight) - group).abs() < 1e-6);
    // 写しは西行きの辺1だけ（辺2は組の一員で候補だが、載せた辺0の双子ではない）
    let mirrors: Vec<_> = out.targets.iter().filter(|t| t.is_mirror()).collect();
    assert_eq!(mirrors.len(), 1);
    assert_eq!(mirrors[0].target, 1);
    assert_eq!(mirrors[0].weight, pair.weight);
}

#[test]
fn width_uses_the_twin_at_the_accident_position() {
    let mut w = long_and_cut_twins();
    // 長い辺は2車線、西行きの断片は1・3・2車線
    for (e, lanes) in [(0, 2), (1, 1), (2, 3), (3, 2)] {
        w.attrs.lanes[e] = lanes;
    }
    let index = w.index();
    let mut xy = Vec::new();
    let width_at = |e: u32, north: f64, east: f64, xy: &mut Vec<(f64, f64)>| {
        let frame = LocalFrame::new(at_m(north, east));
        index.total_width_m(e, &frame, xy)
    };
    let lane = 3.25;
    // 東100〜200mの位置では、長い辺（2車線）と辺2（3車線）の和
    let got = width_at(0, 2.0, 150.0, &mut xy).unwrap();
    assert!((got - 5.0 * lane).abs() < 1e-9, "{got}");
    // 東0〜100mの位置では、辺1（1車線）との和
    let got = width_at(0, 2.0, 40.0, &mut xy).unwrap();
    assert!((got - 3.0 * lane).abs() < 1e-9, "{got}");
    // 西行きの断片から見ると、双子は長い辺だけ
    let got = width_at(2, 10.0, 150.0, &mut xy).unwrap();
    assert!((got - 5.0 * lane).abs() < 1e-9, "{got}");
    // 双子のない辺は自分の幅員だけ。双子の幅員が不明なら、分かる方の2倍
    let mut single = world(
        &[(0.0, 0.0), (0.0, 200.0)],
        &[Spec::new(0, 1, TRUNK)],
        &[],
        &[],
    );
    single.attrs.lanes[0] = 2;
    let frame = LocalFrame::new(at_m(1.0, 50.0));
    assert_eq!(
        single.index().total_width_m(0, &frame, &mut xy),
        Some(2.0 * lane)
    );
    w.attrs.lanes[2] = 0;
    let index = w.index();
    let frame = LocalFrame::new(at_m(2.0, 150.0));
    assert_eq!(
        index.total_width_m(0, &frame, &mut xy),
        Some(4.0 * lane),
        "双子の幅員が不明なら、分かる方の2倍"
    );
}

#[test]
fn twin_competition_uses_the_pair_not_the_edges() {
    // 上下線の双子（辺0・1）と、その間にある住宅街の道路（辺2）。双子は2本で1つの候補
    let w = world(
        &[
            (0.0, 0.0),
            (0.0, 200.0),
            (12.0, 200.0),
            (12.0, 0.0),
            (6.0, 0.0),
            (6.0, 200.0),
        ],
        &[
            Spec::new(0, 1, RES).oneway(),
            Spec::new(2, 3, RES).oneway(),
            Spec::new(4, 5, RES),
        ],
        &[],
        &[(0, 1)],
    );
    let out = assign(&w, at_m(6.0, 100.0), &attr(3000, 14));
    // 双子の組のスコア = 2本の和。距離は6m・6m、住宅街の道路は0m
    let sigma2 = 3.5_f64 * 3.5 + 4.0;
    let pair = 2.0 * (-(36.0) / (2.0 * sigma2)).exp();
    let share_pair = pair / (pair + 1.0);
    let direct: Vec<_> = out.targets.iter().filter(|t| !t.is_mirror()).collect();
    assert_eq!(direct.len(), 2);
    let pair_target = direct.iter().find(|t| t.target != 2).unwrap();
    assert!((f64::from(pair_target.weight) - share_pair).abs() < 2e-3);
    let mirror = out.targets.iter().find(|t| t.is_mirror()).unwrap();
    assert_eq!(mirror.weight, pair_target.weight);
    assert!(direct_sum(&out) <= 1.0 + 1e-6);
}

#[test]
fn intersection_accidents_go_to_the_cluster_when_one_is_near() {
    // 十字路。ハブ（ノード0）をクラスタにする
    let cross = |class: HighwayClass| {
        world(
            &[
                (0.0, 0.0),
                (100.0, 0.0),
                (-100.0, 0.0),
                (0.0, 100.0),
                (0.0, -100.0),
            ],
            &[
                Spec::new(0, 1, class),
                Spec::new(0, 2, class),
                Spec::new(0, 3, class),
                Spec::new(0, 4, class),
            ],
            &[&[0]],
            &[],
        )
    };
    let w = cross(RES);
    let out = assign(&w, at_m(3.0, 4.0), &attr(3000, 1));
    assert_eq!(out.targets.len(), 1);
    let t = out.targets[0];
    assert_eq!(
        (t.kind, t.target, t.pos_dm, t.weight),
        (1, 0, u32::MAX, 1.0)
    );
    // 環状交差点（31）も同じ
    let out = assign(&w, at_m(3.0, 4.0), &attr(3000, 31));
    assert_eq!(out.targets[0].kind, 1);
    // 交差点付近・単路は、近くにクラスタがあっても辺に割り当てる
    for shape in [7, 37, 13, 14] {
        let out = assign(&w, at_m(3.0, 4.0), &attr(3000, shape));
        assert!(out.targets.iter().all(|t| t.kind == 0), "{shape}");
    }
    // 30mより遠いと、辺の候補に進む
    let out = assign(&w, at_m(60.0, 2.0), &attr(3000, 1));
    assert_eq!(out.targets[0].kind, 0);
    assert_eq!(out.targets[0].target, 0);
    // 辺も35mより遠ければ、理由1
    let out = assign(&w, at_m(60.0, 40.0), &attr(3000, 1));
    assert_eq!(out.unassigned, Some(UnassignedReason::NoCandidate));
    // 高速・自専道の辺だけのクラスタ：高速でないコードの事故は載せない（理由2）、高速のコードなら載る
    let m = cross(HighwayClass::Motorway);
    let out = assign(&m, at_m(3.0, 4.0), &attr(3000, 1));
    assert_eq!(out.unassigned, Some(UnassignedReason::MotorwayCodeMismatch));
    let out = assign(&m, at_m(3.0, 4.0), &attr(5000, 1));
    assert_eq!(out.targets[0].kind, 1);
    // 普通の道路だけのクラスタに、高速のコードの事故は載せない
    let out = assign(&w, at_m(3.0, 4.0), &attr(5000, 1));
    assert_eq!(out.unassigned, Some(UnassignedReason::MotorwayCodeMismatch));
}

#[test]
fn nearer_cluster_wins_among_clusters() {
    // 40m離れた2つの交差点（ノード0・1）
    let w = world(
        &[
            (0.0, 0.0),
            (0.0, 40.0),
            (60.0, 0.0),
            (-60.0, 0.0),
            (60.0, 40.0),
            (-60.0, 40.0),
        ],
        &[
            Spec::new(0, 1, RES),
            Spec::new(0, 2, RES),
            Spec::new(0, 3, RES),
            Spec::new(1, 4, RES),
            Spec::new(1, 5, RES),
        ],
        &[&[0], &[1]],
        &[],
    );
    // 交差点0から5m、交差点1から35m（30mを超える）：交差点0だけ
    let out = assign(&w, at_m(0.0, 5.0), &attr(3000, 1));
    assert_eq!(weights(&out), vec![(1, 0, 1.0, false)]);
    // 交差点0から12m、交差点1から28m：どちらも30m以内。近い方が大きい（σ²=3.5²+10²）
    let out = assign(&w, at_m(0.0, 12.0), &attr(3000, 1));
    let share0 = 1.0 / (1.0 + ((144.0_f64 - 784.0) / (2.0 * (12.25 + 100.0))).exp());
    let got = weights(&out);
    assert_eq!(got.len(), 1, "{got:?}（遠い方は割合が0.1未満で切れる）");
    assert_eq!((got[0].0, got[0].1), (1, 0));
    assert!(
        (f64::from(got[0].2) - share0).abs() < 1e-3,
        "{got:?} {share0}"
    );
    // 真ん中：同じ距離なので0.5ずつ
    let out = assign(&w, at_m(0.0, 20.0), &attr(3000, 1));
    let got = weights(&out);
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got.iter().all(|g| (g.2 - 0.5).abs() < 1e-3));
    assert!(direct_sum(&out) <= 1.0 + 1e-6);
}

#[test]
fn width_and_structure_factors_use_their_tables() {
    // 橋の辺（辺0）とそうでない辺（辺1）が並ぶ。道路形状が橋（12）の事故は橋の辺に寄る
    let w = world(
        &[(0.0, 0.0), (0.0, 200.0), (10.0, 0.0), (10.0, 200.0)],
        &[
            Spec::new(0, 1, RES).flags(edge_flags::BRIDGE),
            Spec::new(2, 3, RES),
        ],
        &[],
        &[],
    );
    let mut counts = TableCounts::default();
    counts.shape_by_struct[1] = [100, 900, 0];
    counts.shape_by_struct[0] = [990, 10, 0];
    let tables = Tables::from_counts(counts, 1.0);
    let mut scratch = Scratch::default();
    // 橋の辺（辺0、北0m）に7.8m、橋でない辺（辺1、北10m）に2.2m。形状が橋（12）なら橋の辺が勝つ
    let out = w.index().assign_one(
        &tables,
        at_m(7.8, 100.0),
        &attr(3000, 12),
        &mut scratch,
        false,
    );
    assert_eq!(out.targets[0].target, 0, "{:?}", weights(&out));
    // 形状が橋でなければ（14）、距離の近い橋でない辺
    let out = w.index().assign_one(
        &tables,
        at_m(7.8, 100.0),
        &attr(3000, 14),
        &mut scratch,
        false,
    );
    assert_eq!(out.targets[0].target, 1, "{:?}", weights(&out));
    // 幅員：辺0は幅25m（区分6）、辺1は幅4m（区分2）。表は、区分と同じコードを出しやすい
    let mut wide = world(
        &[(0.0, 0.0), (0.0, 200.0), (10.0, 0.0), (10.0, 200.0)],
        &[Spec::new(0, 1, RES), Spec::new(2, 3, RES)],
        &[],
        &[],
    );
    wide.attrs.width_dm[0] = 250;
    wide.attrs.width_dm[1] = 40;
    let mut counts = TableCounts::default();
    counts.width[6][5] = 900;
    counts.width[6][1] = 50;
    counts.width[2][1] = 900;
    counts.width[2][5] = 50;
    let tables = Tables::from_counts(counts, 1.0);
    let at = |width_code: u8| {
        let mut a = attr(3000, 14);
        a.carriageway_width = width_code;
        wide.index().assign_one(
            &tables,
            at_m(5.0, 100.0),
            &a,
            &mut Scratch::default(),
            false,
        )
    };
    let by_edge = |out: &Outcome, edge: u32| {
        out.targets
            .iter()
            .find(|t| t.target == edge)
            .map_or(0.0, |t| t.weight)
    };
    // 同じ位置（どちらの辺からも5m）でも、コード6は広い辺0、コード2は狭い辺1に寄る
    assert!(by_edge(&at(6), 0) > 0.9, "{:?}", weights(&at(6)));
    assert!(by_edge(&at(2), 1) > 0.85, "{:?}", weights(&at(2)));
    // 幅員が欠損なら因子を使わない（距離の分散だけの差で、どちらも残る）
    let missing = at(MISSING_U8);
    assert_eq!(missing.targets.len(), 2, "{:?}", weights(&missing));
}

#[test]
fn text_helpers() {
    type Visit<'a> = &'a dyn Fn(&mut dyn FnMut(u32));
    let collect = |f: Visit| {
        let mut out = Vec::new();
        f(&mut |n| out.push(n));
        out
    };
    assert_eq!(
        collect(&|f| numeric_tokens("1;20, 246 ;E4;C1;x7", &mut |n| f(n))),
        vec![1, 20, 246]
    );
    assert!(collect(&|f| numeric_tokens("E4;E48", &mut |n| f(n))).is_empty());
    assert_eq!(
        collect(&|f| named_numbers("国道50号笠間バイパス", &["国道"], &mut |n| f(n))),
        vec![50]
    );
    assert_eq!(
        collect(&|f| named_numbers("国道１６号・国道２４６号", &["国道"], &mut |n| f(n))),
        vec![16, 246]
    );
    // 「号」が続かないもの、数字がないものは対象外
    assert!(collect(&|f| named_numbers("国道バイパス", &["国道"], &mut |n| f(n))).is_empty());
    assert!(collect(&|f| named_numbers("国道16線", &["国道"], &mut |n| f(n))).is_empty());
    for (width, bin) in [
        (3.4, 1),
        (3.5, 2),
        (5.4, 2),
        (5.5, 3),
        (8.9, 3),
        (9.0, 4),
        (13.0, 5),
        (19.5, 6),
    ] {
        assert_eq!(width_bin(width), bin, "{width}");
    }
    assert_eq!(expected_number(&attr(16, 14)), Some(16));
    assert_eq!(
        expected_number(&attr(1090, 14)),
        None,
        "県道は番号を照合しない"
    );
    assert_eq!(expected_number(&attr(0, 14)), None);
    let (d, ratio) = project_origin(&[(-10.0, 3.0), (10.0, 3.0)]);
    assert_eq!((d, ratio), (3.0, 0.5));
}

#[test]
fn observe_counts_only_accidents_with_a_single_candidate() {
    let one = world(
        &[(0.0, 0.0), (0.0, 200.0)],
        &[Spec::new(0, 1, RES)],
        &[],
        &[],
    );
    let index = one.index();
    let mut scratch = Scratch::default();
    let mut counts = TableCounts::default();
    index.observe(at_m(2.0, 100.0), &attr(3000, 14), &mut scratch, &mut counts);
    assert_eq!(counts.unique_edge_accidents, 1);
    assert_eq!(
        counts.route_by_kind[HighwayClass::Residential.code() as usize][4],
        1
    );
    assert_eq!(counts.shape_by_struct[0][0], 1);
    // 距離が20mを超える唯一の候補は数えない
    index.observe(
        at_m(25.0, 100.0),
        &attr(3000, 14),
        &mut scratch,
        &mut counts,
    );
    assert_eq!(counts.unique_edge_accidents, 1);
    // 道路形状0、交差点（クラスタなし）の形状は、形状の表に入れない
    index.observe(at_m(2.0, 100.0), &attr(3000, 1), &mut scratch, &mut counts);
    assert_eq!(counts.unique_edge_accidents, 2);
    assert_eq!(counts.shape_by_struct[0][0], 1);
    index.observe(at_m(2.0, 100.0), &attr(3000, 0), &mut scratch, &mut counts);
    assert_eq!(counts.unique_edge_accidents, 2);

    // 候補が2本あれば数えない
    let two = parallel_world(RES, RES);
    let mut counts = TableCounts::default();
    two.index()
        .observe(at_m(2.0, 100.0), &attr(3000, 14), &mut scratch, &mut counts);
    assert_eq!(counts, TableCounts::default());
    // 高速・自専道の規則で1本に絞られれば数える
    let mixed = parallel_world(HighwayClass::Motorway, RES);
    mixed
        .index()
        .observe(at_m(2.0, 100.0), &attr(3000, 14), &mut scratch, &mut counts);
    assert_eq!(counts.unique_edge_accidents, 1);
    // 双子の2本は1つと数える
    let twins = world(
        &[(0.0, 0.0), (0.0, 200.0), (12.0, 200.0), (12.0, 0.0)],
        &[
            Spec::new(0, 1, TRUNK).oneway(),
            Spec::new(2, 3, TRUNK).oneway(),
        ],
        &[],
        &[(0, 1)],
    );
    let mut counts = TableCounts::default();
    twins
        .index()
        .observe(at_m(2.0, 100.0), &attr(16, 14), &mut scratch, &mut counts);
    assert_eq!(counts.unique_edge_accidents, 1);
    assert_eq!(counts.ref_states[0][1][0], 1, "番号のない国道は中立の状態");
    // 双子でつながる辺（長い1本と、反対側の分かれた辺）も1つと数える
    let cut = long_and_cut_twins();
    let mut counts = TableCounts::default();
    cut.index()
        .observe(at_m(2.0, 90.0), &attr(16, 14), &mut scratch, &mut counts);
    assert_eq!(counts.unique_edge_accidents, 1);
    // 双子でつながらない2本が候補なら数えない
    let mut counts = TableCounts::default();
    parallel_world(TRUNK, TRUNK).index().observe(
        at_m(2.0, 100.0),
        &attr(16, 14),
        &mut scratch,
        &mut counts,
    );
    assert_eq!(counts, TableCounts::default());
    // クラスタの唯一の候補
    let cross = world(
        &[(0.0, 0.0), (100.0, 0.0), (-100.0, 0.0), (0.0, 100.0)],
        &[
            Spec::new(0, 1, TRUNK),
            Spec::new(0, 2, TRUNK),
            Spec::new(0, 3, TRUNK),
        ],
        &[&[0]],
        &[],
    );
    let mut counts = TableCounts::default();
    cross
        .index()
        .observe(at_m(3.0, 4.0), &attr(16, 1), &mut scratch, &mut counts);
    assert_eq!(counts.unique_cluster_accidents, 1);
    assert_eq!(counts.route_by_cluster[TRUNK.code() as usize][0], 1);
    assert_eq!(counts.unique_edge_accidents, 0);
}

fn random_world(
    seed: u64,
) -> (
    Graph,
    GraphAttrs,
    crate::domain::routing::topology::Topology,
) {
    let mut config = RandomGraphConfig::new(80);
    config.extra_edges = 60;
    config.lat_e7 = (350_000_000, 350_020_000);
    config.lon_e7 = (1_390_000_000, 1_390_020_000);
    config.classes = vec![
        HighwayClass::Motorway,
        HighwayClass::MotorwayLink,
        HighwayClass::Trunk,
        HighwayClass::Primary,
        HighwayClass::Residential,
        HighwayClass::Residential,
        HighwayClass::Service,
        HighwayClass::Footway,
        HighwayClass::Track,
    ];
    let graph = random_graph(seed, &config).build();
    let e = graph.edge_count();
    let attrs = GraphAttrs {
        params: Value::Null,
        way_id: vec![0; e],
        node_id: (0..graph.node_count() as i64).collect(),
        ref_idx: vec![0; e],
        name_idx: vec![0; e],
        strtab: vec![0],
        lanes: vec![0; e],
        width_dm: vec![0; e],
        layer: vec![0; e],
    };
    let topology = build_topology(
        graph.parts(),
        &attrs,
        &TopologyConfig {
            scc_min_nodes: 1,
            ..TopologyConfig::default()
        },
    )
    .unwrap();
    (graph, attrs, topology)
}

fn random_accidents(
    graph: &Graph,
    count: usize,
    seed: u64,
) -> (Vec<f64>, Vec<f64>, Vec<AccidentAttr>) {
    use crate::domain::routing::graph::synthetic::Rng;
    let mut rng = Rng::new(seed);
    let (mut lat, mut lon, mut attrs) = (Vec::new(), Vec::new(), Vec::new());
    let routes = [16, 1010, 2100, 3100, 3100, 4100, 5100, 9900, 0];
    let shapes = [1, 7, 11, 12, 13, 14, 14, 0, 31, 21, 37];
    for _ in 0..count {
        let node = graph.node_coord(rng.below(graph.node_count()) as u32);
        lat.push(f64::from(node.lat_e7 + rng.range_i32(-300, 300)) / 1e7);
        lon.push(f64::from(node.lon_e7 + rng.range_i32(-300, 300)) / 1e7);
        attrs.push(attr(*rng.pick(&routes), *rng.pick(&shapes)));
    }
    (lat, lon, attrs)
}

#[test]
fn snap_accidents_is_deterministic_across_thread_counts_and_passes_the_checks() {
    let (graph, attrs, topology) = random_world(11);
    let (lat, lon, accident_attrs) = random_accidents(&graph, 900, 5);
    let input = SnapInput {
        graph: graph.parts(),
        graph_attrs: &attrs,
        topology: &topology,
        lat: &lat,
        lon: &lon,
        accident_attrs: &accident_attrs,
    };
    let params = AssignParams {
        chunk: 37,
        ..AssignParams::default()
    };
    let run = |threads: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| snap_accidents(&input, &params, &|_| {}).unwrap())
    };
    let (one, four) = (run(1), run(4));
    assert_eq!(one.assignment, four.assignment);
    assert_eq!(one.tables, four.tables);
    let assignment = &one.assignment;
    assert_eq!(assignment.accident_count(), 900);
    assignment
        .validate(graph.edge_count(), topology.cluster_count())
        .unwrap();
    let stats = verify_assignment(assignment, graph.parts(), &topology, &accident_attrs).unwrap();
    assert_eq!(stats.accidents, 900);
    assert_eq!(stats.assigned + stats.unassigned, 900);
    assert!(
        stats.assigned > 300 && stats.cluster_targets > 0 && stats.mirror_targets > 0
            || stats.mirror_targets == 0
    );
    // 道路形状0の事故は理由4だけ
    for r in &assignment.unassigned {
        let shape = accident_attrs[r.accident as usize].road_shape;
        assert_eq!(
            shape == 0,
            r.reason == UnassignedReason::GeneralTrafficPlace
        );
    }
    // PARAMSに規則と件数がある
    let p = &assignment.params;
    assert_eq!(p["sigma0_m"], 3.5);
    assert_eq!(p["edge_radius_m"], 35.0);
    assert_eq!(p["cluster_radius_m"], 30.0);
    assert_eq!(p["accidents"], 900);
    assert_eq!(p["unassigned"], assignment.unassigned.len());
    assert!(p["tables"]["route_by_kind"]["probabilities"].is_array());
    // 高速のコードの事故が載っている（検査が空振りでない）
    assert!(stats.motorway_coded_assigned > 0);
}

#[test]
fn snap_accidents_rejects_mismatched_inputs() {
    let (graph, attrs, topology) = random_world(3);
    let (lat, lon, accident_attrs) = random_accidents(&graph, 10, 1);
    let bad = SnapInput {
        graph: graph.parts(),
        graph_attrs: &attrs,
        topology: &topology,
        lat: &lat[..9],
        lon: &lon,
        accident_attrs: &accident_attrs,
    };
    assert!(snap_accidents(&bad, &AssignParams::default(), &|_| {}).is_err());
    let mut short_attrs = attrs.clone();
    short_attrs.lanes.pop();
    let bad = SnapInput {
        graph: graph.parts(),
        graph_attrs: &short_attrs,
        topology: &topology,
        lat: &lat,
        lon: &lon,
        accident_attrs: &accident_attrs,
    };
    assert!(snap_accidents(&bad, &AssignParams::default(), &|_| {}).is_err());
}

#[test]
fn likelihood_false_gives_the_same_weight_at_the_same_distance() {
    // 同じ種別の道路が12m離れて並ぶ。事故は真ん中（どちらからも6m）。refだけが違う
    let w = world(
        &[(0.0, 0.0), (0.0, 200.0), (12.0, 0.0), (12.0, 200.0)],
        &[
            Spec::new(0, 1, RES).with_ref("16"),
            Spec::new(2, 3, RES).with_ref("17"),
        ],
        &[],
        &[],
    );
    let mut counts = TableCounts::default();
    counts.route_by_kind[RES.code() as usize][0] = 900;
    counts.route_by_kind[RES.code() as usize][4] = 900;
    counts.ref_states[0][ref_group(RES.code())][REF_MATCH] = 900;
    counts.ref_states[0][ref_group(RES.code())][REF_MISMATCH] = 10;
    let tables = Tables::from_counts(counts, 10.0);
    let weight_of = |out: &Outcome, edge: u32| {
        out.targets
            .iter()
            .find(|t| t.kind == 0 && t.target == edge)
            .map_or(0.0, |t| t.weight)
    };
    let at = at_m(6.0, 100.0);
    // 既定では、国道16号の事故はrefの一致する辺0に寄る
    let with_likelihood = RoadIndex::build(
        w.graph.parts(),
        &w.attrs,
        &w.topology,
        AssignParams::default(),
    );
    let out =
        with_likelihood.assign_one(&tables, at, &attr(16, 14), &mut Scratch::default(), false);
    assert!(weight_of(&out, 0) > 0.8, "{:?}", weights(&out));
    // 尤度表を使わなければ、属性が違っても距離が同じなので同じ重み（0.5ずつ）
    let without_likelihood = RoadIndex::build(
        w.graph.parts(),
        &w.attrs,
        &w.topology,
        AssignParams {
            use_likelihood: false,
            ..AssignParams::default()
        },
    );
    for a in [attr(16, 14), attr(3000, 14), AccidentAttr::MISSING] {
        let out = without_likelihood.assign_one(&tables, at, &a, &mut Scratch::default(), false);
        assert_eq!(out.targets.len(), 2, "{:?}", weights(&out));
        // 座標は1e-7度に丸めるので、距離には1cmほどの差が残る
        assert!(
            (weight_of(&out, 0) - 0.5).abs() < 1e-2,
            "{:?}",
            weights(&out)
        );
        assert!(
            (weight_of(&out, 1) - 0.5).abs() < 1e-2,
            "{:?}",
            weights(&out)
        );
    }
}

#[test]
fn clusters_false_assigns_intersection_accidents_only_to_edges() {
    // 十字路。ハブ（ノード0）をクラスタにする
    let w = world(
        &[
            (0.0, 0.0),
            (100.0, 0.0),
            (-100.0, 0.0),
            (0.0, 100.0),
            (0.0, -100.0),
        ],
        &[
            Spec::new(0, 1, RES),
            Spec::new(0, 2, RES),
            Spec::new(0, 3, RES),
            Spec::new(0, 4, RES),
        ],
        &[&[0]],
        &[],
    );
    let index = RoadIndex::build(
        w.graph.parts(),
        &w.attrs,
        &w.topology,
        AssignParams {
            use_clusters: false,
            ..AssignParams::default()
        },
    );
    assert!(
        index
            .cluster_candidates(
                at_m(3.0, 4.0),
                &LocalFrame::new(at_m(3.0, 4.0)),
                &mut Scratch::default()
            )
            .is_empty()
    );
    let out = index.assign_one(
        &Tables::neutral(),
        at_m(3.0, 4.0),
        &attr(3000, 1),
        &mut Scratch::default(),
        true,
    );
    assert!(!out.targets.is_empty(), "{:?}", weights(&out));
    assert!(
        out.targets.iter().all(|t| t.kind == 0),
        "{:?}",
        weights(&out)
    );
    let detail = out.detail.as_ref().unwrap();
    assert!(detail.clusters.is_empty());
    // 既定では同じ事故がクラスタに載る
    let out = assign(&w, at_m(3.0, 4.0), &attr(3000, 1));
    assert_eq!(out.targets[0].kind, 1);
}

#[test]
fn params_json_omits_the_comparison_switches_at_default_and_records_false() {
    let json = AssignParams::default().to_json();
    assert!(json.get("use_likelihood").is_none());
    assert!(json.get("use_clusters").is_none());
    let json = AssignParams {
        use_likelihood: false,
        use_clusters: false,
        ..AssignParams::default()
    }
    .to_json();
    assert_eq!(json["use_likelihood"], false);
    assert_eq!(json["use_clusters"], false);
}

#[test]
fn params_json_records_the_rules() {
    let json = AssignParams::default().to_json();
    assert_eq!(json["rules_version"], ASSIGN_RULES_VERSION);
    assert_eq!(json["sigma0_m"], 3.5);
    assert_eq!(json["max_targets"], 3);
    assert_eq!(json["renormalize_after_cut"], false);
    assert!(distance_m(at_m(0.0, 0.0), at_m(0.0, 100.0)) > 99.0);
}
