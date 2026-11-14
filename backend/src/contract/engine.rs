//! 計算エンジンの入口と結果型。
//!
//! ②が [`RouteEngine`] を実装し、③がAPIハンドラから呼ぶ。③はJSONの検証・正規化をして
//! [`RouteQuery`] を作り、[`RouteOutcome`] をOpenAPIのDTOへ変換する。
//! ID文字列（`route-N`、`route-N:segment-S`、`route-N:nav-S-E`）、`api_version`、
//! `exact_condition_count`、`points_shown`、`effective_conditions` の日本語表記は
//! この結果から③が組み立てる。数値・件数・順序・選定・文言は②が決め、③は再計算しない。

use std::ops::RangeInclusive;

use super::BoxFuture;
use super::cancel::{CancelToken, Interrupt};
use super::conditions::{ConditionFilter, DayType, Mode, Period, Weather};
use super::geo::{LatLon, LonLat, straight_distance_m};
use super::routing::{RoutedPath, Router, RoutingError};

/// 出発地と目的地の直線距離の下限（m、この値を含む）。上限はない（issue #39）。
pub const MIN_STRAIGHT_DISTANCE_M: f64 = 20.0;
/// 最短時間経路からの追加許容時間（分、両端を含む）。
pub const MAX_DETOUR_RANGE_MIN: RangeInclusive<f64> = 0.0..=30.0;
/// `max_detour_min` 省略時の値（分）。
pub const DEFAULT_MAX_DETOUR_MIN: f64 = 10.0;
/// 返す候補の上限。
pub const MAX_CANDIDATES: usize = 6;
/// 集合Bの経路周辺半径（m）。応答の `unattributed_radius_m`。新方式（schema 3）では、
/// 帰属不明の事故を集合Bに数える範囲（behavior.md 6章）。
pub const RADIUS_M: f64 = 72.0;
/// 距離重みのガウスσ（m）。旧方式（schema 2）の距離重みのσ。応答には出さない。
pub const DISTANCE_WEIGHT_SIGMA_M: f64 = 24.0;
/// 候補ごとの表示事故点の上限。
pub const MAX_ACCIDENT_POINTS: usize = 250;
/// 候補ごとの表示用高リスク区間の上限。
pub const MAX_HIGH_RISK_SECTIONS: usize = 100;

/// 最短時間経路のリスク `base` に対する `risk` の削減率（%、小数1桁）。
///
/// `100 × (base − risk) / base` を小数1桁に丸める（[`crate::domain::rounding::py_round`] と同じ
/// 二進値の10進丸め）。`base` が0以下なら算出できないので `None`。負の値はリスクが基準より
/// 高いことを表す。両エンジンと応答で同じ式を使うため、ここに1つだけ置く。
pub fn risk_reduction_percent(base: f64, risk: f64) -> Option<f64> {
    (base > 0.0).then(|| {
        format!("{:.1}", 100.0 * (base - risk) / base)
            .parse()
            .expect("固定桁で書式化した有限値は再び読める")
    })
}

/// routeの意味的な入力不正。すべて400 invalid_request。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueryError {
    InvalidOrigin,
    InvalidDest,
    StraightDistance,
    MaxDetour,
}

impl QueryError {
    /// 400応答の `message`。位置・入力値を含めない。
    pub const fn message(self) -> &'static str {
        match self {
            QueryError::InvalidOrigin => "出発地には日本国内の緯度・経度を指定してください。",
            QueryError::InvalidDest => "目的地には日本国内の緯度・経度を指定してください。",
            QueryError::StraightDistance => "出発地と目的地の直線距離は20m以上にしてください。",
            QueryError::MaxDetour => "追加を許容する時間は0〜30分で指定してください。",
        }
    }
}

/// 出発地・目的地と直線距離を検証する。behavior.md 4章の検証順では
/// mode等より先に行うため、③はDTOの解釈の途中でこれを先に呼んでよい。
pub fn validate_endpoints(origin: LatLon, dest: LatLon) -> Result<(), QueryError> {
    if !origin.is_in_japan() {
        return Err(QueryError::InvalidOrigin);
    }
    if !dest.is_in_japan() {
        return Err(QueryError::InvalidDest);
    }
    if !(MIN_STRAIGHT_DISTANCE_M..).contains(&straight_distance_m(origin, dest)) {
        return Err(QueryError::StraightDistance);
    }
    Ok(())
}

pub fn validate_max_detour(max_detour_min: f64) -> Result<(), QueryError> {
    if max_detour_min.is_finite() && MAX_DETOUR_RANGE_MIN.contains(&max_detour_min) {
        Ok(())
    } else {
        Err(QueryError::MaxDetour)
    }
}

/// 正規化・検証済みのroute入力。省略値は③が補ってから作る。
#[derive(Debug, Clone, PartialEq)]
pub struct RouteQuery {
    origin: LatLon,
    dest: LatLon,
    mode: Mode,
    max_detour_min: f64,
    conditions: ConditionFilter,
}

impl RouteQuery {
    pub fn new(
        origin: LatLon,
        dest: LatLon,
        mode: Mode,
        max_detour_min: f64,
        conditions: ConditionFilter,
    ) -> Result<Self, QueryError> {
        validate_endpoints(origin, dest)?;
        validate_max_detour(max_detour_min)?;
        Ok(Self {
            origin,
            dest,
            mode,
            max_detour_min,
            conditions,
        })
    }

    pub fn origin(&self) -> LatLon {
        self.origin
    }

    pub fn dest(&self) -> LatLon {
        self.dest
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn max_detour_min(&self) -> f64 {
        self.max_detour_min
    }

    pub fn conditions(&self) -> ConditionFilter {
        self.conditions
    }
}

/// 候補取得の結果。[`RouteEngine::collect_candidates`] から [`RouteEngine::evaluate`] へ渡す。
///
/// 中身の意味はエンジンが決める。候補を外部から取得するエンジン（旧方式の `AccidentEngine`）は
/// 下記の1〜6本を入れる。候補を `evaluate` の中で自前に探索するエンジン（新方式の
/// `RoutingEngine`）は、`routes` も `warnings` も空のまま返し、`evaluate` はこれを使わない。
/// ③は中身を検査せず、そのまま `evaluate` へ渡す。
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateSet {
    /// 旧方式：検証・重複排除・duration安定ソート済みの1〜6本。添字iが `route-(i+1)`。
    /// 新方式：空。
    pub routes: Vec<RoutedPath>,
    /// 旧方式：追加取得エラー（呼出し順）、1候補比較不能の警告。評価時の警告はこの後に続く。
    /// 新方式：空（1候補比較不能の警告は `evaluate` が入れる）。
    pub warnings: Vec<String>,
}

/// 計算の失敗。HTTP応答への変換は③が行う（README.mdの対応表）。
///
/// 各値の意味は、旧方式（schema 2の `AccidentEngine`）と新方式（schema 3の `RoutingEngine`）で
/// 次のとおり（behavior.md 5章・付録A.1）。HTTPへの対応は両方式で同じ。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EngineError {
    /// 502 no_route。旧方式：初回の経路なし、または全呼出し後に候補0。
    /// 新方式：出発地か目的地を道路へ吸着できない、2点が同じ位置に吸着された、経路がない。
    NoRoute,
    /// 422 route_too_long。旧方式：初回候補のdistanceまたはgeometry長が250km超。
    /// 新方式：最短時間経路の距離が250km超（それ以外の250km超の候補は、エラーにせず外す）。
    RouteTooLong,
    /// 502 external_api_error。旧方式：初回呼出しの外部失敗、または初回候補の値・形状の不正。
    /// 新方式：返さない（経路提供元を呼ばない）。
    ExternalApi,
    /// 待ち行列満杯・共有上流ゲートの失敗。503 service_busy。
    Busy,
    /// 期限切れ（503 service_busy）または取消し（応答なし）。
    Interrupted(Interrupt),
    /// ②の内部契約違反（相関制約の不整合等）。500 internal_error。
    ContractViolation(String),
}

impl From<Interrupt> for EngineError {
    fn from(value: Interrupt) -> Self {
        EngineError::Interrupted(value)
    }
}

impl From<RoutingError> for EngineError {
    fn from(value: RoutingError) -> Self {
        match value {
            RoutingError::NoRoute => EngineError::NoRoute,
            RoutingError::ExternalApi => EngineError::ExternalApi,
            RoutingError::Busy => EngineError::Busy,
            RoutingError::Interrupted(i) => EngineError::Interrupted(i),
        }
    }
}

/// 事故の件数・人数。内部加算はu64。JSONの安全整数範囲の検査は③がシリアライズ時に行う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct AccidentTotals {
    pub accident_count: u64,
    pub fatal_accidents: u64,
    pub injury_accidents: u64,
    pub deaths: u64,
    pub injuries: u64,
}

/// 表示事故点（間引き後）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AccidentPoint {
    /// gzip CSVのデータ行0始まり。`dataset_id` と組で識別する。
    pub id: u64,
    pub position: LonLat,
    pub deaths: u16,
    pub injuries: u16,
    pub source_year: u16,
    /// 発生日YYYYMMDD。
    pub date: u32,
    pub weather: Weather,
    pub period: Period,
    pub day_type: DayType,
    pub attribution: Attribution,
}

/// 事故を経路に結び付けた方法。応答の `attribution`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attribution {
    /// 道路の区間（歩道の親の車道を含む）。
    Edge,
    /// 交差点。
    Intersection,
    /// どの道路の事故か判断できない。旧方式は常にこれ。
    Unattributed,
}

impl Attribution {
    pub const ALL: [Attribution; 3] = [
        Attribution::Edge,
        Attribution::Intersection,
        Attribution::Unattributed,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Attribution::Edge => "edge",
            Attribution::Intersection => "intersection",
            Attribution::Unattributed => "unattributed",
        }
    }

    pub fn from_label(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.label() == s)
    }
}

/// 表示用の高リスク小線分。終点の元頂点番号は常に `source_vertex_start_index + 1`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HighRiskSegment {
    /// 75m分割後の小線分の0始まりindex（sid）。
    pub segment_index: u32,
    pub source_vertex_start_index: u32,
    /// 補間済みの両端。
    pub start: LonLat,
    pub end: LonLat,
    pub severity_weight: u64,
}

/// ナビ用の結合区間（両端のsidを含む）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavigationSection {
    pub start_segment_index: u32,
    pub end_segment_index: u32,
    pub start_m: f64,
    pub end_m: f64,
    pub severity_weight: u64,
}

/// 進行方向案内の種類（behavior.md 7章）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Maneuver {
    Left,
    Right,
    SlightLeft,
    SlightRight,
    Uturn,
    Arrive,
}

impl Maneuver {
    pub const fn as_str(self) -> &'static str {
        match self {
            Maneuver::Left => "left",
            Maneuver::Right => "right",
            Maneuver::SlightLeft => "slight_left",
            Maneuver::SlightRight => "slight_right",
            Maneuver::Uturn => "uturn",
            Maneuver::Arrive => "arrive",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [
            Maneuver::Left,
            Maneuver::Right,
            Maneuver::SlightLeft,
            Maneuver::SlightRight,
            Maneuver::Uturn,
            Maneuver::Arrive,
        ]
        .into_iter()
        .find(|m| m.as_str() == s)
    }
}

/// 経路始点から案内地点までの距離（m）。`vertex_distances_m` と同じ軸で、丸めない。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuidanceStep {
    pub maneuver: Maneuver,
    pub at_m: f64,
}

/// 選定経路のナビ用データ（behavior.md 7章）。距離は丸めない。
#[derive(Debug, Clone, PartialEq)]
pub struct NavigationData {
    pub projection_lat_deg: f64,
    pub length_m: f64,
    pub vertex_distances_m: Vec<f64>,
    pub sections: Vec<NavigationSection>,
    pub guidance: Vec<GuidanceStep>,
}

/// 評価済みの1候補。添字iの候補のIDは `route-(i+1)`。
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedRoute {
    pub path: RoutedPath,
    /// 集合B（天候で絞らない）の集計。
    pub totals: AccidentTotals,
    /// 移動手段・時間帯・曜日区分・天候のどれでも絞る前の、経路に結び付いた事故の総件数。
    /// 新方式は集合Bを作るときの `assigned` と `nearest` を合わせた事故IDの数（同じIDを
    /// 二重に数えない）。旧方式はこの値を作らないので常に `None`（応答では `null`）。
    pub total_accident_count: Option<u64>,
    /// 天候指定時の一致集計。天候allなら `None`。
    pub weather_match: Option<AccidentTotals>,
    /// 小数2桁に丸めた値。
    pub weighted_risk: f64,
    /// 小数3桁に丸めた値。
    pub risk_per_km: f64,
    /// 間引き後の事故点。件数は `min(accident_count, 250)`。
    pub accident_points: Vec<AccidentPoint>,
    /// 表示順（重み降順・同値はsid昇順）の最大100件。
    pub high_risk_sections: Vec<HighRiskSegment>,
    /// 100件で切る前の高リスクの小線分数（behavior.md 7章）。旧方式は深刻度の和が3以上、
    /// 新方式は事故の件数がしきい値以上の小線分。どちらも `severity_weight` は3以上。
    pub high_risk_total: u64,
    /// fastest・safest・middleの候補だけ `Some`。
    pub navigation: Option<NavigationData>,
    /// 小数3桁に丸めた追加時間（分）。
    pub extra_minutes: f64,
    pub within_detour: bool,
}

/// ロード済みデータの識別情報。`/api/health` の元データ。
#[derive(Debug, Clone, PartialEq)]
pub struct DatasetInfo {
    pub release_id: String,
    pub dataset_id: String,
    /// modes=0を含むロード行数。
    pub accidents: u64,
    /// source_yearの昇順。
    pub data_years: Vec<u16>,
    pub load_seconds: f64,
    /// グリッド索引を採用した場合だけ `Some`。
    pub grid_cells: Option<u64>,
}

/// routeの計算結果。1要求の中で一つのデータスナップショットから作る。
#[derive(Debug, Clone, PartialEq)]
pub struct RouteOutcome {
    /// 計算に使ったスナップショットのdataset_id。
    pub dataset_id: String,
    pub data_years: Vec<u16>,
    pub mode: Mode,
    /// 正規化した指定条件。緩和しないので入力と同じ。
    pub conditions: ConditionFilter,
    pub candidates: Vec<EvaluatedRoute>,
    /// `candidates` の添字。
    pub fastest: usize,
    /// `candidates` の添字。新方式は常に `Some`。旧方式は全候補で集合Bが空なら `None`。
    pub safest: Option<usize>,
    /// `candidates` の添字。新方式で中間案を出すときだけ `Some`。旧方式は常に `None`。
    pub middle: Option<usize>,
    pub extra_minutes: Option<f64>,
    pub risk_reduction_percent: Option<f64>,
    pub reason: String,
    pub warnings: Vec<String>,
    pub risk_note: String,
    /// 小数2桁のミリ秒。
    pub aggregation_ms: f64,
}

/// 計算エンジン。③は `Arc<dyn RouteEngine>` で保持する。
///
/// 呼出し順は `collect_candidates` → `evaluate`。`evaluate` はCPU処理なので、
/// ③が `tokio::task::spawn_blocking` とCPU同時数の上限の中で呼ぶ。
///
/// 実装は、releaseの `metadata.json` の `schema_version` で `data::load_engine` が選ぶ。
/// 2なら旧方式の `AccidentEngine`（OSRMの候補を `collect_candidates` で取得する）、
/// 3なら新方式の `RoutingEngine`（`collect_candidates` はルーターを呼ばずに空の
/// [`CandidateSet`] を返し、`evaluate` が経路探索から集計・選定までを行う）。
/// どちらの結果も、③の `api::route_response` の検査（頂点数2〜15,000、距離0超250km以下、
/// `check_detour`、`check_outcome` のその他の検査）を満たさなければならない。
pub trait RouteEngine: Send + Sync {
    /// ロード済みデータの識別情報。
    fn dataset(&self) -> &DatasetInfo;

    /// 経路候補を取得する。旧方式は初回→必要なら左→右、最大3呼出しで、並列化しない。
    /// 新方式は `router` を呼ばずに空の [`CandidateSet`] をすぐに返す。
    fn collect_candidates<'a>(
        &'a self,
        router: &'a dyn Router,
        query: &'a RouteQuery,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<CandidateSet, EngineError>>;

    /// 事故集計・選定・ナビ区間・説明を作る。新方式では、経路探索もここで行う。
    /// `cancel` を定期的に確認して中断する。
    fn evaluate(
        &self,
        query: &RouteQuery,
        candidates: CandidateSet,
        cancel: &CancelToken,
    ) -> Result<RouteOutcome, EngineError>;
}
