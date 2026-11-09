//! 事故の集合Bと、事故を置く小線分（behavior.md 6・7章）。
//!
//! 経路が通る辺・内部ノードのクラスタへの割り当て、歩道の親の車道、帰属不明の事故を
//! 事故IDの集合で1回だけ数える。72m判定と最寄りsidは、応答の `coordinates` を75m分割した
//! 小線分で行う。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::contract::{
    AccidentPoint, AccidentTotals, Attribution, CancelToken, ConditionFilter, EngineError,
    Interrupt, LonLat, Mode,
};
use crate::domain::corridor::{
    CANCEL_CHECK_INTERVAL, CORRIDOR_RADIUS_M, CORRIDOR_RADIUS_SQ_M2, Subdivision,
};
use crate::domain::routing::HIGH_RISK_MIN_ACCIDENTS;
use crate::domain::routing::accident::{EdgeAccidentIndex, UnassignedGrid};
use crate::domain::routing::graph::{Coord, EdgeId, Graph};
use crate::domain::routing::search::Path;
use crate::domain::routing::search::endpoints::{Placement, placement};
use crate::domain::routing::topology::{ClusterId, Topology};
use crate::domain::scoring::{self, sample, severity};
use crate::domain::snapshot::{AccidentSnapshot, Cell, cell_of};

use super::geometry::{self, RouteShape};

/// 事故を置く位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    /// 経路が通る辺への割り当て。
    Edge {
        dedge_index: u32,
        edge: EdgeId,
        pos_dm: u32,
    },
    /// 経路の内部ノードが属するクラスタへの割り当て。
    Cluster { cluster: ClusterId },
    /// 歩道の親の車道への割り当て。72m判定で求めた最寄りsid。
    SidewalkParent { sid: u32 },
    /// 帰属不明の事故。72m判定で求めた最寄りsid。
    Unassigned { sid: u32 },
}

impl Location {
    /// 応答の `attribution` への対応。ここだけに書く。
    fn attribution(self) -> Attribution {
        match self {
            Location::Edge { .. } | Location::SidewalkParent { .. } => Attribution::Edge,
            Location::Cluster { .. } => Attribution::Intersection,
            Location::Unassigned { .. } => Attribution::Unattributed,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Chosen {
    weight: f32,
    /// 0：辺、1：クラスタ。同じ重みなら辺を優先する。
    rank: u8,
    /// 辺は経路の添字、クラスタは内部ノードの添字。
    position: u32,
    location: Location,
}

/// 同じ事故に複数の割り当てが当たったときの選び方（behavior.md 7章）。
fn better(new: Chosen, old: Chosen) -> bool {
    if new.weight != old.weight {
        return new.weight > old.weight;
    }
    if new.rank != old.rank {
        return new.rank < old.rank;
    }
    if new.position != old.position {
        return new.position < old.position;
    }
    match (new.location, old.location) {
        (Location::Edge { pos_dm: a, .. }, Location::Edge { pos_dm: b, .. }) => a < b,
        _ => false,
    }
}

fn remember(assigned: &mut BTreeMap<u32, Chosen>, accident: u32, candidate: Chosen) {
    match assigned.get(&accident) {
        Some(&old) if !better(candidate, old) => {}
        _ => {
            assigned.insert(accident, candidate);
        }
    }
}

/// 応答の形状を75m分割した小線分の格子。点のセルに登録されたsidだけを比べる。
struct PathGrid {
    cells: HashMap<Cell, Vec<u32>>,
}

impl PathGrid {
    fn build(subdivision: &Subdivision, cancel: &CancelToken) -> Result<Self, Interrupt> {
        let projection = &subdivision.projection;
        let lat_margin = CORRIDOR_RADIUS_M / projection.ky;
        let lon_margin = CORRIDOR_RADIUS_M / projection.kx;
        let mut cells: HashMap<Cell, Vec<u32>> = HashMap::new();
        for (sid, segment) in subdivision.segments.iter().enumerate() {
            if sid % CANCEL_CHECK_INTERVAL == 0 {
                cancel.check()?;
            }
            let lat_lo = segment.start.lat.min(segment.end.lat) - lat_margin;
            let lat_hi = segment.start.lat.max(segment.end.lat) + lat_margin;
            let lon_lo = segment.start.lon.min(segment.end.lon) - lon_margin;
            let lon_hi = segment.start.lon.max(segment.end.lon) + lon_margin;
            let (gy_lo, gx_lo) = cell_of(lat_lo, lon_lo);
            let (gy_hi, gx_hi) = cell_of(lat_hi, lon_hi);
            for gy in gy_lo..=gy_hi {
                for gx in gx_lo..=gx_hi {
                    cells.entry((gy, gx)).or_default().push(sid as u32);
                }
            }
        }
        Ok(Self { cells })
    }

    fn nearest(
        &self,
        subdivision: &Subdivision,
        point: Coord,
        cancel: &CancelToken,
    ) -> Result<Option<(u32, f64)>, Interrupt> {
        let latlon = LonLat::from(point.to_latlon());
        let Some(sids) = self.cells.get(&cell_of(latlon.lat, latlon.lon)) else {
            return Ok(None);
        };
        let xy = subdivision.projection.xy(latlon);
        let mut best: Option<(f64, u32)> = None;
        for chunk in sids.chunks(CANCEL_CHECK_INTERVAL) {
            cancel.check()?;
            for &sid in chunk {
                let d2 = subdivision.segments[sid as usize].distance_sq(xy);
                if best.is_none_or(|(b, _)| d2 < b) {
                    best = Some((d2, sid));
                }
            }
        }
        Ok(best.map(|(d2, sid)| (sid, d2)))
    }
}

/// 集合B（絞り込み後）と、各事故を置く位置。
#[derive(Debug, Clone, PartialEq)]
pub struct AccidentSet {
    /// 事故IDの昇順。
    pub ids: Vec<u32>,
    pub locations: BTreeMap<u32, Location>,
    /// 条件（移動手段・時間帯・曜日区分）で絞る前の事故IDの数。天候でも絞らない。
    /// `assigned` と `nearest` を合わせた数（`nearest` は `assigned` にないIDだけ）。
    pub total: u64,
}

/// 集合Bの集計。
#[derive(Debug, Clone, PartialEq)]
pub struct AccidentSummary {
    pub totals: AccidentTotals,
    pub weather_match: Option<AccidentTotals>,
    pub accident_points: Vec<AccidentPoint>,
}

fn add(totals: &mut AccidentTotals, deaths: u16, injuries: u16) {
    totals.accident_count += 1;
    if deaths > 0 {
        totals.fatal_accidents += 1;
    } else {
        totals.injury_accidents += 1;
    }
    totals.deaths += u64::from(deaths);
    totals.injuries += u64::from(injuries);
}

fn matches_conditions(
    snapshot: &AccidentSnapshot,
    id: u32,
    mode: Mode,
    conditions: ConditionFilter,
) -> bool {
    let columns = snapshot.columns();
    let index = id as usize;
    if index >= columns.len() {
        return false;
    }
    if columns.modes[index] & mode.mask() == 0 {
        return false;
    }
    if conditions
        .period
        .is_some_and(|period| columns.period[index] != period.code())
    {
        return false;
    }
    if conditions
        .day_type
        .is_some_and(|day_type| columns.day_type[index] != day_type.code())
    {
        return false;
    }
    true
}

/// 経路の各有向辺が通る、辺の始点からの距離の区間（両端を含む）。
fn travelled_range(
    graph: &Graph,
    path: &Path,
    index: usize,
    start_inside: bool,
    end_inside: bool,
) -> (u32, u32) {
    let d = path.dedges[index];
    let length = graph.edge(d.edge()).length_dm;
    let (mut lo, mut hi) = (0_u32, length);
    if index == 0 && start_inside && d.edge() == path.start.edge {
        let offset = path.start.offset_dm.min(length);
        if d.is_reverse() {
            hi = offset;
        } else {
            lo = offset;
        }
    }
    if index + 1 == path.dedges.len() && end_inside && d.edge() == path.end.edge {
        let offset = path.end.offset_dm.min(length);
        if d.is_reverse() {
            lo = lo.max(offset);
        } else {
            hi = hi.min(offset);
        }
    }
    (lo.min(hi), hi)
}

/// 辺の割り当て・クラスタの割り当て・歩道の親・帰属不明から集合Bを作る。
#[allow(clippy::too_many_arguments)]
pub fn build_accident_set(
    graph: &Graph,
    topology: &Topology,
    accidents: &EdgeAccidentIndex,
    unassigned: &UnassignedGrid,
    snapshot: &AccidentSnapshot,
    subdivision: &Subdivision,
    path: &Path,
    mode: Mode,
    conditions: ConditionFilter,
    cancel: &CancelToken,
) -> Result<AccidentSet, EngineError> {
    let start_inside = placement(graph, &path.start) == Some(Placement::Inside);
    let end_inside = placement(graph, &path.end) == Some(Placement::Inside);
    let mut assigned: BTreeMap<u32, Chosen> = BTreeMap::new();

    // 6章(1)の1つ目：経路が通る辺への割り当て（向きは問わない）。
    for (i, &d) in path.dedges.iter().enumerate() {
        if i % CANCEL_CHECK_INTERVAL == 0 {
            cancel.check()?;
        }
        let edge = d.edge();
        let (lo, hi) = travelled_range(graph, path, i, start_inside, end_inside);
        for entry in accidents.edge_entries(edge) {
            if entry.pos_dm < lo || entry.pos_dm > hi {
                continue;
            }
            remember(
                &mut assigned,
                entry.accident,
                Chosen {
                    weight: entry.weight,
                    rank: 0,
                    position: i as u32,
                    location: Location::Edge {
                        dedge_index: i as u32,
                        edge,
                        pos_dm: entry.pos_dm,
                    },
                },
            );
        }
    }

    // 6章(1)の2つ目：内部ノード（i = 0..n−2 の終点）が属するクラスタ。
    let mut seen_clusters: HashSet<ClusterId> = HashSet::new();
    for i in 0..path.dedges.len().saturating_sub(1) {
        if i % CANCEL_CHECK_INTERVAL == 0 {
            cancel.check()?;
        }
        let node = graph.dedge_to(path.dedges[i]);
        let Some(cluster) = topology.cluster_of(node) else {
            continue;
        };
        if !seen_clusters.insert(cluster) {
            continue;
        }
        for entry in accidents.cluster_entries(cluster) {
            remember(
                &mut assigned,
                entry.accident,
                Chosen {
                    weight: entry.weight,
                    rank: 1,
                    position: i as u32,
                    location: Location::Cluster { cluster },
                },
            );
        }
    }

    let grid = PathGrid::build(subdivision, cancel)?;
    let mut nearest: BTreeMap<u32, Location> = BTreeMap::new();

    // 6章(1)の3つ目：経路が通る歩道の、親の車道への割り当て（72m以内）。
    let mut parents: BTreeSet<EdgeId> = BTreeSet::new();
    for &d in &path.dedges {
        if let Some(parent) = topology.sidewalk_parent(d.edge()) {
            parents.insert(parent);
        }
    }
    for parent in parents {
        for entry in accidents.edge_entries(parent) {
            let id = entry.accident;
            if assigned.contains_key(&id) || nearest.contains_key(&id) {
                continue;
            }
            let index = id as usize;
            if index >= snapshot.len() {
                continue;
            }
            let columns = snapshot.columns();
            let point = Coord::from_latlon(crate::contract::LatLon::new(
                columns.lat[index],
                columns.lon[index],
            ));
            if let Some((sid, d2)) = grid.nearest(subdivision, point, cancel)?
                && d2 <= CORRIDOR_RADIUS_SQ_M2
            {
                nearest.insert(id, Location::SidewalkParent { sid });
            }
        }
    }

    // 6章(2)：帰属不明の事故。経路の小線分の外接矩形を72m広げたセルを走査する。
    let mut unassigned_ids: BTreeSet<u32> = BTreeSet::new();
    for cell in grid.cells.keys() {
        for &id in unassigned.cell(*cell) {
            unassigned_ids.insert(id);
        }
    }
    for id in unassigned_ids {
        if assigned.contains_key(&id) || nearest.contains_key(&id) {
            continue;
        }
        let index = id as usize;
        if index >= snapshot.len() {
            continue;
        }
        let columns = snapshot.columns();
        let point = Coord::from_latlon(crate::contract::LatLon::new(
            columns.lat[index],
            columns.lon[index],
        ));
        if let Some((sid, d2)) = grid.nearest(subdivision, point, cancel)?
            && d2 <= CORRIDOR_RADIUS_SQ_M2
        {
            nearest.insert(id, Location::Unassigned { sid });
        }
    }

    // 絞る前の総件数。`nearest` は `assigned` にないIDだけなので、両者の和が事故IDの数。
    let total = (assigned.len() + nearest.len()) as u64;

    // 6章(3)：手段・時間帯・曜日区分で絞る（天候では減らさない）。
    let mut ids = Vec::new();
    let mut locations = BTreeMap::new();
    for (&id, chosen) in &assigned {
        if matches_conditions(snapshot, id, mode, conditions) {
            ids.push(id);
            locations.insert(id, chosen.location);
        }
    }
    for (&id, &location) in &nearest {
        if matches_conditions(snapshot, id, mode, conditions) {
            ids.push(id);
            locations.insert(id, location);
        }
    }
    ids.sort_unstable();
    Ok(AccidentSet {
        ids,
        locations,
        total,
    })
}

/// 集合Bの件数・人数・天候一致・表示事故点（behavior.md 7章）。
pub fn summarize(
    snapshot: &AccidentSnapshot,
    conditions: ConditionFilter,
    accidents: &AccidentSet,
    cancel: &CancelToken,
) -> Result<AccidentSummary, Interrupt> {
    let columns = snapshot.columns();
    let mut totals = AccidentTotals::default();
    let mut weather_match = conditions.weather.map(|_| AccidentTotals::default());
    let mut fatal = Vec::new();
    let mut other = Vec::new();
    for (i, &id) in accidents.ids.iter().enumerate() {
        if i % CANCEL_CHECK_INTERVAL == 0 {
            cancel.check()?;
        }
        let index = id as usize;
        let (deaths, injuries) = (columns.deaths[index], columns.injuries[index]);
        add(&mut totals, deaths, injuries);
        if let (Some(m), Some(weather)) = (weather_match.as_mut(), conditions.weather)
            && columns.weather[index] == weather.code()
        {
            add(m, deaths, injuries);
        }
        if deaths > 0 {
            fatal.push(id);
        } else {
            other.push(id);
        }
    }
    let mut sampled = sample(&fatal, crate::contract::engine::MAX_ACCIDENT_POINTS);
    sampled.extend(sample(
        &other,
        crate::contract::engine::MAX_ACCIDENT_POINTS - sampled.len(),
    ));
    let accident_points = sampled
        .into_iter()
        .map(|id| {
            let mut point = scoring::accident_point(snapshot, id);
            point.attribution = accidents
                .locations
                .get(&id)
                .map_or(Attribution::Unattributed, |&location| {
                    location.attribution()
                });
            point
        })
        .collect();
    Ok(AccidentSummary {
        totals,
        weather_match,
        accident_points,
    })
}

/// 元の形状のうち、有向辺 `dedge_index` を描いた頂点範囲で、通る向きの先頭からの割合 `f` の点。
/// `(点, 元の線分の番号, 線分の中の割合)` を返す。
fn point_at_fraction(shape: &RouteShape, dedge_index: usize, f: f64) -> (Coord, usize, f64) {
    let Some(&(start, end)) = shape.ranges.get(dedge_index) else {
        return (shape.full[0], 0, 0.0);
    };
    if start >= end || end >= shape.full.len() {
        let segment = start.min(shape.full.len() - 2);
        return (shape.full[start.min(shape.full.len() - 1)], segment, 0.0);
    }
    let lengths: Vec<f64> = (start..end)
        .map(|i| {
            shape.projection.distance(
                LonLat::from(shape.full[i].to_latlon()),
                LonLat::from(shape.full[i + 1].to_latlon()),
            )
        })
        .collect();
    let total: f64 = lengths.iter().sum();
    if total <= 0.0 {
        return (shape.full[start], start, 0.0);
    }
    let target = f.clamp(0.0, 1.0) * total;
    let mut acc = 0.0;
    for (i, &length) in lengths.iter().enumerate() {
        if length <= 0.0 {
            continue;
        }
        if target <= acc + length {
            let t = ((target - acc) / length).clamp(0.0, 1.0);
            let a = shape.full[start + i];
            let b = shape.full[start + i + 1];
            return (interpolate(a, b, t), start + i, t);
        }
        acc += length;
    }
    (shape.full[end], end - 1, 1.0)
}

/// `snap::point_on_edge` と同じ丸めの線形補間。
fn interpolate(a: Coord, b: Coord, t: f64) -> Coord {
    if t <= 0.0 {
        return a;
    }
    if t >= 1.0 {
        return b;
    }
    let lerp = |x: i32, y: i32| (f64::from(x) + t * (f64::from(y) - f64::from(x))).round() as i32;
    Coord::new(lerp(a.lat_e7, b.lat_e7), lerp(a.lon_e7, b.lon_e7))
}

/// 辺への割り当ての事故を置くsid。
#[allow(clippy::too_many_arguments)]
fn edge_sid(
    graph: &Graph,
    path: &Path,
    shape: &RouteShape,
    subdivision: &Subdivision,
    bases: &[u32],
    dedge_index: usize,
    pos_dm: u32,
    cancel: &CancelToken,
) -> Result<u32, EngineError> {
    let start_inside = placement(graph, &path.start) == Some(Placement::Inside);
    let end_inside = placement(graph, &path.end) == Some(Placement::Inside);
    let (lo, hi) = travelled_range(graph, path, dedge_index, start_inside, end_inside);
    let f = if hi <= lo {
        0.0
    } else if path.dedges[dedge_index].is_reverse() {
        f64::from(hi - pos_dm) / f64::from(hi - lo)
    } else {
        f64::from(pos_dm - lo) / f64::from(hi - lo)
    };
    let (point, segment, t) = point_at_fraction(shape, dedge_index, f);
    if shape.simplified {
        Ok(geometry::nearest_sid(
            subdivision,
            LonLat::from(point.to_latlon()),
            cancel,
        )?)
    } else {
        geometry::sid_at_segment(bases, segment, t).ok_or_else(|| {
            EngineError::ContractViolation("事故を置く小線分がありません".to_owned())
        })
    }
}

/// クラスタへの割り当ての事故を置くsid。同じクラスタの事故は同じsidになる。
#[allow(clippy::too_many_arguments)]
fn cluster_sid(
    graph: &Graph,
    topology: &Topology,
    path: &Path,
    shape: &RouteShape,
    subdivision: &Subdivision,
    base: &[u32],
    cluster: ClusterId,
    cancel: &CancelToken,
) -> Result<u32, EngineError> {
    let record = &topology.clusters[cluster as usize];
    let center = Coord::new(record.lat_e7, record.lon_e7);
    let xy = subdivision.projection.xy(LonLat::from(center.to_latlon()));
    if shape.simplified {
        // 間引いた後の頂点範囲に写すのが難しいので、全小線分から最寄りを選ぶ。
        let mut best: Option<(f64, u32)> = None;
        for (sid, segment) in subdivision.segments.iter().enumerate() {
            if sid % CANCEL_CHECK_INTERVAL == 0 {
                cancel.check()?;
            }
            let d2 = segment.distance_sq(xy);
            if best.is_none_or(|(b, _)| d2 < b) {
                best = Some((d2, sid as u32));
            }
        }
        return Ok(best.map_or(0, |(_, sid)| sid));
    }
    let mut first: Option<usize> = None;
    let mut last: Option<usize> = None;
    for i in 0..path.dedges.len().saturating_sub(1) {
        if topology.cluster_of(graph.dedge_to(path.dedges[i])) == Some(cluster) {
            let vertex = shape.internal_node_vertex(i).ok_or_else(|| {
                EngineError::ContractViolation("クラスタの頂点範囲がありません".to_owned())
            })?;
            if first.is_none() {
                first = Some(vertex);
            }
            last = Some(vertex);
        }
    }
    let (Some(start_vertex), Some(end_vertex)) = (first, last) else {
        return Ok(geometry::nearest_sid(
            subdivision,
            LonLat::from(center.to_latlon()),
            cancel,
        )?);
    };
    let segment_count = subdivision.segments.len();
    if segment_count == 0 {
        return Err(EngineError::ContractViolation(
            "小線分がありません".to_owned(),
        ));
    }
    // クラスタの頂点範囲の前後1小線分までを候補にする。
    let start = if start_vertex > 0 {
        base[start_vertex] - 1
    } else {
        0
    };
    let end = if end_vertex < subdivision.edge_lengths.len() {
        base[end_vertex]
    } else {
        segment_count as u32 - 1
    };
    let mut best: Option<(f64, u32)> = None;
    for sid in start..=end {
        let d2 = subdivision.segments[sid as usize].distance_sq(xy);
        if best.is_none_or(|(b, _)| d2 < b) {
            best = Some((d2, sid));
        }
    }
    Ok(best.map_or(0, |(_, sid)| sid))
}

/// 集合Bの事故を1つのsidに置き、sidごとの件数と深刻度（`10×死者+負傷者`）の合計を返す。
///
/// しきい値を変えたときの区間の数を数える評価用にも使う（`routing_eval engine-batch`）。
#[allow(clippy::too_many_arguments)]
pub fn sid_counts(
    graph: &Graph,
    topology: &Topology,
    snapshot: &AccidentSnapshot,
    path: &Path,
    shape: &RouteShape,
    subdivision: &Subdivision,
    accidents: &AccidentSet,
    cancel: &CancelToken,
) -> Result<(Vec<u32>, Vec<u64>), EngineError> {
    let columns = snapshot.columns();
    let mut counts = vec![0_u32; subdivision.segments.len()];
    let mut severities = vec![0_u64; subdivision.segments.len()];
    let bases = geometry::segment_bases(subdivision);
    // 同じクラスタの事故は同じsidに入るので、クラスタごとに1回だけ求める。
    let mut cluster_sids: HashMap<ClusterId, u32> = HashMap::new();
    for (i, &id) in accidents.ids.iter().enumerate() {
        if i % CANCEL_CHECK_INTERVAL == 0 {
            cancel.check()?;
        }
        let Some(&location) = accidents.locations.get(&id) else {
            continue;
        };
        let sid = match location {
            Location::Edge {
                dedge_index,
                pos_dm,
                ..
            } => edge_sid(
                graph,
                path,
                shape,
                subdivision,
                &bases,
                dedge_index as usize,
                pos_dm,
                cancel,
            )?,
            Location::Cluster { cluster } => match cluster_sids.get(&cluster) {
                Some(&sid) => sid,
                None => {
                    let sid = cluster_sid(
                        graph,
                        topology,
                        path,
                        shape,
                        subdivision,
                        &bases,
                        cluster,
                        cancel,
                    )?;
                    cluster_sids.insert(cluster, sid);
                    sid
                }
            },
            Location::SidewalkParent { sid } | Location::Unassigned { sid } => sid,
        };
        let index = id as usize;
        counts[sid as usize] += 1;
        severities[sid as usize] += severity(columns.deaths[index], columns.injuries[index]);
    }
    Ok((counts, severities))
}

/// sidごとの件数と深刻度から、件数が [`HIGH_RISK_MIN_ACCIDENTS`] 以上のsidだけに
/// 深刻度の和を残した重み（behavior.md 7章）。
pub fn high_risk_weights_from_counts(counts: &[u32], severities: &[u64]) -> Vec<u64> {
    severities
        .iter()
        .enumerate()
        .map(|(sid, &severity)| {
            if counts[sid] as usize >= HIGH_RISK_MIN_ACCIDENTS {
                severity
            } else {
                0
            }
        })
        .collect()
}

/// 集合Bの事故を1つのsidに置き、件数が [`HIGH_RISK_MIN_ACCIDENTS`] 以上のsidだけに
/// 深刻度の和を残した重み（behavior.md 7章）。
///
/// 評価本体は `sid_counts` の結果を [`high_risk_weights_from_counts`] に渡す（2通りの
/// しきい値を数えるため、件数と深刻度を1回だけ求める）。この関数は同じ規則の入口として残す。
#[allow(dead_code, clippy::too_many_arguments)]
pub fn high_risk_weights(
    graph: &Graph,
    topology: &Topology,
    snapshot: &AccidentSnapshot,
    path: &Path,
    shape: &RouteShape,
    subdivision: &Subdivision,
    accidents: &AccidentSet,
    cancel: &CancelToken,
) -> Result<Vec<u64>, EngineError> {
    let (counts, severities) = sid_counts(
        graph,
        topology,
        snapshot,
        path,
        shape,
        subdivision,
        accidents,
        cancel,
    )?;
    Ok(high_risk_weights_from_counts(&counts, &severities))
}
