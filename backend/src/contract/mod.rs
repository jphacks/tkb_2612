//! 計算エンジン（②）とAPI・外部通信・基盤（③）の内部接続契約。
//!
//! 外部のHTTP/JSON契約は `docs/api/openapi.yaml`、計算は `docs/api/behavior.md` が正本。
//! ここに置くのはRust内部の型・trait・期限の伝え方だけで、serdeのDTOは持たない。
//! 変更は両担当の承認を得てから行う（README.md）。
//!
//! 単位は名前の接尾辞で示す：`_s` 秒、`_m` メートル、`_min` 分、`_ms` ミリ秒。

pub mod calendar;
pub mod cancel;
pub mod conditions;
pub mod engine;
pub mod geo;
pub mod routing;

use std::future::Future;
use std::pin::Pin;

/// dyn互換のtraitで非同期処理を返すための型。
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub use calendar::{Calendar, DayClassification, DayTypeReason, DepartureClass, TokyoMinute};
pub use cancel::{CancelGuard, CancelToken, Interrupt};
pub use conditions::{ConditionFilter, DayType, Mode, Period, Weather};
pub use engine::{
    AccidentPoint, AccidentTotals, CandidateSet, DatasetInfo, EngineError, EvaluatedRoute,
    HighRiskSegment, NavigationData, NavigationSection, QueryError, RouteEngine, RouteOutcome,
    RouteQuery,
};
pub use geo::{LatLon, LonLat};
pub use routing::{
    MalformedRoute, ProviderRoute, RoutedPath, Router, RoutingError, RoutingRequest,
};
