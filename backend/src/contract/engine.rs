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

/// 出発地と目的地の直線距離（m、両端を含む）。
pub const STRAIGHT_DISTANCE_RANGE_M: RangeInclusive<f64> = 20.0..=30_000.0;
/// 最短時間経路からの追加許容時間（分、両端を含む）。
pub const MAX_DETOUR_RANGE_MIN: RangeInclusive<f64> = 0.0..=30.0;
/// `max_detour_min` 省略時の値（分）。
pub const DEFAULT_MAX_DETOUR_MIN: f64 = 10.0;
/// 返す候補の上限。
pub const MAX_CANDIDATES: usize = 6;
/// 集合Bの経路周辺半径（m）。応答の `radius_m`。
pub const RADIUS_M: f64 = 72.0;
/// 距離重みのガウスσ（m）。応答の `distance_weight_sigma_m`。
pub const DISTANCE_WEIGHT_SIGMA_M: f64 = 24.0;
/// 候補ごとの表示事故点の上限。
pub const MAX_ACCIDENT_POINTS: usize = 250;
/// 候補ごとの表示用高リスク区間の上限。
pub const MAX_HIGH_RISK_SECTIONS: usize = 100;

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
            QueryError::StraightDistance => "出発地と目的地の直線距離は20m〜30kmにしてください。",
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
    if !STRAIGHT_DISTANCE_RANGE_M.contains(&straight_distance_m(origin, dest)) {
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
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateSet {
    /// 検証・重複排除・duration安定ソート済みの1〜6本。添字iが `route-(i+1)`。
    pub routes: Vec<RoutedPath>,
    /// 追加取得エラー（呼出し順）、1候補比較不能の警告。評価時の警告はこの後に続く。
    pub warnings: Vec<String>,
}

/// 計算の失敗。HTTP応答への変換は③が行う（README.mdの対応表）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EngineError {
    /// 初回の経路なし、または全呼出し後に候補0。502 no_route。
    NoRoute,
    /// 初回候補のdistanceまたはgeometry長が80km超。422 route_too_long。
    RouteTooLong,
    /// 初回呼出しの外部失敗、または初回候補の値・形状の不正。502 external_api_error。
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

/// 選定経路のナビ用データ（behavior.md 7章）。距離は丸めない。
#[derive(Debug, Clone, PartialEq)]
pub struct NavigationData {
    pub projection_lat_deg: f64,
    pub length_m: f64,
    pub vertex_distances_m: Vec<f64>,
    pub sections: Vec<NavigationSection>,
}

/// 評価済みの1候補。添字iの候補のIDは `route-(i+1)`。
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedRoute {
    pub path: RoutedPath,
    /// 集合B（天候で絞らない）の集計。
    pub totals: AccidentTotals,
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
    /// 100件で切る前の重み3以上の小線分数。
    pub high_risk_total: u64,
    /// fastestまたはsafestの候補だけ `Some`。
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
    /// `candidates` の添字。全候補で集合Bが空なら `None`。
    pub safest: Option<usize>,
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
pub trait RouteEngine: Send + Sync {
    /// ロード済みデータの識別情報。
    fn dataset(&self) -> &DatasetInfo;

    /// 経路候補を取得する（初回→必要なら左→右、最大3呼出し、並列化しない）。
    fn collect_candidates<'a>(
        &'a self,
        router: &'a dyn Router,
        query: &'a RouteQuery,
        cancel: &'a CancelToken,
    ) -> BoxFuture<'a, Result<CandidateSet, EngineError>>;

    /// 事故集計・選定・ナビ区間・説明を作る。`cancel` を定期的に確認して中断する。
    fn evaluate(
        &self,
        query: &RouteQuery,
        candidates: CandidateSet,
        cancel: &CancelToken,
    ) -> Result<RouteOutcome, EngineError>;
}
