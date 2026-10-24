//! 経路候補の取得・検証・重複排除・並べ替え（behavior.md 5章）。

use crate::contract::engine::MAX_CANDIDATES;
use crate::contract::geo::{METERS_PER_DEGREE, straight_distance_m};
use crate::contract::routing::MAX_ROUTES_PER_CALL;
use crate::contract::{
    CancelToken, CandidateSet, EngineError, LatLon, ProviderRoute, RouteQuery, RoutedPath, Router,
    RoutingRequest,
};

use super::rounding::py_round;

/// 経路の頂点数（両端を含む）。
pub const VERTEX_COUNT_RANGE: std::ops::RangeInclusive<usize> = 2..=15_000;
/// distanceまたはgeometry長がこれを超えるとroute_too_long（ちょうどは可）。
pub const MAX_ROUTE_LENGTH_M: f64 = 80_000.0;
/// 初回の重複排除後がこれ未満なら左右の経由点で2回追加する。
pub const MIN_INITIAL_CANDIDATES: usize = 3;
/// fingerprintの桁数。
pub const FINGERPRINT_DIGITS: usize = 5;

/// 1候補比較不能の警告（openapi.yamlの応答例と同じ文面）。
pub const SINGLE_CANDIDATE_WARNING: &str = "経路候補が1本のみのため、経路間の比較はできません。";
/// 追加候補の取得に失敗したときの警告。1回の追加呼出しの失敗ごとに1件。
///
/// 文面は仕様に定めがない（#8 未決事項4）ため②が定め、利用者の承認を得た。
/// 上流のURLや位置・例外の文字列を含めない。
pub const ADDITIONAL_ROUTE_WARNING: &str =
    "追加の経路候補を取得できなかったため、取得できた候補だけで比較しています。";

/// 経路の頂点を小数5桁に丸めた順序付きの列。同じなら後から来た経路を捨てる。
pub type Fingerprint = Vec<(u64, u64)>;

pub fn fingerprint(path: &RoutedPath) -> Fingerprint {
    path.coordinates
        .iter()
        .map(|p| {
            (
                py_round(p.lon, FINGERPRINT_DIGITS).to_bits(),
                py_round(p.lat, FINGERPRINT_DIGITS).to_bits(),
            )
        })
        .collect()
}

/// 頂点間の直線距離（behavior.md 4章の式）の和。
pub fn geometry_length_m(path: &RoutedPath) -> f64 {
    path.coordinates
        .windows(2)
        .map(|w| straight_distance_m(LatLon::from(w[0]), LatLon::from(w[1])))
        .sum()
}

/// 1経路の検証の失敗。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteDefect {
    /// 型として解釈できない、または値・形状が不正。
    Invalid,
    /// distanceまたはgeometry長が80km超。
    TooLong,
}

/// 1経路の値と形状を検証する。
pub fn validate_route(path: &RoutedPath) -> Result<(), RouteDefect> {
    let positive = |x: f64| x.is_finite() && x > 0.0;
    if !positive(path.duration_s)
        || !positive(path.distance_m)
        || !VERTEX_COUNT_RANGE.contains(&path.coordinates.len())
        || !path.coordinates.iter().all(|p| p.is_in_japan())
    {
        return Err(RouteDefect::Invalid);
    }
    if path.distance_m > MAX_ROUTE_LENGTH_M || geometry_length_m(path) > MAX_ROUTE_LENGTH_M {
        return Err(RouteDefect::TooLong);
    }
    Ok(())
}

/// 左右の経由点。`side` は-1（左）または+1（右）。
pub fn side_via(origin: LatLon, dest: LatLon, side: f64) -> LatLon {
    let mid_lat = (origin.lat + dest.lat) / 2.0;
    let mid_lon = (origin.lon + dest.lon) / 2.0;
    let k = METERS_PER_DEGREE;
    let kx = k * (mid_lat * std::f64::consts::PI / 180.0).cos();
    let dx = (dest.lon - origin.lon) * kx;
    let dy = (dest.lat - origin.lat) * k;
    let l = dx.hypot(dy);
    let offset = (0.15 * l).clamp(200.0, 1200.0);
    LatLon::new(
        mid_lat + side * dx / l * offset / k,
        mid_lon - side * dy / l * offset / kx,
    )
}

/// 受け付けた候補（呼出し順）。
#[derive(Default)]
struct Accepted {
    routes: Vec<RoutedPath>,
    fingerprints: Vec<Fingerprint>,
}

impl Accepted {
    fn push(&mut self, path: RoutedPath) {
        let fp = fingerprint(&path);
        if !self.fingerprints.contains(&fp) {
            self.fingerprints.push(fp);
            self.routes.push(path);
        }
    }
}

/// 初回の呼出しの結果を受け付ける。どの経路の不正も要求全体の失敗にする。
fn accept_initial(
    response: Vec<ProviderRoute>,
    accepted: &mut Accepted,
    cancel: &CancelToken,
) -> Result<(), EngineError> {
    if response.is_empty() {
        return Err(EngineError::NoRoute);
    }
    for route in response.into_iter().take(MAX_ROUTES_PER_CALL) {
        cancel.check()?;
        let path = route.map_err(|_| EngineError::ExternalApi)?;
        match validate_route(&path) {
            Ok(()) => accepted.push(path),
            Err(RouteDefect::Invalid) => return Err(EngineError::ExternalApi),
            Err(RouteDefect::TooLong) => return Err(EngineError::RouteTooLong),
        }
    }
    Ok(())
}

/// 追加の呼出しの結果を受け付ける。不正な経路でそこまでの正常な経路を残して止め、
/// 警告にするかを返す。
fn accept_additional(
    response: Vec<ProviderRoute>,
    accepted: &mut Accepted,
    cancel: &CancelToken,
) -> Result<bool, EngineError> {
    if response.is_empty() {
        return Ok(true);
    }
    for route in response.into_iter().take(MAX_ROUTES_PER_CALL) {
        cancel.check()?;
        let Ok(path) = route else {
            return Ok(true);
        };
        if validate_route(&path).is_err() {
            return Ok(true);
        }
        accepted.push(path);
    }
    Ok(false)
}

/// 初回→必要なら左→右の順で経路を取得し、検証・重複排除・duration安定ソートした1〜6本を返す。
/// 並列化しない。上流への送信前の `cancel.check()` はRouter側が行う。
pub async fn collect_candidates(
    router: &dyn Router,
    query: &RouteQuery,
    cancel: &CancelToken,
) -> Result<CandidateSet, EngineError> {
    let (origin, dest, mode) = (query.origin(), query.dest(), query.mode());
    cancel.check()?;
    let initial = RoutingRequest {
        origin,
        dest,
        mode,
        via: None,
    };
    let mut accepted = Accepted::default();
    let response = router.route(&initial, cancel).await?;
    accept_initial(response, &mut accepted, cancel)?;

    let mut warnings = Vec::new();
    if accepted.routes.len() < MIN_INITIAL_CANDIDATES {
        // 1回目で3本以上になっても2回目を省略しない
        for side in [-1.0, 1.0] {
            cancel.check()?;
            let request = RoutingRequest {
                origin,
                dest,
                mode,
                via: Some(side_via(origin, dest, side)),
            };
            let failed = match router.route(&request, cancel).await {
                Ok(response) => accept_additional(response, &mut accepted, cancel)?,
                // NoRoute・ExternalApiは警告。Busy・Interruptedは部分成功にしない
                Err(e) if !e.aborts_request() => true,
                Err(e) => return Err(e.into()),
            };
            if failed {
                warnings.push(ADDITIONAL_ROUTE_WARNING.to_owned());
            }
        }
    }
    cancel.check()?;

    let mut routes = accepted.routes;
    if routes.is_empty() {
        return Err(EngineError::NoRoute);
    }
    // 同durationは挿入順のまま（distanceでは並べ替えない）
    routes.sort_by(|a, b| a.duration_s.total_cmp(&b.duration_s));
    routes.truncate(MAX_CANDIDATES);
    if routes.len() == 1 {
        warnings.push(SINGLE_CANDIDATE_WARNING.to_owned());
    }
    Ok(CandidateSet { routes, warnings })
}
