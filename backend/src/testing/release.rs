//! 人工releaseの生成（②の実エンジン試験、③のC3・C4用）。
//!
//! `route_example(name)` の `accident_ids` の事故だけを、前処理と同じ書き出し
//! （`data::release`）で17列のreleaseにする。IDの計算を二重に持たない。
//! `dataset_id`・`release_id` は実際のハッシュで、fixtureの0埋めとは異なる。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::NaiveDate;
use serde_json::{Map, json};

use crate::contract::{Calendar, Period};
use crate::data::holidays::parse_holiday_csv;
use crate::data::preprocess::rows::AccidentRecord;
use crate::data::release::{AccidentCsvWriter, build_release, write_release_files};

use super::fixtures::{RouteExample, route_example, route_example_accidents};
use super::router::{RouterResponse, ScriptedRouter, valid_routes};

/// 人工releaseに同梱する祝日表（2025年の国民の祝日・休日）。
///
/// 収録範囲は2025-01-01〜2025-12-31。範囲外（2024年以前・2026年以降）は土日と年末年始だけで
/// 判定され、`holiday_table_covered=false` になる。③がC3で `/api/conditions` を確かめるときは、
/// 範囲内（例：2025-11-24は振替休日、2025-05-06は振替休日）と範囲外（例：2026-01-05）の
/// 両方の日付を選べる。
pub const TEST_HOLIDAY_CSV: &str = "\
国民の祝日・休日月日,国民の祝日・休日名称
2025/1/1,元日
2025/1/13,成人の日
2025/2/11,建国記念の日
2025/2/23,天皇誕生日
2025/2/24,休日
2025/3/20,春分の日
2025/4/29,昭和の日
2025/5/3,憲法記念日
2025/5/4,みどりの日
2025/5/5,こどもの日
2025/5/6,休日
2025/7/21,海の日
2025/8/11,山の日
2025/9/15,敬老の日
2025/9/23,秋分の日
2025/10/13,スポーツの日
2025/11/3,文化の日
2025/11/23,勤労感謝の日
2025/11/24,休日
";

/// metadataの `generated_at`（人工releaseでは固定）。IDには影響しない。
const GENERATED_AT: &str = "2025-01-01T00:00:00+09:00";

/// 作ったreleaseの識別子。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseIds {
    pub dataset_id: String,
    pub release_id: String,
}

/// 時間帯の代表の時。
fn representative_hour(period: Period) -> u8 {
    match period {
        Period::EarlyMorning => 6,
        Period::Daytime => 12,
        Period::Evening => 17,
        Period::Night => 21,
        Period::LateNight => 2,
    }
}

/// 例の事故を17列の行にする。事故IDは行番号なので、`accident_ids` が `0..n` でなければ失敗。
pub fn example_records(
    example: &RouteExample,
    calendar: &dyn Calendar,
) -> Result<Vec<AccidentRecord>, String> {
    let ids: Vec<u64> = (0..example.accident_ids.len() as u64).collect();
    if example.accident_ids != ids {
        return Err(format!(
            "{}: accident_idsが0から始まる連番ではありません: {:?}",
            example.name, example.accident_ids
        ));
    }
    let accidents = route_example_accidents();
    example
        .accident_ids
        .iter()
        .map(|&id| {
            let a = accidents
                .iter()
                .find(|a| a.point.id == id)
                .ok_or_else(|| format!("{}: 事故ID{id}がfixtureにありません", example.name))?;
            let p = &a.point;
            let date = NaiveDate::from_ymd_opt(
                (p.date / 10_000) as i32,
                (p.date / 100) % 100,
                p.date % 100,
            )
            .ok_or_else(|| format!("事故ID{id}の日付が不正です"))?;
            let hour = representative_hour(p.period);
            Ok(AccidentRecord {
                lat: p.position.lat,
                lon: p.position.lon,
                modes: a.modes,
                period: p.period,
                weather: p.weather,
                // fixtureの曜日区分をそのまま書く（祝日表で再判定しない）
                day_type: p.day_type,
                deaths: p.deaths,
                injuries: p.injuries,
                source_year: p.source_year,
                date,
                hour,
                minute: 0,
                daynight: if (6..18).contains(&hour) { 12 } else { 22 },
                accident_type: 1,
                injury_a: if p.deaths > 0 { 1 } else { 2 },
                injury_b: 0,
                holiday_table_covered: calendar.classify_date(date).holiday_table_covered,
            })
        })
        .collect()
}

/// 例の人工releaseを `dir` に書き出す（ディレクトリは無ければ作る）。
pub fn write_example_release(example: &RouteExample, dir: &Path) -> std::io::Result<ReleaseIds> {
    let invalid = |m: String| std::io::Error::new(std::io::ErrorKind::InvalidInput, m);
    let holidays = TEST_HOLIDAY_CSV.as_bytes().to_vec();
    let calendar = parse_holiday_csv(&holidays).map_err(|e| invalid(e.to_string()))?;
    let mut writer = AccidentCsvWriter::new();
    for record in example_records(example, &calendar).map_err(invalid)? {
        writer.push(&record);
    }
    let mut extra = Map::new();
    extra.insert("test_example".to_owned(), json!(example.name));
    let content = build_release(writer.finish(), holidays, &calendar, GENERATED_AT, extra);
    write_release_files(dir, &content)?;
    Ok(ReleaseIds {
        dataset_id: content.dataset_id,
        release_id: content.release_id,
    })
}

/// 一時ディレクトリに作った人工release。破棄時にディレクトリごと消える。
#[derive(Debug)]
pub struct TestRelease {
    /// `data::load_engine` に渡すディレクトリ。
    dir: PathBuf,
    /// schema 3の人工releaseで、破棄する一時ディレクトリの根（schema 2では `None`）。
    root: Option<PathBuf>,
    pub ids: ReleaseIds,
}

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

impl TestRelease {
    /// `route_example(name)` の人工release。
    pub fn for_example(name: &str) -> Self {
        Self::from_example(&route_example(name))
    }

    pub fn from_example(example: &RouteExample) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "accident-route-release-{}-{}-{nanos}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed),
        ));
        let ids = write_example_release(example, &dir)
            .unwrap_or_else(|e| panic!("人工releaseを作れません: {e}"));
        Self {
            dir,
            root: None,
            ids,
        }
    }

    /// 小さな合成グラフから作ったschema 3の人工release。
    pub fn routing() -> (Self, RoutingTestQuery) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let root = std::env::temp_dir().join(format!(
            "accident-routing-release-{}-{}-{nanos}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed),
        ));
        let release = write_routing_test_release(&root)
            .unwrap_or_else(|e| panic!("schema 3の人工releaseを作れません: {e}"));
        let query = release.query();
        let ids = ReleaseIds {
            dataset_id: release.dataset_id,
            release_id: release.release_id,
        };
        (
            Self {
                dir: release.release_dir,
                root: Some(root),
                ids,
            },
            query,
        )
    }

    /// `data::load_calendar`・`data::load_engine` に渡すディレクトリ。
    pub fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for TestRelease {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.root.as_deref().unwrap_or(&self.dir));
    }
}

/// 例ごとの `ScriptedRouter`。初回は固定経路を返し、初回の先頭6本を重複排除して3本未満の例では
/// 追加の2回にも初回と同じ経路を返す（重複排除で候補は増えず、追加取得エラーの警告も出ない）。
/// 本数はエンジンと同じ基準（5桁fingerprintでの重複排除後）で数える。
///
/// ②のエンジン試験と③のC3が同じものを使う（#7〜#9 共通前提2章「C3の固定経路」）。
pub fn example_router(example: &RouteExample) -> ScriptedRouter {
    let initial = valid_routes(&example.routes);
    let mut responses: Vec<RouterResponse> = vec![initial];
    let distinct: std::collections::BTreeSet<_> = example
        .routes
        .iter()
        .take(crate::contract::routing::MAX_ROUTES_PER_CALL)
        .map(crate::domain::candidates::fingerprint)
        .collect();
    if distinct.len() < crate::domain::candidates::MIN_INITIAL_CANDIDATES {
        responses.push(valid_routes(&example.routes));
        responses.push(valid_routes(&example.routes));
    }
    ScriptedRouter::new(responses)
}

// ---------------------------------------------------------------------------------------------
// schema 3の人工release（M4.2・M4.4）
// ---------------------------------------------------------------------------------------------

use crate::contract::{LatLon, Mode};
use crate::data::release::{ACCIDENTS_FILE, ROUTING_FILES, sha256_hex};
use crate::data::routing_files::{
    Deps, encode_accident_attrs, encode_alt, encode_assignment, encode_graph, encode_odindex,
    encode_restrictions, encode_risk, encode_topology,
};
use crate::data::routing_package::{PackageInputs, package_routing_release};
use crate::domain::routing::NONE;
use crate::domain::routing::accident::{
    AccidentAttr, AccidentAttrTable, AssignTarget, Assignment, UnassignedReason, UnassignedRecord,
};
use crate::domain::routing::graph::synthetic::{EdgeSpec, GraphBuilder};
use crate::domain::routing::graph::{EdgeId, Graph, NodeId};
use crate::domain::routing::prep::odindex::build_odindex;
use crate::domain::routing::risk::{
    CoefRecord, RiskModel, RiskParams, TABLE_LEN, WeatherMultiplier,
};
use crate::domain::routing::search::alt::Landmarks;
use crate::domain::routing::topology::{ClusterRecord, Topology};

/// 人工のschema 3 releaseが確実に見つけられる検索。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoutingTestQuery {
    pub origin: LatLon,
    pub dest: LatLon,
    pub mode: Mode,
}

/// 書き出したschema 3の人工release。
#[derive(Debug, Clone)]
pub struct RoutingTestRelease {
    /// `data::load_engine` に渡すディレクトリ（`<dir>/releases/<release_id>`）。
    pub release_dir: PathBuf,
    pub release_id: String,
    pub dataset_id: String,
    pub origin: LatLon,
    pub dest: LatLon,
    pub mode: Mode,
}

impl RoutingTestRelease {
    /// このreleaseで確実に経路が見つかる検索。
    pub fn query(&self) -> RoutingTestQuery {
        RoutingTestQuery {
            origin: self.origin,
            dest: self.dest,
            mode: self.mode,
        }
    }
}

/// すべての辺を吸着可能にするビット列。
fn all_edge_bits(edges: usize) -> Vec<u64> {
    (0..edges.div_ceil(64))
        .map(|w| {
            let in_word = (edges - w * 64).min(64);
            if in_word == 64 {
                u64::MAX
            } else {
                (1_u64 << in_word) - 1
            }
        })
        .collect()
}

/// 人工releaseの合成グラフ。主鎖（0→39）と、途中のノードから出る行き止まりの枝。
fn routing_test_graph() -> (Graph, EdgeId, NodeId, NodeId) {
    let mut builder = GraphBuilder::new();
    let count = 40_usize;
    let mut main = Vec::with_capacity(count);
    for i in 0..count {
        main.push(builder.add_node_deg(35.6, 139.7 + i as f64 * 0.0005));
    }
    let mut main_edges = Vec::with_capacity(count - 1);
    for pair in main.windows(2) {
        main_edges.push(builder.add_edge(EdgeSpec::new(pair[0], pair[1]).length_dm(500)));
    }
    // 枝は行き止まりなので、主鎖の経路は1本に決まる。ノード数を数十にする。
    for &i in &[10_usize, 20, 30] {
        let base = builder.node_coord(main[i]).to_latlon();
        for sign in [-1.0_f64, 1.0] {
            let branch = builder.add_node_deg(base.lat + sign * 0.0005, base.lon);
            builder.add_edge(EdgeSpec::new(main[i], branch).length_dm(500));
            let tip = builder.add_node_deg(base.lat + sign * 0.001, base.lon);
            builder.add_edge(EdgeSpec::new(branch, tip).length_dm(500));
        }
    }
    let graph = builder.build();
    let last_edge = *main_edges.last().expect("主鎖の辺がある");
    (graph, last_edge, main[0], main[count - 1])
}

/// 主鎖の途中のノード（クラスタ）にだけ交差点クラスタを作る。
fn routing_test_topology(graph: &Graph, cluster_node: NodeId) -> Topology {
    let mut node_cluster = vec![NONE; graph.node_count()];
    node_cluster[cluster_node as usize] = 0;
    let coord = graph.node_coord(cluster_node);
    let bits = all_edge_bits(graph.edge_count());
    Topology {
        params: json!({ "rules_version": "jp-topology-2" }),
        snap_car: bits.clone(),
        snap_bike: bits.clone(),
        snap_walk: bits,
        node_cluster,
        clusters: vec![ClusterRecord {
            lat_e7: coord.lat_e7,
            lon_e7: coord.lon_e7,
            node_start: 0,
            node_count: 1,
            top_class: 11,
            branches: 5,
            flags: 0,
            pad: [0; 3],
        }],
        cluster_nodes: vec![cluster_node],
        twins: Vec::new(),
        sidewalk_parents: Vec::new(),
    }
}

fn routing_test_risk_params() -> RiskParams {
    RiskParams {
        train_source_years: vec![2025],
        years: 1,
        c: 1.0,
        c_groups: vec![5, 6, 7, 8],
        car_exposure: None,
        beta: crate::domain::routing::graph::time::PerMode {
            walk: 0.2,
            bike: 0.3,
            car: 0.4,
        },
        beta_cluster: crate::domain::routing::graph::time::PerMode {
            walk: 0.2,
            bike: 0.3,
            car: 0.4,
        },
        weather_multiplier: WeatherMultiplier {
            rain: 2.0,
            fog: 4.0,
            snow: 4.0,
        },
        urban: json!({}),
        pooling: json!({}),
        fit: json!({}),
    }
}

/// schema 2の人工release（`twoSelected`）をbaseにして、schema 3のreleaseを `dir` に書き出す。
///
/// `dir` は出力の根で、releaseは `<dir>/releases/<release_id>` にできる（読み取り専用）。
/// 8つの `.bin` は小さな合成グラフ（主鎖40ノード+枝12ノード）から作り、事故3件を
/// 辺・交差点クラスタ・帰属不明に1件ずつ割り当てる。経路は主鎖をたどるので、
/// どの候補でも `accident_count > 0` になる。
pub fn write_routing_test_release(dir: &Path) -> Result<RoutingTestRelease, String> {
    use std::fs;

    let base_release = dir.join("base");
    fs::create_dir_all(&base_release)
        .map_err(|e| format!("{} を作れません: {e}", base_release.display()))?;
    let example = route_example("twoSelected");
    write_example_release(&example, &base_release)
        .map_err(|e| format!("schema 2の人工releaseを作れません: {e}"))?;
    let rows = example.accident_ids.len() as u64;
    let accidents_bytes = fs::read(base_release.join(ACCIDENTS_FILE))
        .map_err(|e| format!("accidents.csv.gzを読めません: {e}"))?;
    let accidents_hash = sha256_hex(&accidents_bytes);

    let (graph, last_edge, origin_node, dest_node) = routing_test_graph();
    let graph_bytes = encode_graph(&graph);
    let graph_hash = sha256_hex(&graph_bytes);
    let graph_deps = Deps::from([("graph.bin".to_owned(), graph_hash.clone())]);

    let restrictions = crate::domain::routing::Restrictions::empty();
    let restrictions_bytes = encode_restrictions(&restrictions, &graph_deps);

    let cluster_node = 20;
    let topology = routing_test_topology(&graph, cluster_node);
    topology
        .validate(&graph)
        .map_err(|e| format!("合成のtopologyが不正です: {e}"))?;
    let topology_bytes = encode_topology(&topology, &graph_deps);
    let topology_hash = sha256_hex(&topology_bytes);
    let cluster_count = topology.cluster_count() as u64;

    let odindex = build_odindex(graph.parts());
    let odindex_bytes = encode_odindex(&odindex, &graph_deps);

    let accident_attrs = AccidentAttrTable {
        params: json!({ "rows": rows }),
        attrs: vec![AccidentAttr::MISSING; rows as usize],
    };
    let accident_attrs_deps = Deps::from([(ACCIDENTS_FILE.to_owned(), accidents_hash.clone())]);
    let accident_attrs_bytes = encode_accident_attrs(&accident_attrs, &accident_attrs_deps);
    let accident_attrs_hash = sha256_hex(&accident_attrs_bytes);

    // 事故0：目的地へ入る辺、事故1：主鎖の交差点クラスタ、事故2：帰属不明。
    let assignment = Assignment {
        params: json!({
            "accidents": rows,
            "targets": 2,
            "unassigned": 1,
            "edges": graph.edge_count(),
            "clusters": cluster_count,
        }),
        offsets: vec![0, 1, 2, 2],
        targets: vec![
            AssignTarget {
                target: last_edge,
                pos_dm: 250,
                weight: 1.0,
                kind: 0,
                flags: 0,
                reserved: 0,
            },
            AssignTarget {
                target: 0,
                pos_dm: u32::MAX,
                weight: 1.0,
                kind: 1,
                flags: 0,
                reserved: 0,
            },
        ],
        unassigned: vec![UnassignedRecord {
            accident: 2,
            reason: UnassignedReason::NoCandidate,
        }],
    };
    assignment
        .validate(graph.edge_count(), cluster_count as usize)
        .map_err(|e| format!("合成の割り当てが不正です: {e}"))?;
    let assignment_deps = Deps::from([
        ("graph.bin".to_owned(), graph_hash.clone()),
        ("topology.bin".to_owned(), topology_hash.clone()),
        ("accident_attrs.bin".to_owned(), accident_attrs_hash.clone()),
        (ACCIDENTS_FILE.to_owned(), accidents_hash.clone()),
    ]);
    let assignment_bytes = encode_assignment(&assignment, &assignment_deps);
    let assignment_hash = sha256_hex(&assignment_bytes);

    let risk = RiskModel {
        params: routing_test_risk_params(),
        raw_params: json!({}),
        edge_k: vec![0; graph.edge_count()],
        cluster_j: vec![0; cluster_count as usize],
        k_table: vec![CoefRecord::default(); TABLE_LEN],
        j_table: vec![CoefRecord::default(); TABLE_LEN],
    };
    let risk_deps = Deps::from([
        ("graph.bin".to_owned(), graph_hash.clone()),
        ("topology.bin".to_owned(), topology_hash.clone()),
        ("accident_attrs.bin".to_owned(), accident_attrs_hash),
        ("assignment.bin".to_owned(), assignment_hash),
        (ACCIDENTS_FILE.to_owned(), accidents_hash),
    ]);
    let risk_bytes = encode_risk(&risk, &risk_deps);
    let alt_bytes = encode_alt(&Landmarks::empty(), &graph_deps);

    let bins = dir.join("bins");
    fs::create_dir_all(&bins).map_err(|e| format!("{} を作れません: {e}", bins.display()))?;
    let contents: [&[u8]; 8] = [
        &graph_bytes,
        &restrictions_bytes,
        &topology_bytes,
        &odindex_bytes,
        &accident_attrs_bytes,
        &assignment_bytes,
        &risk_bytes,
        &alt_bytes,
    ];
    let mut paths: [PathBuf; 8] = std::array::from_fn(|_| PathBuf::new());
    for (i, (&file, bytes)) in ROUTING_FILES.iter().zip(contents).enumerate() {
        let path = bins.join(file);
        fs::write(&path, bytes).map_err(|e| format!("{} に書けません: {e}", path.display()))?;
        paths[i] = path;
    }

    let summary = package_routing_release(
        &PackageInputs {
            base_release,
            routing_files: paths,
            generated_at: "2025-01-01T00:00:00+09:00".to_owned(),
        },
        dir,
    )?;
    let origin = graph.node_coord(origin_node).to_latlon();
    let dest = graph.node_coord(dest_node).to_latlon();
    Ok(RoutingTestRelease {
        release_dir: summary.release_dir,
        release_id: summary.release_id,
        dataset_id: summary.dataset_id,
        origin,
        dest,
        mode: Mode::Car,
    })
}
