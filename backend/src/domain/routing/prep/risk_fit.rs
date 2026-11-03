//! P6：事故の割り当てから `risk.bin` の係数を推定する（形式書14章、作業書M3b.1）。
//!
//! 入力はメモリ上の表だけを受け取り、ファイルの読み書きはしない。`routing_prep risk-fit` と
//! テストが同じ [`fit_risk`] を呼ぶ。推定の式は [`RiskView`](crate::domain::routing::risk::RiskView)
//! と同じ意味にし、辺の密度・交差点の密度・深刻度・天候の割合・縮小の母数を返す。
use std::collections::{HashMap, HashSet};

use rayon::prelude::*;
use serde_json::{Map, Value, json};

use crate::contract::Mode;
use crate::domain::routing::accident::{AccidentAttrTable, Assignment, TargetKind};
use crate::domain::routing::graph::GraphParts;
use crate::domain::routing::prep::assign::is_road_edge;
use crate::domain::routing::prep::attrs::GraphAttrs;
use crate::domain::routing::risk::{
    CELL_COUNT, CoefRecord, EDGE_K_NO_RISK, EDGE_K_SIDEWALK, RiskModel, RiskParams, TABLE_LEN,
    branch_band, cluster_class_j, road_class_k, road_group, table_index,
};
use crate::domain::routing::topology::{Topology, cluster_flags};
use crate::domain::snapshot::AccidentColumns;

/// 辺の道路種別の数（group 0〜15 × band 0〜3 × urban 0〜1 = 128）。
const EDGE_KINDS: usize = 128;
/// 交差点の種別の数（top_group 0〜9 × branch_band 0〜2 × signal 0〜1 = 80）。
const CLUSTER_KINDS: usize = 80;
/// urban の格子（度）。
const URBAN_CELL_DEG: f64 = 0.005;
/// urban=1 にする地形コード1の割合の下限。
const URBAN_MIN_RATIO: f64 = 0.5;
/// urban=1 にする事故件数の下限。
const URBAN_MIN_COUNT: u32 = 10;
/// まとめ（pooling）を始める件数。
const POOL_MIN_COUNT: f64 = 20.0;
/// 深刻度・天候の割合をセルごとに使う件数の下限。
const SEVERITY_MIN_COUNT: f64 = 30.0;
/// 縮小の母数を親から借りる学習件数の下限。
const FIT_MIN_COUNT: f64 = 100.0;
/// W の候補（形式書14.4節の作業書M3b.1）。
const W_GRID: [f64; 15] = [
    0.01, 0.02, 0.05, 0.1, 0.15, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 0.95, 0.98,
];

/// `fit_risk` の入力（すべて借用。ファイルは読まない）。
pub struct RiskFitInput<'a> {
    pub graph: &'a GraphParts,
    pub graph_attrs: &'a GraphAttrs,
    pub topology: &'a Topology,
    pub accident_attrs: &'a AccidentAttrTable,
    pub assignment: &'a Assignment,
    pub accidents: &'a AccidentColumns,
}

/// `fit_risk` の設定。
#[derive(Debug, Clone, PartialEq)]
pub struct RiskFitConfig {
    /// 学習に使う `source_year`（昇順で重複なし）。
    pub train_years: Vec<u16>,
    /// 検証に使う `source_year`。
    pub validate_year: u16,
    /// 自動車の生活道路の係数c（計画書5.5節）。
    pub c: f64,
}

/// 推定の結果。
pub struct RiskFitOutput {
    pub model: RiskModel,
    /// 監査JSON（所要時間・メモリはコマンドが足す）。
    pub audit: Value,
}

/// セル1つの集計。
#[derive(Debug, Clone, Copy, Default)]
struct CellStat {
    /// 割り当ての重みの和（MIRROR を含む）。
    n: f64,
    /// `Σ w·(10·死者+負傷者)`。
    severity: f64,
    /// 天候コード1〜5ごとの重みの和。
    weather: [f64; 5],
}

/// 種別1つのセル別集計。
#[derive(Debug, Clone, Copy, Default)]
struct KindStat {
    cells: [CellStat; CELL_COUNT],
}

impl KindStat {
    fn add(&mut self, cell: usize, weight: f64, severity: f64, weather: usize) {
        let c = &mut self.cells[cell];
        c.n += weight;
        c.severity += weight * severity;
        c.weather[weather] += weight;
    }

    fn total_n(&self) -> f64 {
        self.cells.iter().map(|c| c.n).sum()
    }

    fn total_severity(&self) -> f64 {
        self.cells.iter().map(|c| c.severity).sum()
    }

    fn total_weather(&self, weather: usize) -> f64 {
        self.cells.iter().map(|c| c.weather[weather]).sum()
    }

    fn add_assign(&mut self, other: &KindStat) {
        for (a, b) in self.cells.iter_mut().zip(&other.cells) {
            a.n += b.n;
            a.severity += b.severity;
            for (x, y) in a.weather.iter_mut().zip(&b.weather) {
                *x += y;
            }
        }
    }
}

/// 縮小の母数を選ぶための、辺・クラスタ1つ分の学習・検証の重み。
#[derive(Debug, Clone, Copy, Default)]
struct FitStat {
    train: f64,
    validate: f64,
    /// 雨・霧・雪（天候コード3〜5）の学習年の重み。
    train_weather: [f64; 3],
    /// 雨・霧・雪の検証年の重み。
    validate_weather: [f64; 3],
}

impl FitStat {
    fn add(&mut self, validate: bool, weight: f64, weather: Option<usize>) {
        if validate {
            self.validate += weight;
            if let Some(w) = weather {
                self.validate_weather[w] += weight;
            }
        } else {
            self.train += weight;
            if let Some(w) = weather {
                self.train_weather[w] += weight;
            }
        }
    }
}

/// 辺またはクラスタ1つ分の推定用の行。
#[derive(Debug, Clone)]
struct FitEntry {
    id: u32,
    /// 辺の長さ（m）。クラスタは1。
    length: f64,
    stat: FitStat,
}

/// 係数表の1行（10セル）。
#[derive(Debug, Clone, Default)]
struct Row {
    r: [f64; CELL_COUNT],
    s: [f64; CELL_COUNT],
    rho: [[f32; 5]; CELL_COUNT],
    a: f64,
}

impl Row {
    fn r_sum(&self) -> f64 {
        self.r.iter().sum()
    }

    /// 全セルで `r` により重み付けした天候 `weather`（0〜4）の割合。
    fn rho_weighted(&self, weather: usize) -> f64 {
        let r = self.r_sum();
        if r == 0.0 {
            return 0.0;
        }
        self.r
            .iter()
            .zip(&self.rho)
            .map(|(&r, rho)| r * f64::from(rho[weather]))
            .sum::<f64>()
            / r
    }
}

/// W の探索結果。
#[derive(Debug, Clone)]
struct WFit {
    w: f64,
    deviance: Option<f64>,
    deviance_low: Option<f64>,
    deviance_high: Option<f64>,
    parent: Option<String>,
}

impl WFit {
    fn empty(deviance: Option<f64>) -> Self {
        Self {
            w: 0.0,
            deviance,
            deviance_low: deviance,
            deviance_high: deviance,
            parent: None,
        }
    }
}

/// 推定の途中で使う、種別の親（pooling・W の借用）。
struct Parent {
    level: String,
    kinds: Vec<usize>,
}

/// JSONを一度文字列にして読み直す。
fn json_roundtrip(value: &Value) -> Value {
    let text = serde_json::to_string(value).expect("JSONに書ける値");
    serde_json::from_str(&text).expect("自分で書いたJSON")
}

/// 浮動小数 `value` がJSONの往復で同じビット列になるか。
fn json_fixed(value: f64) -> bool {
    let Ok(text) = serde_json::to_string(&value) else {
        return false;
    };
    serde_json::from_str::<f64>(&text).is_ok_and(|back| back.to_bits() == value.to_bits())
}

/// JSONの往復で同じ値になる、最も近い浮動小数を探す。
///
/// `serde_json` の浮動小数の読込は、長い仮数の一部を1ulpほど違う値に読むことがある。
/// `PARAMS` の値が書く前と読み直し後で一致するように、往復で不動になる隣の値を選ぶ。
fn stable_float(value: f64) -> f64 {
    if !value.is_finite() || value == 0.0 {
        return value;
    }
    if value < 0.0 {
        return -stable_float(-value);
    }
    if json_fixed(value) {
        return value;
    }
    for step in 1..=32_u64 {
        for bits in [
            value.to_bits().checked_add(step),
            value.to_bits().checked_sub(step),
        ]
        .into_iter()
        .flatten()
        {
            let candidate = f64::from_bits(bits);
            if candidate.is_finite() && json_fixed(candidate) {
                return candidate;
            }
        }
    }
    value
}

/// JSONの中の浮動小数をすべて [`stable_float`] に通す。
fn stabilize_json(value: &mut Value) {
    match value {
        Value::Number(number) if number.is_f64() => {
            if let Some(x) = number.as_f64()
                && let Some(stable) = serde_json::Number::from_f64(stable_float(x))
            {
                *number = stable;
            }
        }
        Value::Array(values) => {
            for value in values {
                stabilize_json(value);
            }
        }
        Value::Object(map) => {
            for value in map.values_mut() {
                stabilize_json(value);
            }
        }
        _ => {}
    }
}

/// 入力の件数などの検査。
fn validate_input(input: &RiskFitInput<'_>, config: &RiskFitConfig) -> Result<(), String> {
    let edges = input.graph.edges.len();
    if input.graph_attrs.lanes.len() != edges || input.graph_attrs.width_dm.len() != edges {
        return Err(
            "graph_attrs.binのLANES・WIDTHDMの件数がgraph.binの辺数と一致しません".to_owned(),
        );
    }
    if input.accident_attrs.attrs.len() != input.accidents.len() {
        return Err(format!(
            "accident_attrs.binの件数{}がaccidents.csv.gzの件数{}と違います",
            input.accident_attrs.attrs.len(),
            input.accidents.len()
        ));
    }
    if input.assignment.accident_count() != input.accidents.len() {
        return Err(format!(
            "assignment.binの事故数{}がaccidents.csv.gzの件数{}と違います",
            input.assignment.accident_count(),
            input.accidents.len()
        ));
    }
    input
        .assignment
        .validate(edges, input.topology.cluster_count())
        .map_err(|e| format!("assignment.binが不正です: {e}"))?;
    if config.train_years.is_empty() {
        return Err("--train-years に年がありません".to_owned());
    }
    if config.train_years.windows(2).any(|w| w[0] >= w[1]) {
        return Err("--train-years は昇順で重複のない年を指定してください".to_owned());
    }
    if !(config.c.is_finite() && config.c >= 1.0) {
        return Err("--c は1以上の有限の数".to_owned());
    }
    Ok(())
}

/// 双子を含めた `lanes` と `width`（デシメートル）の合計。
fn total_lanes_width(
    edge: usize,
    parts: &GraphParts,
    attrs: &GraphAttrs,
    topology: &Topology,
) -> (u32, u32) {
    let lanes = u32::from(attrs.lanes.get(edge).copied().unwrap_or(0));
    let width = u32::from(attrs.width_dm.get(edge).copied().unwrap_or(0));
    let mut twin_lanes: Option<u32> = None;
    let mut twin_width: Option<u32> = None;
    let mut has_twin = false;
    for twin in topology.twins_of(edge as u32) {
        has_twin = true;
        let t = twin as usize;
        if t >= parts.edges.len() {
            continue;
        }
        let l = u32::from(attrs.lanes.get(t).copied().unwrap_or(0));
        if l > 0 {
            twin_lanes = Some(twin_lanes.map_or(l, |m| m.max(l)));
        }
        let w = u32::from(attrs.width_dm.get(t).copied().unwrap_or(0));
        if w > 0 {
            twin_width = Some(twin_width.map_or(w, |m| m.max(w)));
        }
    }
    if !has_twin {
        return (lanes, width);
    }
    let total = |own: u32, twin: Option<u32>| -> u32 {
        if own == 0 {
            twin.unwrap_or(0)
        } else {
            own + twin.unwrap_or(own)
        }
    };
    (total(lanes, twin_lanes), total(width, twin_width))
}

/// `lanes` と `width` から band（0〜3）を決める（形式書14.2節）。
fn band_of(lanes: u32, width_dm: u32) -> u8 {
    if lanes > 0 {
        match lanes {
            1 => 1,
            2..=3 => 2,
            _ => 3,
        }
    } else if width_dm > 0 {
        if width_dm < 55 {
            1
        } else if width_dm < 130 {
            2
        } else {
            3
        }
    } else {
        0
    }
}

/// 0.005度の格子のセル。
fn urban_cell(lat: f64, lon: f64) -> (i32, i32) {
    (
        (lat / URBAN_CELL_DEG).floor() as i32,
        (lon / URBAN_CELL_DEG).floor() as i32,
    )
}

/// 学習年の事故から、格子ごとの（件数、地形コード1の件数）を作る。
fn accident_terrain_cells(
    input: &RiskFitInput<'_>,
    train_years: &[u16],
) -> HashMap<(i32, i32), (u32, u32)> {
    let mut cells: HashMap<(i32, i32), (u32, u32)> = HashMap::new();
    for i in 0..input.accidents.len() {
        if !train_years.contains(&input.accidents.source_year[i]) {
            continue;
        }
        let terrain = input.accident_attrs.attrs[i].terrain;
        let entry = cells
            .entry(urban_cell(input.accidents.lat[i], input.accidents.lon[i]))
            .or_insert((0, 0));
        entry.0 += 1;
        if terrain == 1 {
            entry.1 += 1;
        }
    }
    cells
}

/// セルと周囲8セルの事故から urban を決める。
fn urban_decision(cell: (i32, i32), cells: &HashMap<(i32, i32), (u32, u32)>) -> bool {
    let mut count = 0_u32;
    let mut urban = 0_u32;
    for dlat in -1..=1 {
        for dlon in -1..=1 {
            if let Some(&(c, t)) = cells.get(&(cell.0 + dlat, cell.1 + dlon)) {
                count += c;
                urban += t;
            }
        }
    }
    count >= URBAN_MIN_COUNT && f64::from(urban) / f64::from(count.max(1)) >= URBAN_MIN_RATIO
}

/// 辺の中点の格子。
fn edge_cell(edge: usize, parts: &GraphParts) -> (i32, i32) {
    let record = &parts.edges[edge];
    let a = parts.node_ll[record.tail as usize].to_latlon();
    let b = parts.node_ll[record.head as usize].to_latlon();
    urban_cell((a.lat + b.lat) / 2.0, (a.lon + b.lon) / 2.0)
}

/// `EDGEK` を決める（形式書14.2節）。
fn build_edge_k(input: &RiskFitInput<'_>, train_years: &[u16], log: &dyn Fn(&str)) -> Vec<u8> {
    let edges = input.graph.edges.len();
    let terrain_cells = accident_terrain_cells(input, train_years);
    log(&format!(
        "  urbanの格子: 事故のあるセル{}",
        terrain_cells.len()
    ));
    let mut needed: HashSet<(i32, i32)> = HashSet::new();
    for (e, record) in input.graph.edges.iter().enumerate() {
        if is_road_edge(record) {
            needed.insert(edge_cell(e, input.graph));
        }
    }
    let needed: Vec<(i32, i32)> = needed.into_iter().collect();
    let urban: HashMap<(i32, i32), bool> = needed
        .par_iter()
        .map(|&cell| (cell, urban_decision(cell, &terrain_cells)))
        .collect();
    log(&format!("  urbanを決めた辺のセル{}", urban.len()));
    (0..edges)
        .into_par_iter()
        .map(|e| {
            if input.topology.sidewalk_parent(e as u32).is_some() {
                return EDGE_K_SIDEWALK;
            }
            let record = &input.graph.edges[e];
            if !is_road_edge(record) {
                return EDGE_K_NO_RISK;
            }
            let Some(group) = road_group(record.class) else {
                return EDGE_K_NO_RISK;
            };
            let (lanes, width) =
                total_lanes_width(e, input.graph, input.graph_attrs, input.topology);
            let band = band_of(lanes, width);
            let urban = urban
                .get(&edge_cell(e, input.graph))
                .copied()
                .unwrap_or(false);
            road_class_k(group, band, urban)
        })
        .collect()
}

/// `CLUSJ` を決める（形式書14.3節）。
fn build_cluster_j(topology: &Topology) -> Vec<u8> {
    topology
        .clusters
        .iter()
        .map(|record| {
            let top_group = road_group(record.top_class).map_or(9, |g| g.min(9));
            let signal = record.flags & cluster_flags::SIGNAL != 0;
            cluster_class_j(top_group, branch_band(record.branches), signal)
        })
        .collect()
}

/// 種別の集計を親の範囲で足す。
fn aggregate(stats: &[KindStat], kinds: &[usize]) -> KindStat {
    let mut total = KindStat::default();
    for &k in kinds {
        if let Some(stat) = stats.get(k) {
            total.add_assign(stat);
        }
    }
    total
}

/// 辺の親（形式書14.2節のまとめの順）。
fn edge_parent(kind: usize, stats: &[KindStat], lengths: &[f64], min_count: f64) -> Parent {
    let group = kind / 8;
    let urban = kind % 2;
    let band_all: Vec<usize> = (0..4).map(|b| group * 8 + b * 2 + urban).collect();
    let group_all: Vec<usize> = (0..8).map(|b| group * 8 + b).collect();
    let all: Vec<usize> = (0..EDGE_KINDS).filter(|&k| lengths[k] > 0.0).collect();
    let total = |kinds: &[usize]| -> f64 { kinds.iter().map(|&k| stats[k].total_n()).sum() };
    if total(&band_all) >= min_count {
        Parent {
            level: "group_urban".to_owned(),
            kinds: band_all,
        }
    } else if total(&group_all) >= min_count {
        Parent {
            level: "group".to_owned(),
            kinds: group_all,
        }
    } else {
        Parent {
            level: "all".to_owned(),
            kinds: all,
        }
    }
}

/// 交差点の親（形式書14.3節のまとめの順）。
fn cluster_parent(kind: usize, stats: &[KindStat], counts: &[f64], min_count: f64) -> Parent {
    let top_group = kind / 8;
    let signal = kind % 2;
    let branch_all: Vec<usize> = (0..3).map(|b| top_group * 8 + b * 2 + signal).collect();
    let group_all: Vec<usize> = (0..6).map(|b| top_group * 8 + b).collect();
    let all: Vec<usize> = (0..CLUSTER_KINDS).filter(|&j| counts[j] > 0.0).collect();
    let total = |kinds: &[usize]| -> f64 { kinds.iter().map(|&j| stats[j].total_n()).sum() };
    if total(&branch_all) >= min_count {
        Parent {
            level: "group_signal".to_owned(),
            kinds: branch_all,
        }
    } else if total(&group_all) >= min_count {
        Parent {
            level: "group".to_owned(),
            kinds: group_all,
        }
    } else {
        Parent {
            level: "all".to_owned(),
            kinds: all,
        }
    }
}

/// 辺・クラスタの `r` を、データの少ない種別だけ親の率で置き換える。
fn fill_density(
    stats: &[KindStat],
    lengths: &[f64],
    years: f64,
    cluster: bool,
    pooling: &mut Vec<Value>,
) -> Vec<Row> {
    let count = stats.len();
    let mut rows = vec![Row::default(); count];
    for k in 0..count {
        if lengths[k] <= 0.0 {
            continue;
        }
        let total = stats[k].total_n();
        if total < POOL_MIN_COUNT {
            let parent = if cluster {
                cluster_parent(k, stats, lengths, POOL_MIN_COUNT)
            } else {
                edge_parent(k, stats, lengths, POOL_MIN_COUNT)
            };
            let agg = aggregate(stats, &parent.kinds);
            let parent_length: f64 = parent.kinds.iter().map(|&j| lengths[j]).sum();
            if parent_length > 0.0 {
                for (c, cell) in agg.cells.iter().enumerate() {
                    rows[k].r[c] = cell.n / (years * parent_length);
                }
            }
            pooling.push(json!({
                "kind": k,
                "level": parent.level,
                "parent_kinds": parent.kinds,
                "train_count": total,
            }));
        } else {
            for (c, cell) in stats[k].cells.iter().enumerate() {
                rows[k].r[c] = cell.n / (years * lengths[k]);
            }
        }
    }
    rows
}

/// `s`・`rho` を、セルまたは種別の合計・親の値で埋める。
fn fill_severity(
    stats: &[KindStat],
    lengths: &[f64],
    cluster: bool,
    rows: &mut [Row],
    pooling: &mut Vec<Value>,
) {
    for k in 0..rows.len() {
        if lengths[k] <= 0.0 {
            continue;
        }
        let total = stats[k].total_n();
        if total < SEVERITY_MIN_COUNT {
            let parent = if cluster {
                cluster_parent(k, stats, lengths, SEVERITY_MIN_COUNT)
            } else {
                edge_parent(k, stats, lengths, SEVERITY_MIN_COUNT)
            };
            let agg = aggregate(stats, &parent.kinds);
            let n = agg.total_n();
            if n > 0.0 {
                let s = agg.total_severity() / n;
                for c in 0..CELL_COUNT {
                    rows[k].s[c] = s;
                    for w in 0..5 {
                        rows[k].rho[c][w] = (agg.total_weather(w) / n) as f32;
                    }
                }
            }
            pooling.push(json!({
                "kind": k,
                "level": parent.level,
                "parent_kinds": parent.kinds,
                "train_count": total,
            }));
        } else if total > 0.0 {
            let s_total = stats[k].total_severity() / total;
            let mut rho_total = [0.0_f32; 5];
            for (w, rho) in rho_total.iter_mut().enumerate() {
                *rho = (stats[k].total_weather(w) / total) as f32;
            }
            for (c, cell) in stats[k].cells.iter().enumerate() {
                if cell.n >= SEVERITY_MIN_COUNT {
                    rows[k].s[c] = cell.severity / cell.n;
                    for (w, rho) in rows[k].rho[c].iter_mut().enumerate() {
                        *rho = (cell.weather[w] / cell.n) as f32;
                    }
                } else {
                    rows[k].s[c] = s_total;
                    rows[k].rho[c] = rho_total;
                }
            }
        }
    }
}

/// 種別の辺・クラスタをまとめた `FitEntry` を作る。
fn aggregate_entries(entries: &[Vec<FitEntry>], kinds: &[usize]) -> Vec<FitEntry> {
    let mut out = Vec::new();
    for &k in kinds {
        if let Some(list) = entries.get(k) {
            out.extend(list.iter().cloned());
        }
    }
    out
}

/// 1つの W についてポアソン逸脱度を計算する。予測が0以下で `y>0` の辺があれば `None`。
fn deviance_at(
    entries: &[FitEntry],
    zero_y_train: f64,
    r_sum: f64,
    length_total: f64,
    years: f64,
    w: f64,
) -> Option<f64> {
    let with_validate_length: f64 = entries
        .iter()
        .filter(|e| e.stat.validate > 0.0)
        .map(|e| e.length)
        .sum();
    let zero_y_length = (length_total - with_validate_length).max(0.0);
    let mut d = 2.0 * ((1.0 - w) * r_sum * zero_y_length + w / years * zero_y_train);
    for entry in entries {
        let y = entry.stat.validate;
        if y <= 0.0 {
            continue;
        }
        let p = (1.0 - w) * r_sum * entry.length + w / years * entry.stat.train;
        if !(p.is_finite() && p > 0.0) {
            return None;
        }
        d += 2.0 * (y * (y / p).ln() - (y - p));
    }
    d.is_finite().then_some(d)
}

/// W の候補から、逸脱度が最小のものを選ぶ。
fn fit_weights(
    entries: &[FitEntry],
    length_total: f64,
    r_sum: f64,
    train_total: f64,
    years: f64,
) -> WFit {
    if !(r_sum.is_finite() && r_sum > 0.0 && length_total > 0.0) {
        let validate: f64 = entries.iter().map(|e| e.stat.validate).sum();
        return WFit::empty((validate == 0.0).then_some(0.0));
    }
    let with_validate: f64 = entries
        .iter()
        .filter(|e| e.stat.validate > 0.0)
        .map(|e| e.stat.train)
        .sum();
    let zero_y_train = (train_total - with_validate).max(0.0);
    let mut best: Option<(f64, f64)> = None;
    for &w in &W_GRID {
        if let Some(d) = deviance_at(entries, zero_y_train, r_sum, length_total, years, w) {
            // 差が浮動小数の誤差の範囲なら、候補の順（小さいW）を優先する。
            if best.is_none_or(|(_, bd)| d < bd - 1e-9 * (1.0 + bd.abs())) {
                best = Some((w, d));
            }
        }
    }
    let low = deviance_at(entries, zero_y_train, r_sum, length_total, years, W_GRID[0]);
    let high = deviance_at(
        entries,
        zero_y_train,
        r_sum,
        length_total,
        years,
        W_GRID[W_GRID.len() - 1],
    );
    match best {
        Some((w, d)) => WFit {
            w,
            deviance: Some(d),
            deviance_low: low,
            deviance_high: high,
            parent: None,
        },
        None => WFit {
            w: W_GRID[0],
            deviance: None,
            deviance_low: low,
            deviance_high: high,
            parent: None,
        },
    }
}

/// `a = T·R·(1/W − 1)`。W=0 のときは0（R=0 とみなす）。
fn coefficient_a(years: f64, r_sum: f64, w: f64) -> f64 {
    if r_sum > 0.0 && w > 0.0 {
        years * r_sum * (1.0 / w - 1.0)
    } else {
        0.0
    }
}

/// 学習件数が少ない種別の W を親の W で置き換える。
fn apply_parent_weights(
    rows: &mut [Row],
    lengths: &[f64],
    stats: &[KindStat],
    entries: &[Vec<FitEntry>],
    cluster: bool,
    years: f64,
) -> Vec<WFit> {
    let mut fits: Vec<WFit> = vec![WFit::empty(Some(0.0)); rows.len()];
    let mut own: Vec<Option<WFit>> = vec![None; rows.len()];
    for k in 0..rows.len() {
        if lengths[k] <= 0.0 {
            continue;
        }
        let r = rows[k].r_sum();
        let fit = fit_weights(&entries[k], lengths[k], r, stats[k].total_n(), years);
        own[k] = Some(fit.clone());
        fits[k] = fit;
    }
    let mut cache: HashMap<String, WFit> = HashMap::new();
    for k in 0..rows.len() {
        if lengths[k] <= 0.0 || stats[k].total_n() >= FIT_MIN_COUNT {
            continue;
        }
        let parent = if cluster {
            cluster_parent(k, stats, lengths, FIT_MIN_COUNT)
        } else {
            edge_parent(k, stats, lengths, FIT_MIN_COUNT)
        };
        let key = format!("{}:{:?}", parent.level, parent.kinds);
        let parent_fit = cache.entry(key).or_insert_with(|| {
            let agg_entries = aggregate_entries(entries, &parent.kinds);
            let parent_length: f64 = parent.kinds.iter().map(|&j| lengths[j]).sum();
            let parent_train: f64 = parent.kinds.iter().map(|&j| stats[j].total_n()).sum();
            let parent_r = if parent_length > 0.0 {
                parent_train / (years * parent_length)
            } else {
                0.0
            };
            fit_weights(&agg_entries, parent_length, parent_r, parent_train, years)
        });
        let w = parent_fit.w;
        let r = rows[k].r_sum();
        if r > 0.0 {
            let with_validate: f64 = entries[k]
                .iter()
                .filter(|e| e.stat.validate > 0.0)
                .map(|e| e.stat.train)
                .sum();
            let zero_y_train = (stats[k].total_n() - with_validate).max(0.0);
            let d = deviance_at(&entries[k], zero_y_train, r, lengths[k], years, w);
            let low = own[k].as_ref().and_then(|f| f.deviance_low);
            let high = own[k].as_ref().and_then(|f| f.deviance_high);
            fits[k] = WFit {
                w,
                deviance: d,
                deviance_low: low,
                deviance_high: high,
                parent: Some(parent.level),
            };
        } else {
            fits[k] = WFit {
                w: 0.0,
                deviance: Some(0.0),
                deviance_low: Some(0.0),
                deviance_high: Some(0.0),
                parent: Some(parent.level),
            };
        }
    }
    for k in 0..rows.len() {
        if lengths[k] > 0.0 {
            rows[k].a = coefficient_a(years, rows[k].r_sum(), fits[k].w);
        }
    }
    fits
}

/// 天候の候補の指数（x）から、β の候補（10^x）を作る。
fn weather_grid(from: f64, to: f64) -> Vec<f64> {
    let steps = ((to - from) / 0.25).round() as usize;
    (0..=steps).map(|i| from + 0.25 * i as f64).collect()
}

/// 天候 `weather_index`（0〜2＝雨・霧・雪）の逸脱度。
#[allow(clippy::too_many_arguments)]
fn weather_deviance(
    entries: &[FitEntry],
    weather_index: usize,
    zero_y_train: f64,
    r_sum: f64,
    rho: f64,
    length_total: f64,
    years: f64,
    beta: f64,
) -> Option<f64> {
    let t_r_rho = years * r_sum * rho;
    let denominator = t_r_rho + beta;
    let w = if denominator > 0.0 {
        t_r_rho / denominator
    } else {
        0.0
    };
    let with_validate_length: f64 = entries
        .iter()
        .filter(|e| e.stat.validate_weather[weather_index] > 0.0)
        .map(|e| e.length)
        .sum();
    let zero_y_length = (length_total - with_validate_length).max(0.0);
    let mut d = 2.0 * ((1.0 - w) * r_sum * rho * zero_y_length + w / years * zero_y_train);
    for entry in entries {
        let y = entry.stat.validate_weather[weather_index];
        if y <= 0.0 {
            continue;
        }
        let p = (1.0 - w) * r_sum * rho * entry.length
            + w / years * entry.stat.train_weather[weather_index];
        if rho * r_sum <= 0.0 {
            // ρ_k=0（学習年にその天候の事故がない種別）なら W_w=0 で、予測は β によらず0になる。
            // β の比較に影響しないので、逸脱度に入れない。
            continue;
        }
        if !(p.is_finite() && p > 0.0) {
            return None;
        }
        d += 2.0 * (y * (y / p).ln() - (y - p));
    }
    d.is_finite().then_some(d)
}

/// 天候の母数 β を、雨・霧・雪の逸脱度の合計が最小になる候補から選ぶ。
fn fit_weather(
    rows: &[Row],
    lengths: &[f64],
    entries: &[Vec<FitEntry>],
    years: f64,
    exponents: &[f64],
) -> (f64, Vec<Value>) {
    let mut candidates = Vec::with_capacity(exponents.len());
    let mut best: Option<(f64, f64)> = None;
    for &x in exponents {
        let beta = 10_f64.powf(x);
        let mut total = 0.0;
        let mut valid = true;
        for k in 0..rows.len() {
            if lengths[k] <= 0.0 {
                continue;
            }
            let r_sum = rows[k].r_sum();
            for weather_index in 0..3 {
                let rho = rows[k].rho_weighted(weather_index + 2);
                let total_weather: f64 = entries[k]
                    .iter()
                    .map(|e| e.stat.train_weather[weather_index])
                    .sum();
                let with_validate: f64 = entries[k]
                    .iter()
                    .filter(|e| e.stat.validate_weather[weather_index] > 0.0)
                    .map(|e| e.stat.train_weather[weather_index])
                    .sum();
                let zero_y_train = (total_weather - with_validate).max(0.0);
                let Some(d) = weather_deviance(
                    &entries[k],
                    weather_index,
                    zero_y_train,
                    r_sum,
                    rho,
                    lengths[k],
                    years,
                    beta,
                ) else {
                    valid = false;
                    break;
                };
                total += d;
            }
            if !valid {
                break;
            }
        }
        let deviance = valid.then_some(total);
        candidates.push(json!({
            "x": x,
            "beta": beta,
            "deviance": deviance,
        }));
        if let Some(d) = deviance
            && best.is_none_or(|(_, bd)| d < bd)
        {
            best = Some((beta, d));
        }
    }
    let beta = best.map_or(10_f64.powf(exponents[0]), |(b, _)| b);
    (beta, candidates)
}

/// 辺の種別ごとの `FitEntry` を作る。
fn edge_fit_entries(
    graph: &GraphParts,
    edge_k: &[u8],
    map: HashMap<u32, FitStat>,
) -> Vec<Vec<FitEntry>> {
    let mut out: Vec<Vec<FitEntry>> = (0..EDGE_KINDS).map(|_| Vec::new()).collect();
    for (e, stat) in map {
        let k = edge_k.get(e as usize).copied().unwrap_or(EDGE_K_NO_RISK);
        if (k as usize) < EDGE_KINDS {
            out[k as usize].push(FitEntry {
                id: e,
                length: f64::from(graph.edges[e as usize].length_dm) / 10.0,
                stat,
            });
        }
    }
    for list in &mut out {
        list.sort_by_key(|entry| entry.id);
    }
    out
}

/// クラスタの種別ごとの `FitEntry` を作る。
fn cluster_fit_entries(cluster_j: &[u8], map: HashMap<u32, FitStat>) -> Vec<Vec<FitEntry>> {
    let mut out: Vec<Vec<FitEntry>> = (0..CLUSTER_KINDS).map(|_| Vec::new()).collect();
    for (c, stat) in map {
        let j = cluster_j.get(c as usize).copied().unwrap_or(0);
        if (j as usize) < CLUSTER_KINDS {
            out[j as usize].push(FitEntry {
                id: c,
                length: 1.0,
                stat,
            });
        }
    }
    for list in &mut out {
        list.sort_by_key(|entry| entry.id);
    }
    out
}

/// 推定を実行する。ファイルの読み書きはしない。
///
/// # Errors
/// 入力の件数・割り当ての番号・設定が不正なとき。
pub fn fit_risk(
    input: &RiskFitInput<'_>,
    config: &RiskFitConfig,
    log: &dyn Fn(&str),
) -> Result<RiskFitOutput, String> {
    validate_input(input, config)?;
    let years = config.train_years.len() as f64;
    let edges = input.graph.edges.len();
    let clusters = input.topology.cluster_count();

    log("EDGEKを決める");
    let edge_k = build_edge_k(input, &config.train_years, log);
    let cluster_j = build_cluster_j(input.topology);

    let mut edge_length = vec![0.0_f64; EDGE_KINDS];
    for (e, &k) in edge_k.iter().enumerate() {
        if (k as usize) < EDGE_KINDS {
            edge_length[k as usize] += f64::from(input.graph.edges[e].length_dm) / 10.0;
        }
    }
    let mut cluster_count = vec![0.0_f64; CLUSTER_KINDS];
    for &j in &cluster_j {
        if (j as usize) < CLUSTER_KINDS {
            cluster_count[j as usize] += 1.0;
        }
    }

    let mut edge_stats: Vec<Vec<KindStat>> = vec![vec![KindStat::default(); EDGE_KINDS]; 3];
    let mut cluster_stats: Vec<Vec<KindStat>> = vec![vec![KindStat::default(); CLUSTER_KINDS]; 3];
    let mut edge_fits: Vec<HashMap<u32, FitStat>> = vec![HashMap::new(); 3];
    let mut cluster_fits: Vec<HashMap<u32, FitStat>> = vec![HashMap::new(); 3];
    let mut accident_counts = [[0_u64; 2]; 3];
    let mut assignment_counts = [[0_u64; 2]; 3];
    // [mode][sidewalk/no_risk][train/validate]
    let mut skipped = [[[0_u64; 2]; 2]; 3];
    let mut skipped_total = 0_u64;

    log("事故の割り当てを集計する");
    for i in 0..input.accidents.len() {
        let year = input.accidents.source_year[i];
        let train = config.train_years.contains(&year);
        let validate = year == config.validate_year;
        if !train && !validate {
            continue;
        }
        let cell = input.accidents.period[i] as usize * 2 + input.accidents.day_type[i] as usize;
        if cell >= CELL_COUNT {
            return Err(format!("事故{i}のセルが範囲外です"));
        }
        let weather = input.accidents.weather[i] as usize;
        if !(1..=5).contains(&weather) {
            return Err(format!("事故{i}の天候コードが範囲外です"));
        }
        let weather_index = weather - 1;
        // 縮小の母数の対象は雨・霧・雪（天候コード3〜5）だけ。
        let fit_weather = weather_index.checked_sub(2);
        let severity =
            10.0 * f64::from(input.accidents.deaths[i]) + f64::from(input.accidents.injuries[i]);
        let modes = input.accidents.modes[i];
        let mut involved = [false; 3];
        for mode in Mode::ALL {
            if modes & mode.mask() != 0 {
                let m = crate::domain::routing::risk::mode_index(mode);
                involved[m] = true;
                if train {
                    accident_counts[m][0] += 1;
                }
                if validate {
                    accident_counts[m][1] += 1;
                }
            }
        }
        let targets = input
            .assignment
            .offsets
            .get(i)
            .zip(input.assignment.offsets.get(i + 1))
            .map(|(&a, &b)| &input.assignment.targets[a as usize..b as usize])
            .ok_or_else(|| format!("assignment.binのACCOFSが事故{i}で足りません"))?;
        for target in targets {
            let weight = f64::from(target.weight);
            let kind = TargetKind::from_code(target.kind)
                .ok_or_else(|| format!("事故{i}の割り当てのkind{}が不正です", target.kind))?;
            match kind {
                TargetKind::Edge => {
                    let e = target.target as usize;
                    let k = edge_k[e];
                    for mode in Mode::ALL {
                        let m = crate::domain::routing::risk::mode_index(mode);
                        if !involved[m] {
                            continue;
                        }
                        if train {
                            assignment_counts[m][0] += 1;
                        }
                        if validate {
                            assignment_counts[m][1] += 1;
                        }
                        if k == EDGE_K_SIDEWALK || k == EDGE_K_NO_RISK {
                            let special = usize::from(k == EDGE_K_NO_RISK);
                            if train {
                                skipped[m][special][0] += 1;
                            }
                            if validate {
                                skipped[m][special][1] += 1;
                            }
                            skipped_total += 1;
                            continue;
                        }
                        if train {
                            edge_stats[m][k as usize].add(cell, weight, severity, weather_index);
                        }
                        let entry = edge_fits[m].entry(target.target).or_default();
                        if train {
                            entry.add(false, weight, fit_weather);
                        }
                        if validate {
                            entry.add(true, weight, fit_weather);
                        }
                    }
                }
                TargetKind::Cluster => {
                    let c = target.target as usize;
                    let j = cluster_j[c];
                    for mode in Mode::ALL {
                        let m = crate::domain::routing::risk::mode_index(mode);
                        if !involved[m] {
                            continue;
                        }
                        if train {
                            assignment_counts[m][0] += 1;
                        }
                        if validate {
                            assignment_counts[m][1] += 1;
                        }
                        if train {
                            cluster_stats[m][j as usize].add(cell, weight, severity, weather_index);
                        }
                        let entry = cluster_fits[m].entry(target.target).or_default();
                        if train {
                            entry.add(false, weight, fit_weather);
                        }
                        if validate {
                            entry.add(true, weight, fit_weather);
                        }
                    }
                }
            }
        }
    }
    log(&format!(
        "  学習{}件 検証{}件 事故{}件",
        accident_counts.iter().map(|c| c[0]).sum::<u64>(),
        accident_counts.iter().map(|c| c[1]).sum::<u64>(),
        input.accidents.len()
    ));

    let edge_x_grid = weather_grid(-10.0, 0.0);
    let cluster_x_grid = weather_grid(-4.0, 4.0);
    let mut k_table = vec![CoefRecord::default(); TABLE_LEN];
    let mut j_table = vec![CoefRecord::default(); TABLE_LEN];
    let mut pooling_edges = Map::new();
    let mut pooling_clusters = Map::new();
    let mut fit_edges = Map::new();
    let mut fit_clusters = Map::new();
    let mut betas = [0.0_f64; 3];
    let mut betas_cluster = [0.0_f64; 3];

    let edge_entries: Vec<Vec<Vec<FitEntry>>> = edge_fits
        .into_iter()
        .map(|map| edge_fit_entries(input.graph, &edge_k, map))
        .collect();
    let cluster_entries: Vec<Vec<Vec<FitEntry>>> = cluster_fits
        .into_iter()
        .map(|map| cluster_fit_entries(&cluster_j, map))
        .collect();

    for mode in Mode::ALL {
        let m = crate::domain::routing::risk::mode_index(mode);
        let name = mode.as_str();
        log(&format!("{name}の係数を推定する"));

        let mut r_pool = Vec::new();
        let mut rows = fill_density(&edge_stats[m], &edge_length, years, false, &mut r_pool);
        let mut s_pool = Vec::new();
        fill_severity(&edge_stats[m], &edge_length, false, &mut rows, &mut s_pool);
        let edge_w_fits = apply_parent_weights(
            &mut rows,
            &edge_length,
            &edge_stats[m],
            &edge_entries[m],
            false,
            years,
        );
        let (beta, beta_candidates) =
            fit_weather(&rows, &edge_length, &edge_entries[m], years, &edge_x_grid);
        betas[m] = beta;

        for (k, row) in rows.iter().enumerate() {
            for cell in 0..CELL_COUNT {
                k_table[table_index(mode, k as u8, cell)] = CoefRecord {
                    r: row.r[cell],
                    s: row.s[cell],
                    a: row.a,
                    rho: row.rho[cell],
                    pad: 0.0,
                };
            }
        }
        let mut kinds = Map::new();
        for (k, row) in rows.iter().enumerate() {
            if edge_length[k] <= 0.0 {
                continue;
            }
            let fit = &edge_w_fits[k];
            kinds.insert(
                k.to_string(),
                json!({
                    "w": fit.w,
                    "a": row.a,
                    "deviance": fit.deviance,
                    "deviance_w_low": fit.deviance_low,
                    "deviance_w_high": fit.deviance_high,
                    "train_count": edge_stats[m][k].total_n(),
                    "validate_count": edge_entries[m][k].iter().map(|e| e.stat.validate).sum::<f64>(),
                    "parent": fit.parent,
                    "length_m": edge_length[k],
                    "r_all_cells": row.r_sum(),
                }),
            );
        }
        fit_edges.insert(
            name.to_owned(),
            json!({
                "kinds": kinds,
                "beta": beta,
                "beta_candidates": beta_candidates,
            }),
        );
        pooling_edges.insert(
            name.to_owned(),
            json!({
                "r": r_pool,
                "severity": s_pool,
            }),
        );

        let mut jr_pool = Vec::new();
        let mut jrows = fill_density(&cluster_stats[m], &cluster_count, years, true, &mut jr_pool);
        let mut js_pool = Vec::new();
        fill_severity(
            &cluster_stats[m],
            &cluster_count,
            true,
            &mut jrows,
            &mut js_pool,
        );
        let cluster_w_fits = apply_parent_weights(
            &mut jrows,
            &cluster_count,
            &cluster_stats[m],
            &cluster_entries[m],
            true,
            years,
        );
        let (beta_cluster, beta_cluster_candidates) = fit_weather(
            &jrows,
            &cluster_count,
            &cluster_entries[m],
            years,
            &cluster_x_grid,
        );
        betas_cluster[m] = beta_cluster;

        for (j, row) in jrows.iter().enumerate() {
            if cluster_count[j] <= 0.0 {
                continue;
            }
            for cell in 0..CELL_COUNT {
                j_table[table_index(mode, j as u8, cell)] = CoefRecord {
                    r: row.r[cell],
                    s: row.s[cell],
                    a: row.a,
                    rho: row.rho[cell],
                    pad: 0.0,
                };
            }
        }
        let mut kinds = Map::new();
        for (j, row) in jrows.iter().enumerate() {
            if cluster_count[j] <= 0.0 {
                continue;
            }
            let fit = &cluster_w_fits[j];
            kinds.insert(
                j.to_string(),
                json!({
                    "w": fit.w,
                    "a": row.a,
                    "deviance": fit.deviance,
                    "deviance_w_low": fit.deviance_low,
                    "deviance_w_high": fit.deviance_high,
                    "train_count": cluster_stats[m][j].total_n(),
                    "validate_count": cluster_entries[m][j].iter().map(|e| e.stat.validate).sum::<f64>(),
                    "parent": fit.parent,
                    "clusters": cluster_count[j],
                    "r_all_cells": row.r_sum(),
                }),
            );
        }
        fit_clusters.insert(
            name.to_owned(),
            json!({
                "kinds": kinds,
                "beta_cluster": beta_cluster,
                "beta_cluster_candidates": beta_cluster_candidates,
            }),
        );
        pooling_clusters.insert(
            name.to_owned(),
            json!({
                "r": jr_pool,
                "severity": js_pool,
            }),
        );
    }

    // 監査用の内訳
    let mut edgek_by_group = Map::new();
    let mut sidewalk_edges = 0_u64;
    let mut sidewalk_length = 0.0_f64;
    let mut no_risk_edges = 0_u64;
    let mut no_risk_length = 0.0_f64;
    let mut urban_edges = 0_u64;
    let mut urban_length = 0.0_f64;
    let mut total_length = 0.0_f64;
    for (e, &k) in edge_k.iter().enumerate() {
        let length = f64::from(input.graph.edges[e].length_dm) / 10.0;
        total_length += length;
        if k == EDGE_K_SIDEWALK {
            sidewalk_edges += 1;
            sidewalk_length += length;
            continue;
        }
        if k == EDGE_K_NO_RISK {
            no_risk_edges += 1;
            no_risk_length += length;
            continue;
        }
        let group = k / 8;
        let band = (k % 8) / 2;
        let urban = k % 2;
        let key = format!("{group}_{band}_{urban}");
        let entry = edgek_by_group
            .entry(key)
            .or_insert_with(|| json!({"edges": 0_u64, "length_m": 0.0_f64}));
        if let Some(obj) = entry.as_object_mut() {
            let edges_now = obj.get("edges").and_then(Value::as_u64).unwrap_or(0) + 1;
            let length_now = obj.get("length_m").and_then(Value::as_f64).unwrap_or(0.0) + length;
            obj.insert("edges".to_owned(), json!(edges_now));
            obj.insert("length_m".to_owned(), json!(length_now));
        }
        if urban == 1 {
            urban_edges += 1;
            urban_length += length;
        }
    }
    let mut clusj = Map::new();
    for &j in &cluster_j {
        let count = clusj
            .get(&j.to_string())
            .and_then(Value::as_u64)
            .unwrap_or(0)
            + 1;
        clusj.insert(j.to_string(), json!(count));
    }

    let mut groups = Map::new();
    for mode in Mode::ALL {
        let m = crate::domain::routing::risk::mode_index(mode);
        let mut list = Vec::new();
        for group in 0..16_u8 {
            let mut n = 0.0;
            let mut severity = 0.0;
            let mut length = 0.0;
            for (k, &kind_length) in edge_length.iter().enumerate() {
                if k / 8 == usize::from(group) {
                    n += edge_stats[m][k].total_n();
                    severity += edge_stats[m][k].total_severity();
                    length += kind_length;
                }
            }
            let r = if length > 0.0 {
                n / (years * length) * 1000.0
            } else {
                0.0
            };
            let s = if n > 0.0 { severity / n } else { 0.0 };
            list.push(json!({
                "group": group,
                "r_per_km_year": r,
                "s": s,
                "count": n,
                "length_m": length,
            }));
        }
        groups.insert(mode.as_str().to_owned(), Value::Array(list));
    }

    let mut accident_audit = Map::new();
    let mut assignment_audit = Map::new();
    let mut skipped_audit = Map::new();
    for mode in Mode::ALL {
        let m = crate::domain::routing::risk::mode_index(mode);
        let name = mode.as_str();
        accident_audit.insert(
            name.to_owned(),
            json!({
                "train": accident_counts[m][0],
                "validate": accident_counts[m][1],
            }),
        );
        assignment_audit.insert(
            name.to_owned(),
            json!({
                "train": assignment_counts[m][0],
                "validate": assignment_counts[m][1],
            }),
        );
        skipped_audit.insert(
            name.to_owned(),
            json!({
                "sidewalk": {"train": skipped[m][0][0], "validate": skipped[m][0][1]},
                "no_risk": {"train": skipped[m][1][0], "validate": skipped[m][1][1]},
            }),
        );
    }

    let urban_audit = json!({
        "cell_deg": URBAN_CELL_DEG,
        "min_ratio": URBAN_MIN_RATIO,
        "min_count": URBAN_MIN_COUNT,
        "edges": urban_edges,
        "length_m": urban_length,
        "edge_share": if edges > 0 { urban_edges as f64 / edges as f64 } else { 0.0 },
        "length_share": if total_length > 0.0 { urban_length / total_length } else { 0.0 },
    });

    let mut params = json!({
        "rules_version": "jp-risk-1",
        "train_source_years": config.train_years,
        "years": config.train_years.len() as u32,
        "validate_year": config.validate_year,
        "c": config.c,
        "c_groups": [5, 6, 7, 8],
        "beta": {"walk": betas[0], "bike": betas[1], "car": betas[2]},
        "beta_cluster": {
            "walk": betas_cluster[0],
            "bike": betas_cluster[1],
            "car": betas_cluster[2],
        },
        "weather_multiplier": {"rain": 2.0, "fog": 4.0, "snow": 4.0},
        "urban": urban_audit,
        "pooling": {"edges": pooling_edges, "clusters": pooling_clusters},
        "fit": {"edges": fit_edges, "clusters": fit_clusters},
        "edges": {"count": edges, "sidewalk": sidewalk_edges, "no_risk": no_risk_edges},
        "clusters": {"count": clusters},
    });
    stabilize_json(&mut params);
    params = json_roundtrip(&params);
    let typed: RiskParams = serde_json::from_value(params.clone())
        .map_err(|e| format!("PARAMSを組み立てられません: {e}"))?;

    let mut audit = json!({
        "rules_version": "jp-risk-1",
        "edgek": {
            "groups": edgek_by_group,
            "sidewalk": {"edges": sidewalk_edges, "length_m": sidewalk_length},
            "no_risk": {"edges": no_risk_edges, "length_m": no_risk_length},
            "total_edges": edges,
            "total_length_m": total_length,
        },
        "clusj": clusj,
        "accidents": accident_audit,
        "assignments": assignment_audit,
        "skipped_assignments": skipped_audit,
        "skipped_total": skipped_total,
        "urban": urban_audit,
        "pooling": {"edges": pooling_edges, "clusters": pooling_clusters},
        "fit": {"edges": fit_edges, "clusters": fit_clusters},
        "groups": groups,
        "edges": {"count": edges},
        "clusters": {"count": clusters},
    });
    stabilize_json(&mut audit);
    audit = json_roundtrip(&audit);

    Ok(RiskFitOutput {
        model: RiskModel {
            params: typed,
            raw_params: params,
            edge_k,
            cluster_j,
            k_table,
            j_table,
        },
        audit,
    })
}

#[cfg(test)]
mod tests {
    // テストの本体は `backend/tests/routing_risk_fit.rs` に置く。
}
