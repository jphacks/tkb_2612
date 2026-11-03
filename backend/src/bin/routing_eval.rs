//! 評価と計測のコマンド（担当：②、形式書18章）。
//!
//! ```sh
//! cargo run --release --manifest-path backend/Cargo.toml --bin routing_eval -- <サブコマンド> [--名前 値]...
//! ```
//!
//! | サブコマンド | 内容 |
//! |---|---|
//! | `legality` | 経路の合法性の検査（`domain::routing::legality`） |
//! | `bench` | 探索の計測（時間・メモリ） |
//! | `od-sample` | 出発地・目的地の組の抽出（M1.8。手段ごとの距離帯、シード固定） |
//! | `y-metrics` | リスク指標の評価 |
//! | `graph-stats` | グラフ一式（`graph.bin`・`restrictions.bin`・`topology.bin`・`odindex.bin`）を読み込んで、件数とRSSを表示する（M1の受け入れ条件） |
//! | `fastest-batch` | ODの一覧に対して、手段ごとに吸着 → 最短時間経路を実行し、結果をJSON Linesで出す（M1.7・M1.8） |
//! | `dump-time-model` | `graph.bin` の `TIMEMODL`（所要時間のモデル）をJSONで標準出力に書く（`--dir <DIR>`。M1.7の係数の探索用） |
//!
//! `graph-stats` は `--dir <DIR>`（4つの `.bin` があるディレクトリ）を受け取る。`--parts-only true` を
//! 付けると、`Graph::from_parts` と各 `validate` を省いて節だけを読む（検査にかかる時間の切り分け用）。
//!
//! `od-sample`：`--dir <DIR> --mode <walk|bike|car> --count N --seed S --min-m M --max-m M --out <FILE>`。
//! 任意：`--bbox <lat_min,lon_min,lat_max,lon_max>`（出発・到着の辺をこの範囲に限る）、`--prefix <ID接頭辞>`、
//! `--jitter-m <m>`（辺上の点からの無作為なずれ、既定15）、`--dest <edge|random-point>`（既定 `edge`）。
//! 吸着可能な辺から出発の辺を一様に選び、`edge` なら出発点から距離帯（直線距離）に入る吸着可能な辺から
//! 到着の辺を一様に選ぶ。`random-point` は、到着の点を出発点のまわりの距離帯の中の無作為な点（面積で一様）
//! にする（道のない場所も含むので、吸着できない組が出る。吸着の割合を見る補助用）。出力は1行1組のJSON
//! （`{"id","mode","o":{"lat","lon"},"d":{…},"straight_m","density500",…}`）。
//!
//! `fastest-batch`：`--dir <DIR> --ods <od.jsonl> --out <result.jsonl>`。任意：`--threads N`（既定4）、
//! `--timeout-s S`（1件ごとの期限、既定120）、`--coords true|false`（経路の座標を出す、既定true）、
//! `--twins true|false`（経路の辺の双子の折れ線を出す、既定false）、`--time-model <json>`（`graph.bin` の
//! `TIMEMODL` を、このJSONで置き換えて読む。係数の探索用で、P1の作り直し前の見積もりに使う）。
//! 1件ごとに、吸着（`snap_ms`）と最短時間経路（`search_ms`）の壁時計の時間を測る。作業領域と照合器は
//! 1スレッドの中のODをまたいで再利用する。

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use accident_route_backend::contract::geo::{METERS_PER_DEGREE, straight_distance_m};
use accident_route_backend::contract::{CancelToken, Interrupt, LatLon, Mode};
use accident_route_backend::data::routing_files::{
    decode_graph, decode_graph_parts, decode_odindex, decode_restrictions, decode_topology,
};
use accident_route_backend::domain::routing::graph::restrictions::TurnAutomaton;
use accident_route_backend::domain::routing::graph::synthetic::Rng;
use accident_route_backend::domain::routing::graph::time::{TimeModel, turn_angle_deg};
use accident_route_backend::domain::routing::graph::{
    Coord, DirEdge, EdgeId, Graph, HighwayClass, NodeId, access, node_flags,
};
use accident_route_backend::domain::routing::legality::{Violation, check_path};
use accident_route_backend::domain::routing::search::endpoints::{
    Endpoints, Placement, is_same_position, path_distance_dm, path_time_ds, placement,
    travelled_lengths_dm,
};
use accident_route_backend::domain::routing::search::fastest::{FastestWork, fastest_path_with};
use accident_route_backend::domain::routing::search::{SearchContext, SearchParams};
use accident_route_backend::domain::routing::snap::{OdIndex, SnapPoint, point_on_edge, snap};
use accident_route_backend::domain::routing::topology::Topology;
use accident_route_backend::domain::routing::{Landmarks, Restrictions};
use rayon::prelude::*;
use serde_json::{Value, json};

const USAGE: &str = "使い方: routing_eval <legality|bench|od-sample|y-metrics|graph-stats|fastest-batch|dump-time-model> [--名前 値]...";

/// `--名前 値` の組。名前は先頭の `--` を除いて持つ。
struct Options(BTreeMap<String, String>);

fn parse_options(args: impl Iterator<Item = std::ffi::OsString>) -> Result<Options, String> {
    let mut options = BTreeMap::new();
    let mut args = args;
    while let Some(flag) = args.next() {
        let flag = flag
            .to_str()
            .and_then(|f| f.strip_prefix("--"))
            .ok_or_else(|| format!("不明な引数です: {}", flag.to_string_lossy()))?
            .to_owned();
        let value = args
            .next()
            .ok_or_else(|| format!("--{flag} の値がありません"))?;
        let value = value.into_string().map_err(|v| {
            format!(
                "--{flag} の値が文字列ではありません: {}",
                v.to_string_lossy()
            )
        })?;
        if options.insert(flag.clone(), value).is_some() {
            return Err(format!("--{flag} が重複しています"));
        }
    }
    Ok(Options(options))
}

/// `/proc/self/status` の VmRSS・VmHWM（MB）。
fn memory_mb() -> (f64, f64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|kb| kb.parse::<f64>().ok())
            .map_or(f64::NAN, |kb| kb / 1024.0)
    };
    (field("VmRSS:"), field("VmHWM:"))
}

fn read_bin(dir: &Path, name: &str) -> Result<Vec<u8>, String> {
    std::fs::read(dir.join(name)).map_err(|e| format!("{name} を読めません: {e}"))
}

/// グラフ一式を読み込んで、件数とRSSを表示する。
fn graph_stats(options: &Options) -> Result<(), String> {
    let dir = options
        .0
        .get("dir")
        .map(Path::new)
        .ok_or("--dir が必要です")?;
    let parts_only = options.0.get("parts-only").is_some_and(|v| v == "true");
    let started = Instant::now();
    let report = |label: &str| {
        let (rss, hwm) = memory_mb();
        println!(
            "[{:>6.1}s] {label:<28} VmRSS={rss:.0}MB VmHWM={hwm:.0}MB",
            started.elapsed().as_secs_f64()
        );
    };
    report("開始");

    let bytes = read_bin(dir, "graph.bin")?;
    report("graph.bin を読んだ");
    let graph = if parts_only {
        None
    } else {
        Some(decode_graph(&bytes).map_err(|e| e.to_string())?.value)
    };
    let parts_copy;
    let (nodes, edges, shapes) = match &graph {
        Some(g) => {
            let p = g.parts();
            (p.node_ll.len(), p.edges.len(), p.shape.len())
        }
        None => {
            parts_copy = decode_graph_parts(&bytes).map_err(|e| e.to_string())?.value;
            (
                parts_copy.node_ll.len(),
                parts_copy.edges.len(),
                parts_copy.shape.len(),
            )
        }
    };
    drop(bytes);
    report("graph を復号した");
    println!("  ノード {nodes} 辺 {edges} 形状点 {shapes}");

    let bytes = read_bin(dir, "restrictions.bin")?;
    let restrictions = decode_restrictions(&bytes)
        .map_err(|e| e.to_string())?
        .value;
    drop(bytes);
    if let Some(g) = &graph {
        restrictions.validate(g).map_err(|e| e.to_string())?;
    }
    report("restrictions.bin");
    let seq_len: usize = restrictions.items.iter().map(|r| r.seq.len()).sum();
    println!(
        "  禁止 {} 件（列の長さの合計 {seq_len}）",
        restrictions.items.len()
    );
    for mode in Mode::ALL {
        let automaton = TurnAutomaton::build(&restrictions, mode);
        println!(
            "  {} の照合器の状態数 {}",
            mode.as_str(),
            automaton.state_count()
        );
    }
    report("照合器を作った");

    let bytes = read_bin(dir, "topology.bin")?;
    let topology = decode_topology(&bytes).map_err(|e| e.to_string())?.value;
    drop(bytes);
    if let Some(g) = &graph {
        topology.validate(g).map_err(|e| e.to_string())?;
    }
    report("topology.bin");
    let snappable = |mode: Mode| {
        (0..edges as u32)
            .filter(|&e| topology.snappable(e, mode))
            .count()
    };
    println!(
        "  クラスタ {} 双子の記録 {} 歩道の親 {} 吸着可能な辺 car={} bike={} walk={}",
        topology.cluster_count(),
        topology.twins.len(),
        topology.sidewalk_parents.len(),
        snappable(Mode::Car),
        snappable(Mode::Bike),
        snappable(Mode::Walk)
    );

    let bytes = read_bin(dir, "odindex.bin")?;
    let odindex = decode_odindex(&bytes).map_err(|e| e.to_string())?.value;
    drop(bytes);
    report("odindex.bin");
    println!(
        "  格子 {}x{} 空でないセル {} 登録 {}",
        odindex.params.rows,
        odindex.params.cols,
        odindex.cell_keys.len(),
        odindex.cell_edges.len()
    );
    report("完了（4ファイルを保持）");
    // 読み込んだものを最後まで保持して、上のRSSが常駐の値になるようにする
    std::hint::black_box((&graph, &restrictions, &topology, &odindex));
    Ok(())
}

/// 経路の合法性の検査。
fn legality(options: &Options) -> Result<(), String> {
    let _ = options;
    todo!("legality")
}

/// 探索の計測。
fn bench(options: &Options) -> Result<(), String> {
    let _ = options;
    todo!("bench")
}

// ---------------------------------------------------------------------------------------------
// 共通：読込、座標、JSON
// ---------------------------------------------------------------------------------------------

/// 最短時間経路の距離の上限（m、behavior.md 5章。③の `ROUTE_MAX_DISTANCE_M` と同じ値）。
const ROUTE_MAX_DISTANCE_M: f64 = 80_000.0;

/// 契約の直線距離の範囲（m、behavior.md 4章）。
const STRAIGHT_MIN_M: f64 = 20.0;
const STRAIGHT_MAX_M: f64 = 30_000.0;

/// 常駐データ（M1で作った4ファイル）。
struct Data {
    graph: Graph,
    restrictions: Restrictions,
    topology: Topology,
    odindex: OdIndex,
}

fn bool_option(options: &Options, name: &str, default: bool) -> Result<bool, String> {
    match options.0.get(name).map(String::as_str) {
        None => Ok(default),
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(v) => Err(format!("--{name} は true か false です: {v}")),
    }
}

fn number_option<T: std::str::FromStr>(
    options: &Options,
    name: &str,
    default: Option<T>,
) -> Result<T, String> {
    match options.0.get(name) {
        Some(v) => v
            .parse::<T>()
            .map_err(|_| format!("--{name} の値が数ではありません: {v}")),
        None => default.ok_or_else(|| format!("--{name} が必要です")),
    }
}

fn dir_option(options: &Options) -> Result<&Path, String> {
    options
        .0
        .get("dir")
        .map(Path::new)
        .ok_or_else(|| "--dir が必要です".to_owned())
}

fn parse_mode(text: &str) -> Result<Mode, String> {
    Mode::parse(text).ok_or_else(|| format!("手段は walk・bike・car のどれかです: {text}"))
}

/// グラフ一式を読む。`time_model` があれば、`graph.bin` の `TIMEMODL` をそのJSONで置き換える
/// （係数の探索用。ファイルは書き換えない）。`with_restrictions` が偽なら、右左折禁止は空にする。
fn load_data(
    dir: &Path,
    time_model: Option<&Path>,
    with_restrictions: bool,
) -> Result<Data, String> {
    let bytes = read_bin(dir, "graph.bin")?;
    let mut parts = decode_graph_parts(&bytes).map_err(|e| e.to_string())?.value;
    drop(bytes);
    if let Some(path) = time_model {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("{} を読めません: {e}", path.display()))?;
        let json: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        parts.time_model = TimeModel::from_json(&json).map_err(|e| e.to_string())?;
        eprintln!(
            "所要時間のモデルを置き換えました: {}",
            parts.time_model.version
        );
    }
    let graph = Graph::from_parts(parts).map_err(|e| e.to_string())?;
    let restrictions = if with_restrictions {
        let bytes = read_bin(dir, "restrictions.bin")?;
        let r = decode_restrictions(&bytes)
            .map_err(|e| e.to_string())?
            .value;
        r.validate(&graph).map_err(|e| e.to_string())?;
        r
    } else {
        Restrictions::empty()
    };
    let bytes = read_bin(dir, "topology.bin")?;
    let topology = decode_topology(&bytes).map_err(|e| e.to_string())?.value;
    drop(bytes);
    topology.validate(&graph).map_err(|e| e.to_string())?;
    let bytes = read_bin(dir, "odindex.bin")?;
    let odindex = decode_odindex(&bytes).map_err(|e| e.to_string())?.value;
    drop(bytes);
    Ok(Data {
        graph,
        restrictions,
        topology,
        odindex,
    })
}

/// 度を小数6桁（約0.1m）に丸める。ODの座標は、この値をJSONに書き、読んだ側も同じ値で吸着する。
fn round6(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

fn latlon_json(p: LatLon) -> Value {
    json!({ "lat": p.lat, "lon": p.lon })
}

fn json_latlon(v: &Value) -> Option<LatLon> {
    let lat = v.get("lat")?.as_f64()?;
    let lon = v.get("lon")?.as_f64()?;
    (lat.is_finite() && lon.is_finite()).then(|| LatLon::new(lat, lon))
}

/// 辺の両端の中点（座標の平均）。距離帯の粗い絞り込み用。
fn edge_mid(graph: &Graph, e: EdgeId) -> Coord {
    let r = graph.edge(e);
    let (a, b) = (graph.node_coord(r.tail), graph.node_coord(r.head));
    Coord::new(
        ((i64::from(a.lat_e7) + i64::from(b.lat_e7)) / 2) as i32,
        ((i64::from(a.lon_e7) + i64::from(b.lon_e7)) / 2) as i32,
    )
}

/// 点を、半径 `radius_m` の円の中の無作為な位置へずらす（小数6桁に丸める）。
fn jittered(point: Coord, radius_m: f64, rng: &mut Rng) -> LatLon {
    let p = point.to_latlon();
    if radius_m <= 0.0 {
        return LatLon::new(round6(p.lat), round6(p.lon));
    }
    let r = radius_m * rng.unit();
    let theta = std::f64::consts::TAU * rng.unit();
    let cos_lat = p.lat.to_radians().cos().max(0.05);
    LatLon::new(
        round6(p.lat + r * theta.cos() / METERS_PER_DEGREE),
        round6(p.lon + r * theta.sin() / (METERS_PER_DEGREE * cos_lat)),
    )
}

/// 辺の上の無作為な点（始点・終点を除く）。
fn random_point_on_edge(graph: &Graph, e: EdgeId, rng: &mut Rng) -> Coord {
    let length = graph.edge(e).length_dm;
    let offset = if length > 2 {
        1 + rng.below(length as usize - 1) as u32
    } else {
        length / 2
    };
    point_on_edge(graph, e, offset)
}

/// 点から `reach_m` 以内の格子のセルを通る辺を `out` に足す（重複あり）。
fn edges_in_window(index: &OdIndex, center: Coord, reach_m: f64, out: &mut Vec<EdgeId>) {
    let p = &index.params;
    let my = METERS_PER_DEGREE / 1e7;
    let cos_lat = (f64::from(center.lat_e7) / 1e7)
        .to_radians()
        .cos()
        .max(0.05);
    let mx = my * cos_lat;
    let dlat = (reach_m / my).ceil() as i64;
    let dlon = (reach_m / mx).ceil() as i64;
    let cell =
        |value: i64, base: i32, size: i32| (value - i64::from(base)).div_euclid(i64::from(size));
    let row_lo = cell(i64::from(center.lat_e7) - dlat, p.lat0_e7, p.cell_lat_e7).max(0);
    let row_hi =
        cell(i64::from(center.lat_e7) + dlat, p.lat0_e7, p.cell_lat_e7).min(i64::from(p.rows) - 1);
    let col_lo = cell(i64::from(center.lon_e7) - dlon, p.lon0_e7, p.cell_lon_e7).max(0);
    let col_hi =
        cell(i64::from(center.lon_e7) + dlon, p.lon0_e7, p.cell_lon_e7).min(i64::from(p.cols) - 1);
    if row_lo > row_hi || col_lo > col_hi {
        return;
    }
    for row in row_lo..=row_hi {
        let first = (row * i64::from(p.cols) + col_lo) as u32;
        let last = (row * i64::from(p.cols) + col_hi) as u32;
        let lo = index.cell_keys.partition_point(|&k| k < first);
        let hi = index.cell_keys.partition_point(|&k| k <= last);
        for i in lo..hi {
            let (a, b) = (index.cell_ofs[i] as usize, index.cell_ofs[i + 1] as usize);
            out.extend_from_slice(&index.cell_edges[a..b]);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// od-sample
// ---------------------------------------------------------------------------------------------

/// 範囲 `lat_min,lon_min,lat_max,lon_max`（度）。
#[derive(Debug, Clone, Copy, PartialEq)]
struct BBox {
    lat_min: f64,
    lon_min: f64,
    lat_max: f64,
    lon_max: f64,
}

impl BBox {
    fn parse(text: &str) -> Result<BBox, String> {
        let v: Vec<f64> = text
            .split(',')
            .map(|t| t.trim().parse::<f64>())
            .collect::<Result<_, _>>()
            .map_err(|_| format!("--bbox は lat_min,lon_min,lat_max,lon_max です: {text}"))?;
        match v.as_slice() {
            &[lat_min, lon_min, lat_max, lon_max] if lat_min < lat_max && lon_min < lon_max => {
                Ok(BBox {
                    lat_min,
                    lon_min,
                    lat_max,
                    lon_max,
                })
            }
            _ => Err(format!(
                "--bbox は lat_min,lon_min,lat_max,lon_max（小さい順）です: {text}"
            )),
        }
    }

    fn contains(&self, c: Coord) -> bool {
        let p = c.to_latlon();
        (self.lat_min..=self.lat_max).contains(&p.lat)
            && (self.lon_min..=self.lon_max).contains(&p.lon)
    }
}

/// 出発・到着の辺にできる辺か：その手段で通れて、吸着可能で、範囲の中。
fn usable_edge(data: &Data, e: EdgeId, mode: Mode, bbox: Option<BBox>) -> bool {
    data.graph.edge(e).access & access::both(mode) != 0
        && data.topology.snappable(e, mode)
        && bbox.is_none_or(|b| b.contains(data.graph.node_coord(data.graph.edge(e).tail)))
}

/// ODの抽出（M1.8、計画書9.4節）。出発の辺を、吸着可能な辺から一様に選び、到着の辺を、出発点から
/// 距離帯（直線距離）に入る吸着可能な辺から一様に選ぶ。辺の上の無作為な点を、`--jitter-m` の範囲で
/// ずらしたものをODの点にする（地図上で道の近くを指す点を模す）。シードが同じなら結果は同じ。
fn od_sample(options: &Options) -> Result<(), String> {
    let dir = dir_option(options)?;
    let mode = parse_mode(options.0.get("mode").ok_or("--mode が必要です")?)?;
    let count: usize = number_option(options, "count", None)?;
    let seed: u64 = number_option(options, "seed", None)?;
    let min_m: f64 = number_option(options, "min-m", None)?;
    let max_m: f64 = number_option(options, "max-m", None)?;
    let jitter_m: f64 = number_option(options, "jitter-m", Some(15.0))?;
    let prefix = options.0.get("prefix").cloned().unwrap_or_default();
    let out = options.0.get("out").ok_or("--out が必要です")?;
    let random_dest = match options.0.get("dest").map(String::as_str) {
        None | Some("edge") => false,
        Some("random-point") => true,
        Some(v) => return Err(format!("--dest は edge か random-point です: {v}")),
    };
    let bbox = options.0.get("bbox").map(|t| BBox::parse(t)).transpose()?;
    if !(STRAIGHT_MIN_M..=STRAIGHT_MAX_M).contains(&min_m)
        || !(STRAIGHT_MIN_M..=STRAIGHT_MAX_M).contains(&max_m)
        || min_m >= max_m
    {
        return Err(format!(
            "距離帯は {STRAIGHT_MIN_M}〜{STRAIGHT_MAX_M}m の範囲で min-m < max-m です"
        ));
    }
    let started = Instant::now();
    let data = load_data(dir, None, false)?;
    let edge_count = data.graph.edge_count() as u32;
    let pool: Vec<EdgeId> = (0..edge_count)
        .filter(|&e| usable_edge(&data, e, mode, bbox))
        .collect();
    eprintln!(
        "[{:.1}s] 出発の辺の候補 {} 本（{}）",
        started.elapsed().as_secs_f64(),
        pool.len(),
        mode.as_str()
    );
    if pool.is_empty() {
        return Err("出発の辺の候補がありません".to_owned());
    }
    let mut rng = Rng::new(seed);
    let mut lines = Vec::with_capacity(count);
    let mut window = Vec::new();
    let mut candidates = Vec::new();
    let mut attempts = 0_usize;
    while lines.len() < count && attempts < count * 400 {
        attempts += 1;
        let origin_edge = *rng.pick(&pool);
        let origin_c = random_point_on_edge(&data.graph, origin_edge, &mut rng);
        let origin = jittered(origin_c, jitter_m, &mut rng);
        if snap(&data.graph, &data.topology, &data.odindex, origin, mode).is_none() {
            continue;
        }
        let origin_coord = Coord::from_latlon(origin);
        candidates.clear();
        if !random_dest {
            window.clear();
            edges_in_window(&data.odindex, origin_coord, max_m + 500.0, &mut window);
            window.sort_unstable();
            window.dedup();
            for &e in &window {
                if !usable_edge(&data, e, mode, bbox) {
                    continue;
                }
                let d = straight_distance_m(origin, edge_mid(&data.graph, e).to_latlon());
                if d >= min_m - 300.0 && d <= max_m + 300.0 {
                    candidates.push(e);
                }
            }
            if candidates.is_empty() {
                continue;
            }
        }
        // 周囲の辺の密度（都市部・郊外・地方の目安。手段によらず全辺を数える）
        window.clear();
        edges_in_window(&data.odindex, origin_coord, 500.0, &mut window);
        window.sort_unstable();
        window.dedup();
        let density500 = window.len();
        for _ in 0..30 {
            let (dest, dest_edge) = if random_dest {
                let r = (min_m * min_m + rng.unit() * (max_m * max_m - min_m * min_m)).sqrt();
                let theta = std::f64::consts::TAU * rng.unit();
                let cos_lat = origin.lat.to_radians().cos().max(0.05);
                let dest = LatLon::new(
                    round6(origin.lat + r * theta.cos() / METERS_PER_DEGREE),
                    round6(origin.lon + r * theta.sin() / (METERS_PER_DEGREE * cos_lat)),
                );
                (dest, None)
            } else {
                let dest_edge = *rng.pick(&candidates);
                let dest_c = random_point_on_edge(&data.graph, dest_edge, &mut rng);
                (jittered(dest_c, jitter_m, &mut rng), Some(dest_edge))
            };
            let straight = straight_distance_m(origin, dest);
            if !(min_m..=max_m).contains(&straight)
                || bbox.is_some_and(|b| !b.contains(Coord::from_latlon(dest)))
                || (!random_dest
                    && snap(&data.graph, &data.topology, &data.odindex, dest, mode).is_none())
            {
                continue;
            }
            lines.push(
                json!({
                    "id": format!("{prefix}{}-{:04}", mode.as_str(), lines.len() + 1),
                    "mode": mode.as_str(),
                    "o": latlon_json(origin),
                    "d": latlon_json(dest),
                    "straight_m": (straight * 10.0).round() / 10.0,
                    "density500": density500,
                    "origin_edge": origin_edge,
                    "dest_edge": dest_edge,
                })
                .to_string(),
            );
            break;
        }
    }
    if lines.len() < count {
        return Err(format!(
            "{} 組しか作れませんでした（試行 {attempts}）",
            lines.len()
        ));
    }
    let mut text = lines.join("\n");
    text.push('\n');
    std::fs::write(out, text).map_err(|e| format!("{out} に書けません: {e}"))?;
    eprintln!(
        "[{:.1}s] {} 組を {out} に書きました（試行 {attempts}）",
        started.elapsed().as_secs_f64(),
        count
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// fastest-batch
// ---------------------------------------------------------------------------------------------

/// 連結成分（その手段で通れる辺で、向きを無視してつないだもの）。経路がない理由の分類に使う。
struct Components {
    /// ノードごとの根（圧縮済み）。
    root: Vec<u32>,
    /// 根ごとのノード数（根でない添字は0）。
    size: Vec<u32>,
}

impl Components {
    fn build(graph: &Graph, mode: Mode) -> Components {
        let n = graph.node_count();
        let mut parent: Vec<u32> = (0..n as u32).collect();
        fn find(parent: &mut [u32], mut x: u32) -> u32 {
            while parent[x as usize] != x {
                let up = parent[parent[x as usize] as usize];
                parent[x as usize] = up;
                x = up;
            }
            x
        }
        for e in 0..graph.edge_count() as u32 {
            let r = graph.edge(e);
            if r.access & access::both(mode) == 0 {
                continue;
            }
            let (a, b) = (find(&mut parent, r.tail), find(&mut parent, r.head));
            if a != b {
                parent[a.max(b) as usize] = a.min(b);
            }
        }
        let mut size = vec![0_u32; n];
        for v in 0..n as u32 {
            let root = find(&mut parent, v);
            parent[v as usize] = root;
            size[root as usize] += 1;
        }
        Components { root: parent, size }
    }

    /// 辺が属する成分の（根, ノード数）。
    fn of_edge(&self, graph: &Graph, e: EdgeId) -> (u32, u32) {
        let root = self.root[graph.edge(e).tail as usize];
        (root, self.size[root as usize])
    }
}

/// 入力のOD。
struct Od {
    id: String,
    mode: Mode,
    origin: LatLon,
    dest: LatLon,
}

fn read_ods(path: &str) -> Result<Vec<Od>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path} を読めません: {e}"))?;
    let mut ods = Vec::new();
    for (i, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let v: Value = serde_json::from_str(line)
            .map_err(|e| format!("{path} の {} 行目がJSONではありません: {e}", i + 1))?;
        let bad = || format!("{path} の {} 行目に id・mode・o・d がありません", i + 1);
        let id = v.get("id").and_then(Value::as_str).ok_or_else(bad)?;
        let mode = v
            .get("mode")
            .and_then(Value::as_str)
            .and_then(Mode::parse)
            .ok_or_else(bad)?;
        let origin = v.get("o").and_then(json_latlon).ok_or_else(bad)?;
        let dest = v.get("d").and_then(json_latlon).ok_or_else(bad)?;
        ods.push(Od {
            id: id.to_owned(),
            mode,
            origin,
            dest,
        });
    }
    Ok(ods)
}

/// 辺の折れ線の、始点側から割合 `f0`〜`f1`（折れ線の長さに対する割合、`f0 ≤ f1`）の部分。
/// 割合は `snap::point_on_edge` と同じ規則（`offset_dm / length_dm` を折れ線の幾何学的な長さの割合とする）。
fn sub_polyline(line: &[Coord], f0: f64, f1: f64) -> Vec<Coord> {
    let lengths: Vec<f64> = line
        .windows(2)
        .map(|w| straight_distance_m(w[0].to_latlon(), w[1].to_latlon()))
        .collect();
    let total: f64 = lengths.iter().sum();
    if total <= 0.0 || line.len() < 2 {
        return vec![line[0]];
    }
    let lerp = |a: Coord, b: Coord, t: f64| {
        let f = |x: i32, y: i32| (f64::from(x) + t * (f64::from(y) - f64::from(x))).round() as i32;
        Coord::new(f(a.lat_e7, b.lat_e7), f(a.lon_e7, b.lon_e7))
    };
    // 折れ線の始点からの距離 `d` の点
    let point_at = |d: f64| {
        let mut remaining = d.clamp(0.0, total);
        for (i, &len) in lengths.iter().enumerate() {
            if remaining <= len && len > 0.0 {
                return lerp(line[i], line[i + 1], (remaining / len).clamp(0.0, 1.0));
            }
            remaining -= len;
        }
        line[line.len() - 1]
    };
    let (p0, p1) = (f0.clamp(0.0, 1.0) * total, f1.clamp(0.0, 1.0) * total);
    let mut out = vec![point_at(p0)];
    let mut acc = 0.0;
    for (i, &len) in lengths.iter().enumerate() {
        acc += len;
        if acc > p0 && acc < p1 {
            out.push(line[i + 1]);
        }
    }
    out.push(point_at(p1));
    out
}

/// 経路の座標（behavior.md 5章「経路の値と形状」）：出発の吸着点、通った有向辺の形状点とノード、
/// 到着の吸着点の順。直前と同じ座標は除く。
fn route_coords(
    graph: &Graph,
    start: &SnapPoint,
    end: &SnapPoint,
    dedges: &[DirEdge],
) -> Vec<Coord> {
    let start_inside = placement(graph, start) == Some(Placement::Inside);
    let end_inside = placement(graph, end) == Some(Placement::Inside);
    let mut out: Vec<Coord> = Vec::new();
    for (i, &d) in dedges.iter().enumerate() {
        let e = d.edge();
        let length = f64::from(graph.edge(e).length_dm).max(1.0);
        let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
        if i == 0 && start_inside {
            let f = f64::from(start.offset_dm) / length;
            if d.is_reverse() {
                hi = f;
            } else {
                lo = f;
            }
        }
        if i + 1 == dedges.len() && end_inside {
            let f = f64::from(end.offset_dm) / length;
            if d.is_reverse() {
                lo = f;
            } else {
                hi = f;
            }
        }
        let mut piece = sub_polyline(&graph.edge_polyline(e), lo.min(hi), hi.max(lo));
        if d.is_reverse() {
            piece.reverse();
        }
        for c in piece {
            if out.last() != Some(&c) {
                out.push(c);
            }
        }
    }
    out
}

fn coords_json(coords: &[Coord]) -> Value {
    Value::Array(
        coords
            .iter()
            .map(|c| {
                let p = c.to_latlon();
                json!([round6(p.lat), round6(p.lon)])
            })
            .collect(),
    )
}

/// 経路の説明変数（M1.7の回帰用）。
///
/// - `class_m`：種類（`class` の値）ごとの走行距離（m）。carで `maxspeed` のある辺は含めない。
/// - `ms_m`：carで `maxspeed` のある辺の、`"<class>:<maxspeed_kmh>"` ごとの走行距離（m）。
/// - `node_masks`：途中のノードの個数（添字は信号=1・一時停止=2・踏切=4のビットの組）。
/// - `turns`：隣り合う有向辺の組の個数（`right`・`left`・`u_turn`。現在のモデルのしきい値で分けたもの）と、
///   直進以外（変化角の絶対値が10度を超えるもの）の角度（度、時計回りが正、Uターンは180）。
fn route_features(
    graph: &Graph,
    mode: Mode,
    start: &SnapPoint,
    end: &SnapPoint,
    dedges: &[DirEdge],
) -> Option<Value> {
    let lengths = travelled_lengths_dm(graph, start, end, dedges)?;
    let params = graph.time_model().turn.get(mode);
    let mut class_m = [0.0_f64; HighwayClass::COUNT];
    let mut ms_m: BTreeMap<String, f64> = BTreeMap::new();
    let mut node_masks = [0_u32; 8];
    let (mut right, mut left, mut u_turn) = (0_u32, 0_u32, 0_u32);
    let mut angles: Vec<i32> = Vec::new();
    for (i, &d) in dedges.iter().enumerate() {
        let record = graph.edge(d.edge());
        let meters = f64::from(lengths[i]) / 10.0;
        if mode == Mode::Car && record.maxspeed_kmh > 0 {
            *ms_m
                .entry(format!("{}:{}", record.class, record.maxspeed_kmh))
                .or_default() += meters;
        } else {
            class_m[usize::from(record.class)] += meters;
        }
        let Some(&next) = dedges.get(i + 1) else {
            continue;
        };
        let flags = graph.node_flags(graph.dedge_to(d));
        let mask = usize::from(flags & node_flags::SIGNAL != 0)
            | usize::from(flags & node_flags::STOP != 0) << 1
            | usize::from(flags & node_flags::LEVEL_CROSSING != 0) << 2;
        node_masks[mask] += 1;
        if next == d.reversed() {
            u_turn += 1;
            angles.push(180);
        } else if let Some(angle) = turn_angle_deg(graph, d, next) {
            let magnitude = angle.abs();
            if magnitude > 10.0 {
                angles.push(angle.round() as i32);
            }
            if magnitude <= params.straight_max_deg {
            } else if magnitude >= params.u_turn_min_deg {
                u_turn += 1;
            } else if angle > 0.0 {
                right += 1;
            } else {
                left += 1;
            }
        }
    }
    let class_json: BTreeMap<String, f64> = class_m
        .iter()
        .enumerate()
        .filter(|&(_, &m)| m > 0.0)
        .map(|(c, &m)| (c.to_string(), (m * 10.0).round() / 10.0))
        .collect();
    let ms_json: BTreeMap<String, f64> = ms_m
        .into_iter()
        .map(|(k, m)| (k, (m * 10.0).round() / 10.0))
        .collect();
    Some(json!({
        "class_m": class_json,
        "ms_m": ms_json,
        "node_masks": node_masks,
        "turns": { "right": right, "left": left, "u_turn": u_turn, "angles": angles },
    }))
}

/// 同じ連結成分の中なのに経路がないときの原因の分類。出発から到着へ、辺の向きと通行可否だけでたどれるか
/// （通過できないノードと右左折禁止は見ない／通過できないノードを見る）を、ノード単位の幅優先で調べる。
///
/// - `reach_direction_only`：通行可能な向きだけでたどると、到着に着くか。
/// - `reach_with_barriers`：さらに、通過できないノードを途中で通らない条件を足しても着くか。
/// - どちらも着くなら、原因は右左折禁止（`turn_restriction`）。着かなければ、通過できないノードがあれば
///   `barrier_node`、なければ `one_way_or_access`（向き・通行可否）。
fn diagnose_unreachable(graph: &Graph, mode: Mode, start: &SnapPoint, end: &SnapPoint) -> Value {
    let Some(endpoints) = Endpoints::new(graph, mode, start, end) else {
        return json!({ "cause": "invalid_endpoints" });
    };
    let seeds: Vec<NodeId> = match endpoints.start_node() {
        Some(n) => vec![n],
        None => endpoints.departures.iter().map(|a| a.to).collect(),
    };
    let goals: Vec<NodeId> = match endpoints.end_node() {
        Some(n) => vec![n],
        None => endpoints.arrivals.iter().map(|a| a.from).collect(),
    };
    let bfs = |respect_barriers: bool| -> (bool, usize) {
        let mut seen = vec![false; graph.node_count()];
        let mut queue = std::collections::VecDeque::new();
        for &n in &seeds {
            if !std::mem::replace(&mut seen[n as usize], true) {
                queue.push_back(n);
            }
        }
        let mut count = 0_usize;
        let mut reached = false;
        while let Some(v) = queue.pop_front() {
            count += 1;
            // 出発・到着がノード上なら、そのノードは端点なので障害物でも使える。
            // 辺の途中なら、辺の端のノードは経路の途中のノードになる。
            let blocked = respect_barriers && graph.node_blocked(v, mode);
            let is_start = endpoints.start_node() == Some(v);
            let is_end = endpoints.end_node() == Some(v);
            if goals.contains(&v) && (!blocked || is_end) {
                reached = true;
            }
            // 通過できないノードは、そこへ着くことはできても、先へは進めない
            if blocked && !is_start {
                continue;
            }
            for &d in graph.out_dedges(v) {
                if !graph.passable(d, mode) {
                    continue;
                }
                let u = graph.dedge_to(d);
                if !std::mem::replace(&mut seen[u as usize], true) {
                    queue.push_back(u);
                }
            }
        }
        (reached, count)
    };
    let (direction_only, reach_nodes) = bfs(false);
    let (with_barriers, reach_nodes_barriers) = bfs(true);
    let cause = if with_barriers {
        "turn_restriction"
    } else if direction_only {
        "barrier_node"
    } else {
        "one_way_or_access"
    };
    let mut report = json!({
        "cause": cause,
        "reach_direction_only": direction_only,
        "reach_with_barriers": with_barriers,
        "reach_nodes_direction_only": reach_nodes,
        "reach_nodes_with_barriers": reach_nodes_barriers,
    });
    if cause == "barrier_node" {
        report["pocket"] = barrier_pocket(graph, mode, &endpoints, &seeds, &goals);
    }
    report
}

/// 通過できないノードで囲まれた小さな領域（ポケット）の大きさと、その境の障害物ノード。
/// 出発側（前向き）と到着側（後ろ向き）から、障害物を越えない幅優先を行い、先に尽きた方がポケット。
fn barrier_pocket(
    graph: &Graph,
    mode: Mode,
    endpoints: &Endpoints,
    seeds: &[NodeId],
    goals: &[NodeId],
) -> Value {
    const CAP: usize = 2_000_000;
    let walk =
        |from: &[NodeId], forward: bool, exempt: Option<NodeId>| -> (usize, Vec<NodeId>, bool) {
            let mut seen = std::collections::HashSet::new();
            let mut queue = std::collections::VecDeque::new();
            for &n in from {
                if seen.insert(n) {
                    queue.push_back(n);
                }
            }
            let mut frontier = Vec::new();
            while let Some(v) = queue.pop_front() {
                if seen.len() > CAP {
                    return (seen.len(), frontier, false);
                }
                if graph.node_blocked(v, mode) && exempt != Some(v) {
                    frontier.push(v);
                    continue;
                }
                for &d in graph.out_dedges(v) {
                    // 前向きなら d を進む、後ろ向きなら d の逆を進んで v に着く
                    let arc = if forward { d } else { d.reversed() };
                    if !graph.passable(arc, mode) {
                        continue;
                    }
                    let u = graph.dedge_to(d);
                    if seen.insert(u) {
                        queue.push_back(u);
                    }
                }
            }
            (seen.len(), frontier, true)
        };
    let (fwd_nodes, fwd_frontier, fwd_done) = walk(seeds, true, endpoints.start_node());
    let (bwd_nodes, bwd_frontier, bwd_done) = walk(goals, false, endpoints.end_node());
    let (side, nodes, frontier) = if fwd_done && (!bwd_done || fwd_nodes <= bwd_nodes) {
        ("origin", fwd_nodes, fwd_frontier)
    } else {
        ("destination", bwd_nodes, bwd_frontier)
    };
    let barriers: Vec<Value> = frontier
        .iter()
        .take(8)
        .map(|&n| {
            let c = graph.node_coord(n).to_latlon();
            json!({
                "node": n,
                "lat": round6(c.lat),
                "lon": round6(c.lon),
                "flags": graph.node_flags(n),
            })
        })
        .collect();
    json!({
        "side": side,
        "nodes": nodes,
        "closed": fwd_done || bwd_done,
        "barrier_nodes_total": frontier.len(),
        "barrier_nodes": barriers,
    })
}

fn violation_json(v: &Violation) -> Value {
    let (kind, index) = match *v {
        Violation::NotPassable { index } => ("NotPassable", index),
        Violation::WrongDirection { index } => ("WrongDirection", index),
        Violation::ForbiddenTurn { index } => ("ForbiddenTurn", index),
        Violation::Disconnected { index } => ("Disconnected", index),
        Violation::BarrierNode { index } => ("BarrierNode", index),
    };
    json!({ "kind": kind, "index": index })
}

fn snap_json(s: &Option<SnapPoint>) -> Value {
    match s {
        Some(p) => json!({
            "ok": true,
            "edge": p.edge,
            "offset_dm": p.offset_dm,
            "dist_m": (p.distance_m * 10.0).round() / 10.0,
            "lat": round6(p.position.to_latlon().lat),
            "lon": round6(p.position.to_latlon().lon),
        }),
        None => json!({ "ok": false }),
    }
}

/// 1スレッドが持つ作業領域。
#[derive(Default)]
struct Worker {
    work: FastestWork,
}

/// 1件のODを処理する。
struct Runner<'a> {
    data: &'a Data,
    automata: &'a [Option<TurnAutomaton>; 3],
    components: &'a [Option<Components>; 3],
    landmarks: &'a Landmarks,
    timeout: Duration,
    coords: bool,
    twins: bool,
}

fn mode_slot(mode: Mode) -> usize {
    match mode {
        Mode::Walk => 0,
        Mode::Bike => 1,
        Mode::Car => 2,
    }
}

/// 大きな探索のあとは作業領域の容量を手放す（スレッド数×全国サイズにしない）。
const RELEASE_ABOVE_TOUCHED: usize = 2_000_000;

impl Runner<'_> {
    fn run(&self, od: &Od, worker: &mut Worker) -> Value {
        let data = self.data;
        let graph = &data.graph;
        let mode = od.mode;
        let total_started = Instant::now();
        let mut rec = serde_json::Map::new();
        rec.insert("id".into(), json!(od.id));
        rec.insert("mode".into(), json!(mode.as_str()));
        rec.insert("origin".into(), latlon_json(od.origin));
        rec.insert("dest".into(), latlon_json(od.dest));
        rec.insert(
            "straight_m".into(),
            json!((straight_distance_m(od.origin, od.dest) * 10.0).round() / 10.0),
        );

        let snap_started = Instant::now();
        let start = snap(graph, &data.topology, &data.odindex, od.origin, mode);
        let end = snap(graph, &data.topology, &data.odindex, od.dest, mode);
        rec.insert(
            "snap_ms".into(),
            json!(snap_started.elapsed().as_secs_f64() * 1e3),
        );
        rec.insert("snap_o".into(), snap_json(&start));
        rec.insert("snap_d".into(), snap_json(&end));
        let finish =
            |mut rec: serde_json::Map<String, Value>, status: &str, reason: Option<&str>| {
                rec.insert("status".into(), json!(status));
                if let Some(r) = reason {
                    rec.insert("reason".into(), json!(r));
                }
                rec.insert(
                    "total_ms".into(),
                    json!(total_started.elapsed().as_secs_f64() * 1e3),
                );
                Value::Object(rec)
            };
        let (Some(start), Some(end)) = (start, end) else {
            let status = if start.is_none() {
                "no_snap_origin"
            } else {
                "no_snap_dest"
            };
            return finish(rec, status, None);
        };
        if is_same_position(graph, &start, &end) {
            return finish(rec, "same_position", None);
        }

        // 向きを無視しても別の連結成分なら、探索しても経路はない（探索は省く）
        if let Some(components) = &self.components[mode_slot(mode)] {
            let (root_o, size_o) = components.of_edge(graph, start.edge);
            let (root_d, size_d) = components.of_edge(graph, end.edge);
            rec.insert("comp_nodes_o".into(), json!(size_o));
            rec.insert("comp_nodes_d".into(), json!(size_d));
            if root_o != root_d {
                rec.insert("search_skipped".into(), json!(true));
                return finish(rec, "no_route", Some("disconnected_components"));
            }
        }

        let automaton = self.automata[mode_slot(mode)]
            .as_ref()
            .expect("手段の照合器を作った");
        let ctx = SearchContext {
            graph,
            topology: &data.topology,
            automaton,
            landmarks: self.landmarks,
            mode,
            params: SearchParams::DEFAULT,
        };
        let cancel = CancelToken::with_timeout(self.timeout);
        let search_started = Instant::now();
        let found = fastest_path_with(&ctx, &start, &end, &cancel, &mut worker.work);
        let search_ms = search_started.elapsed().as_secs_f64() * 1e3;
        let (touched_nodes, touched_states) = worker.work.touched();
        rec.insert("search_ms".into(), json!(search_ms));
        rec.insert("touched_nodes".into(), json!(touched_nodes));
        rec.insert("touched_states".into(), json!(touched_states));
        if touched_nodes + touched_states > RELEASE_ABOVE_TOUCHED {
            worker.work.release();
        }
        let path = match found {
            Err(Interrupt::Cancelled | Interrupt::DeadlineExceeded) => {
                return finish(rec, "interrupted", None);
            }
            Ok(None) => {
                rec.insert(
                    "diagnosis".into(),
                    diagnose_unreachable(graph, mode, &start, &end),
                );
                return finish(rec, "no_route", Some("unreachable_in_component"));
            }
            Ok(Some(path)) => path,
        };

        let distance_dm = path_distance_dm(graph, &start, &end, &path.dedges);
        let distance_m = distance_dm.map_or(f64::NAN, |dm| dm as f64 / 10.0);
        rec.insert("time_s".into(), json!(path.time_ds as f64 / 10.0));
        rec.insert("distance_m".into(), json!(distance_m));
        rec.insert("dedge_count".into(), json!(path.dedges.len()));
        // 検査：合法性、時間の正本との一致
        let violations: Vec<Value> = check_path(graph, &data.restrictions, mode, &path.dedges)
            .iter()
            .map(violation_json)
            .collect();
        rec.insert("violations".into(), Value::Array(violations));
        rec.insert(
            "time_matches_canonical".into(),
            json!(path_time_ds(graph, mode, &start, &end, &path.dedges) == Some(path.time_ds)),
        );
        if let Some(features) = route_features(graph, mode, &start, &end, &path.dedges) {
            rec.insert("features".into(), features);
        }
        if self.coords {
            let coords = route_coords(graph, &start, &end, &path.dedges);
            rec.insert("vertices".into(), json!(coords.len()));
            rec.insert("coords".into(), coords_json(&coords));
        }
        if self.twins {
            let mut seen = std::collections::BTreeSet::new();
            let mut lines = Vec::new();
            for d in &path.dedges {
                for twin in data.topology.twins_of(d.edge()) {
                    if seen.insert(twin) {
                        lines.push(json!({
                            "edge": d.edge(),
                            "twin": twin,
                            "class": graph.edge(twin).class,
                            "coords": coords_json(&graph.edge_polyline(twin)),
                        }));
                    }
                }
            }
            rec.insert("twin_lines".into(), Value::Array(lines));
        }
        let status = if distance_m > ROUTE_MAX_DISTANCE_M {
            "route_too_long"
        } else {
            "ok"
        };
        finish(rec, status, None)
    }
}

/// `graph.bin` の `TIMEMODL` をJSONで標準出力に書く。
fn dump_time_model(options: &Options) -> Result<(), String> {
    let dir = dir_option(options)?;
    let bytes = read_bin(dir, "graph.bin")?;
    let parts = decode_graph_parts(&bytes).map_err(|e| e.to_string())?.value;
    let json = serde_json::to_string_pretty(&parts.time_model).map_err(|e| e.to_string())?;
    println!("{json}");
    Ok(())
}

/// ODの一覧に対して、手段ごとに吸着 → 最短時間経路を実行し、結果をJSON Linesで出す。
fn fastest_batch(options: &Options) -> Result<(), String> {
    let dir = dir_option(options)?;
    let ods_path = options.0.get("ods").ok_or("--ods が必要です")?;
    let out_path = options.0.get("out").ok_or("--out が必要です")?;
    let threads: usize = number_option(options, "threads", Some(4))?;
    let timeout_s: f64 = number_option(options, "timeout-s", Some(120.0))?;
    let coords = bool_option(options, "coords", true)?;
    let twins = bool_option(options, "twins", false)?;
    let time_model = options.0.get("time-model").map(Path::new);
    if threads == 0 {
        return Err("--threads は1以上です".to_owned());
    }
    let started = Instant::now();
    let data = load_data(dir, time_model, true)?;
    let (rss, hwm) = memory_mb();
    eprintln!(
        "[{:.1}s] グラフ一式を読みました VmRSS={rss:.0}MB VmHWM={hwm:.0}MB",
        started.elapsed().as_secs_f64()
    );
    let ods = read_ods(ods_path)?;
    let mut automata: [Option<TurnAutomaton>; 3] = [None, None, None];
    let mut components: [Option<Components>; 3] = [None, None, None];
    for mode in Mode::ALL {
        if ods.iter().any(|od| od.mode == mode) {
            automata[mode_slot(mode)] = Some(TurnAutomaton::build(&data.restrictions, mode));
            components[mode_slot(mode)] = Some(Components::build(&data.graph, mode));
        }
    }
    eprintln!(
        "[{:.1}s] 照合器と連結成分を作りました。{} 組を {threads} スレッドで処理します",
        started.elapsed().as_secs_f64(),
        ods.len()
    );
    let landmarks = Landmarks::empty();
    let runner = Runner {
        data: &data,
        automata: &automata,
        components: &components,
        landmarks: &landmarks,
        timeout: Duration::from_secs_f64(timeout_s),
        coords,
        twins,
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .map_err(|e| e.to_string())?;
    let batch_started = Instant::now();
    let results: Vec<Value> = pool.install(|| {
        ods.par_iter()
            .map_init(Worker::default, |worker, od| runner.run(od, worker))
            .collect()
    });
    let batch_s = batch_started.elapsed().as_secs_f64();
    let mut text = String::new();
    for mut record in results {
        if let Value::Object(map) = &mut record {
            map.insert("threads".into(), json!(threads));
        }
        text.push_str(&record.to_string());
        text.push('\n');
    }
    std::fs::write(out_path, text).map_err(|e| format!("{out_path} に書けません: {e}"))?;
    let (rss, hwm) = memory_mb();
    eprintln!(
        "[{:.1}s] {} 組を {batch_s:.1}秒で処理し、{out_path} に書きました VmRSS={rss:.0}MB VmHWM={hwm:.0}MB",
        started.elapsed().as_secs_f64(),
        ods.len()
    );
    Ok(())
}

/// リスク指標の評価。
fn y_metrics(options: &Options) -> Result<(), String> {
    let _ = options;
    todo!("y-metrics")
}

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(command) = args.next() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let run: fn(&Options) -> Result<(), String> = match command.to_str() {
        Some("legality") => legality,
        Some("bench") => bench,
        Some("od-sample") => od_sample,
        Some("y-metrics") => y_metrics,
        Some("graph-stats") => graph_stats,
        Some("fastest-batch") => fastest_batch,
        Some("dump-time-model") => dump_time_model,
        _ => {
            eprintln!(
                "不明なサブコマンドです: {}\n{USAGE}",
                command.to_string_lossy()
            );
            return ExitCode::from(2);
        }
    };
    let options = match parse_options(args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("評価に失敗しました: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use accident_route_backend::domain::routing::graph::synthetic::{ORIGIN, grid};
    use accident_route_backend::domain::routing::prep::odindex::build_odindex;

    fn coord(lat: f64, lon: f64) -> Coord {
        Coord::from_latlon(LatLon::new(lat, lon))
    }

    #[test]
    fn bbox_parses_and_contains() {
        let b = BBox::parse("35.2,139.2,36.5,140.6").unwrap();
        assert!(b.contains(coord(35.68, 139.76)));
        assert!(!b.contains(coord(34.7, 135.5)));
        assert!(BBox::parse("36.5,139.2,35.2,140.6").is_err());
        assert!(BBox::parse("1,2,3").is_err());
        assert!(BBox::parse("a,b,c,d").is_err());
    }

    #[test]
    fn sub_polyline_cuts_by_length_fraction() {
        // 東へ約100m、さらに東へ約300m（全体の1/4と3/4）
        let a = coord(35.0, 139.0);
        let b = coord(35.0, 139.001);
        let c = coord(35.0, 139.004);
        let line = [a, b, c];
        let whole = sub_polyline(&line, 0.0, 1.0);
        assert_eq!(whole, vec![a, b, c]);
        // 割合 1/8〜3/8 は、1つ目の折れ点（1/4）を含む
        let part = sub_polyline(&line, 0.125, 0.375);
        assert_eq!(part.len(), 3);
        assert_eq!(part[1], b);
        assert!(part[0].lon_e7 > a.lon_e7 && part[0].lon_e7 < b.lon_e7);
        assert!(part[2].lon_e7 > b.lon_e7 && part[2].lon_e7 < c.lon_e7);
        // 同じ割合なら1点
        let point = sub_polyline(&line, 0.5, 0.5);
        assert_eq!(point.len(), 2);
        assert_eq!(point[0], point[1]);
    }

    #[test]
    fn route_coords_clips_the_first_and_last_edges() {
        let (builder, shape) = grid(1, 3, 100.0);
        let graph = builder.build();
        let n = |c| shape.node(0, c);
        let d01 = builder.dedge(n(0), n(1));
        let d12 = builder.dedge(n(1), n(2));
        // 出発は辺(0,1)の25m、到着は辺(1,2)の75m
        let start = SnapPoint::on_edge(&graph, d01.edge(), 250);
        let end = SnapPoint::on_edge(&graph, d12.edge(), 750);
        let coords = route_coords(&graph, &start, &end, &[d01, d12]);
        // 出発点、ノード1、到着点
        assert_eq!(coords.len(), 3);
        assert_eq!(coords[1], graph.node_coord(n(1)));
        assert!(coords[0].lon_e7 > graph.node_coord(n(0)).lon_e7);
        assert!(coords[0].lon_e7 < graph.node_coord(n(1)).lon_e7);
        assert!(coords[2].lon_e7 > graph.node_coord(n(1)).lon_e7);
        assert!(coords[2].lon_e7 < graph.node_coord(n(2)).lon_e7);
        // 出発と到着が同じ辺の途中で、逆向きに通る
        let start = SnapPoint::on_edge(&graph, d01.edge(), 800);
        let end = SnapPoint::on_edge(&graph, d01.edge(), 200);
        let coords = route_coords(&graph, &start, &end, &[d01.reversed()]);
        assert_eq!(coords.len(), 2);
        assert!(coords[0].lon_e7 > coords[1].lon_e7);
    }

    #[test]
    fn components_split_by_unpassable_edges() {
        let (mut builder, shape) = grid(1, 4, 100.0);
        let cut = builder.dedge(shape.node(0, 1), shape.node(0, 2)).edge();
        let spec = builder.edge_spec(cut).clone();
        *builder.edge_spec_mut(cut) = spec.deny(Mode::Car);
        let graph = builder.build();
        let car = Components::build(&graph, Mode::Car);
        let walk = Components::build(&graph, Mode::Walk);
        let edge = |a, b| builder.dedge(shape.node(0, a), shape.node(0, b)).edge();
        assert_ne!(
            car.of_edge(&graph, edge(0, 1)).0,
            car.of_edge(&graph, edge(2, 3)).0
        );
        assert_eq!(car.of_edge(&graph, edge(0, 1)).1, 2);
        assert_eq!(
            walk.of_edge(&graph, edge(0, 1)).0,
            walk.of_edge(&graph, edge(2, 3)).0
        );
        assert_eq!(walk.of_edge(&graph, edge(0, 1)).1, 4);
    }

    #[test]
    fn diagnosis_tells_barrier_one_way_and_turn_restriction_apart() {
        let (mut builder, shape) = grid(1, 3, 100.0);
        let n = |c| shape.node(0, c);
        let e01 = builder.dedge(n(0), n(1)).edge();
        let e12 = builder.dedge(n(1), n(2)).edge();
        // 通過できないノード
        builder.add_node_flags(n(1), node_flags::block_bit(Mode::Car));
        let graph = builder.build();
        let start = SnapPoint::on_edge(&graph, e01, 500);
        let end = SnapPoint::on_edge(&graph, e12, 500);
        let barrier = diagnose_unreachable(&graph, Mode::Car, &start, &end);
        assert_eq!(barrier["cause"], "barrier_node");
        // ポケット：障害物のノード（真ん中のノード）で止まった
        let pocket = &barrier["pocket"];
        assert_eq!(pocket["closed"], true);
        assert_eq!(pocket["barrier_nodes_total"], 1);
        assert_eq!(pocket["barrier_nodes"][0]["node"], n(1));
        let c = graph.node_coord(n(1)).to_latlon();
        assert!((pocket["barrier_nodes"][0]["lat"].as_f64().unwrap() - c.lat).abs() < 1e-6);
        // 歩行者は障害物を通れる
        let walk = diagnose_unreachable(&graph, Mode::Walk, &start, &end);
        assert_eq!(walk["cause"], "turn_restriction");

        // 一方通行：到着側の辺が、手前から入れない向き
        let (mut builder, shape) = grid(1, 3, 100.0);
        let n = |c| shape.node(0, c);
        let e01 = builder.dedge(n(0), n(1)).edge();
        let e12 = builder.dedge(n(1), n(2)).edge();
        let spec = builder.edge_spec(e12).clone();
        *builder.edge_spec_mut(e12) = spec.reverse_oneway();
        let graph = builder.build();
        let start = SnapPoint::on_edge(&graph, e01, 500);
        let end = SnapPoint::on_edge(&graph, e12, 500);
        let one_way = diagnose_unreachable(&graph, Mode::Car, &start, &end);
        assert_eq!(one_way["cause"], "one_way_or_access");
    }

    #[test]
    fn od_window_finds_nearby_edges_only() {
        let (builder, shape) = grid(3, 3, 1000.0);
        let parts = builder.parts();
        let index = build_odindex(&parts);
        let mut near = Vec::new();
        edges_in_window(&index, ORIGIN, 300.0, &mut near);
        near.sort_unstable();
        near.dedup();
        // 原点のノードに接する辺だけ（東と北）
        let at_origin: Vec<u32> = [
            builder.dedge(shape.node(0, 0), shape.node(0, 1)).edge(),
            builder.dedge(shape.node(0, 0), shape.node(1, 0)).edge(),
        ]
        .into();
        for e in &at_origin {
            assert!(near.contains(e));
        }
        let mut wide = Vec::new();
        edges_in_window(&index, ORIGIN, 5000.0, &mut wide);
        wide.sort_unstable();
        wide.dedup();
        assert_eq!(wide.len(), builder.edge_count());
        assert!(near.len() < wide.len());
    }

    #[test]
    fn jitter_stays_within_the_radius_and_is_deterministic() {
        let center = coord(35.0, 139.0);
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..50 {
            let p = jittered(center, 20.0, &mut a);
            assert_eq!(p, jittered(center, 20.0, &mut b));
            assert!(straight_distance_m(center.to_latlon(), p) <= 20.5);
        }
        let exact = jittered(center, 0.0, &mut a);
        assert_eq!(exact, LatLon::new(35.0, 139.0));
    }
}
