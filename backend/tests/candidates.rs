//! 候補の取得・検証・重複排除・並べ替え（behavior.md 5章、11章 ExternalAndValidationTests、
//! 追加試験2・3、#8 ステップ9）。

mod common;

use accident_route_backend::contract::geo::METERS_PER_DEGREE;
use accident_route_backend::contract::{
    CancelToken, ConditionFilter, EngineError, Interrupt, MalformedRoute, Mode, RoutedPath,
    RoutingError,
};
use accident_route_backend::domain::candidates::{
    ADDITIONAL_ROUTE_WARNING, SINGLE_CANDIDATE_WARNING, collect_candidates, side_via,
};
use accident_route_backend::testing::{RouterResponse, ScriptedRouter, valid_routes};

use common::{DEST, ORIGIN, path, query};

fn route(duration_s: f64, lat_offset: f64) -> RoutedPath {
    path(
        duration_s,
        1000.0,
        &[(139.0, 35.0), (139.005, 35.0 + lat_offset), (139.01, 35.0)],
    )
}

async fn collect(
    mode: Mode,
    responses: Vec<RouterResponse>,
) -> (
    Result<accident_route_backend::contract::CandidateSet, EngineError>,
    ScriptedRouter,
) {
    let router = ScriptedRouter::new(responses);
    let result = collect_candidates(
        &router,
        &query(mode, 10.0, ConditionFilter::default()),
        &CancelToken::unbounded(),
    )
    .await;
    (result, router)
}

#[tokio::test]
async fn test_left_right_vias_and_deduplication() {
    // 初回1本→左右で各1本追加して3候補
    let (result, router) = collect(
        Mode::Bike,
        vec![
            valid_routes(&[route(600.0, 0.0)]),
            valid_routes(&[route(700.0, 0.001)]),
            valid_routes(&[route(650.0, -0.001)]),
        ],
    )
    .await;
    let set = result.unwrap();
    assert_eq!(set.routes.len(), 3);
    assert!(set.warnings.is_empty());
    // duration昇順
    assert_eq!(
        set.routes.iter().map(|r| r.duration_s).collect::<Vec<_>>(),
        [600.0, 650.0, 700.0]
    );
    let calls = router.calls();
    assert_eq!(calls.len(), 3);
    assert!(calls.iter().all(|c| c.mode == Mode::Bike));
    assert_eq!(calls[0].via, None);
    // side=-1→+1の順。式どおり、東向きの経路ではside=-1が南、+1が北
    let left = calls[1].via.unwrap();
    let right = calls[2].via.unwrap();
    assert!(left.lat < 35.0 && right.lat > 35.0, "{left:?} {right:?}");
    assert_eq!(left.lon, 139.005);
    assert_eq!(left, side_via(ORIGIN, DEST, -1.0));
    assert_eq!(right, side_via(ORIGIN, DEST, 1.0));
    // offset=min(1200,max(200,0.15L))。L≈911mなので200m
    let offset_m = (35.0 - left.lat) * METERS_PER_DEGREE;
    assert!((offset_m - 200.0).abs() < 1e-6, "{offset_m}");

    // 重複だけなら1候補＋比較不能の警告（追加呼出しは2回とも行う）
    let (result, router) = collect(
        Mode::Bike,
        vec![
            valid_routes(&[route(600.0, 0.0)]),
            valid_routes(&[route(600.0, 0.0)]),
            valid_routes(&[route(610.0, 0.000_001)]), // 5桁に丸めると同じ
        ],
    )
    .await;
    let set = result.unwrap();
    assert_eq!(set.routes.len(), 1);
    assert_eq!(set.routes[0].duration_s, 600.0, "先の経路を残す");
    assert_eq!(set.warnings, [SINGLE_CANDIDATE_WARNING]);
    assert_eq!(router.calls().len(), 3);
}

#[tokio::test]
async fn second_additional_call_is_not_skipped() {
    // 1回目で3本以上になっても2回目を呼ぶ
    let (result, router) = collect(
        Mode::Walk,
        vec![
            valid_routes(&[route(600.0, 0.0)]),
            valid_routes(&[route(700.0, 0.001), route(800.0, 0.002)]),
            valid_routes(&[route(900.0, 0.003)]),
        ],
    )
    .await;
    assert_eq!(result.unwrap().routes.len(), 4);
    assert_eq!(router.calls().len(), 3);

    // 初回が3本以上なら追加しない
    let (result, router) = collect(
        Mode::Walk,
        vec![valid_routes(&[
            route(600.0, 0.0),
            route(610.0, 0.001),
            route(620.0, 0.002),
        ])],
    )
    .await;
    assert_eq!(result.unwrap().routes.len(), 3);
    assert_eq!(router.calls().len(), 1);
}

#[tokio::test]
async fn test_additional_api_failure_keeps_original() {
    for error in [RoutingError::ExternalApi, RoutingError::NoRoute] {
        let (result, _) = collect(
            Mode::Walk,
            vec![
                valid_routes(&[route(600.0, 0.0)]),
                Err(error),
                valid_routes(&[route(700.0, 0.001)]),
            ],
        )
        .await;
        let set = result.unwrap();
        assert_eq!(set.routes.len(), 2);
        assert_eq!(set.warnings, [ADDITIONAL_ROUTE_WARNING]);
    }
    // 2回とも失敗：呼出し順に2件の警告、その後に比較不能
    let (result, _) = collect(
        Mode::Walk,
        vec![
            valid_routes(&[route(600.0, 0.0)]),
            Err(RoutingError::ExternalApi),
            Err(RoutingError::NoRoute),
        ],
    )
    .await;
    assert_eq!(
        result.unwrap().warnings,
        [
            ADDITIONAL_ROUTE_WARNING,
            ADDITIONAL_ROUTE_WARNING,
            SINGLE_CANDIDATE_WARNING
        ]
    );
    // 初回の失敗は要求全体の失敗
    for (error, expected) in [
        (RoutingError::ExternalApi, EngineError::ExternalApi),
        (RoutingError::NoRoute, EngineError::NoRoute),
        (RoutingError::Busy, EngineError::Busy),
    ] {
        let (result, router) = collect(Mode::Walk, vec![Err(error)]).await;
        assert_eq!(result.unwrap_err(), expected);
        assert_eq!(router.calls().len(), 1);
    }
    // 追加呼出しのBusy・Interruptedは部分成功にしない
    for (error, expected) in [
        (RoutingError::Busy, EngineError::Busy),
        (
            RoutingError::Interrupted(Interrupt::DeadlineExceeded),
            EngineError::Interrupted(Interrupt::DeadlineExceeded),
        ),
    ] {
        let (result, _) = collect(
            Mode::Walk,
            vec![valid_routes(&[route(600.0, 0.0)]), Err(error)],
        )
        .await;
        assert_eq!(result.unwrap_err(), expected);
    }
}

#[tokio::test]
async fn malformed_route_stops_the_batch() {
    // 初回のMalformedRouteは要求全体のExternalApi
    let (result, _) = collect(
        Mode::Walk,
        vec![Ok(vec![Ok(route(600.0, 0.0)), Err(MalformedRoute)])],
    )
    .await;
    assert_eq!(result.unwrap_err(), EngineError::ExternalApi);
    // 追加では、それより前の正常な経路を残して警告
    let (result, _) = collect(
        Mode::Walk,
        vec![
            valid_routes(&[route(600.0, 0.0)]),
            Ok(vec![
                Ok(route(700.0, 0.001)),
                Err(MalformedRoute),
                Ok(route(800.0, 0.002)),
            ]),
            Ok(vec![Ok(route(750.0, -0.001)), Ok(route(f64::NAN, 0.003))]),
        ],
    )
    .await;
    let set = result.unwrap();
    assert_eq!(
        set.routes.iter().map(|r| r.duration_s).collect::<Vec<_>>(),
        [600.0, 700.0, 750.0]
    );
    assert_eq!(
        set.warnings,
        [ADDITIONAL_ROUTE_WARNING, ADDITIONAL_ROUTE_WARNING]
    );
}

#[tokio::test]
async fn test_bad_provider_geometry() {
    let bad_routes = [
        path(600.0, 1000.0, &[(139.0, 35.0), (f64::NAN, 35.0)]),
        path(600.0, 1000.0, &[(139.0, 35.0)]),
        path(600.0, 1000.0, &[(139.0, 35.0), (160.0, 35.0)]),
        path(0.0, 1000.0, &[(139.0, 35.0), (139.01, 35.0)]),
        path(600.0, -1.0, &[(139.0, 35.0), (139.01, 35.0)]),
        path(f64::INFINITY, 1000.0, &[(139.0, 35.0), (139.01, 35.0)]),
    ];
    for bad in bad_routes {
        let (result, _) = collect(Mode::Walk, vec![valid_routes(std::slice::from_ref(&bad))]).await;
        assert_eq!(result.unwrap_err(), EngineError::ExternalApi, "{bad:?}");
    }
}

/// 1本だけの初回応答。追加の2回は経路なしにする。
fn single(route: RoutedPath) -> Vec<RouterResponse> {
    vec![
        valid_routes(&[route]),
        Err(RoutingError::NoRoute),
        Err(RoutingError::NoRoute),
    ]
}

#[tokio::test]
async fn vertex_count_and_80km_limits() {
    let many = |n: usize| {
        let coordinates: Vec<(f64, f64)> = (0..n)
            .map(|i| (139.0 + 0.01 * i as f64 / n as f64, 35.0))
            .collect();
        path(600.0, 1000.0, &coordinates)
    };
    let (result, _) = collect(Mode::Car, single(many(15_000))).await;
    assert!(result.is_ok());
    let (result, _) = collect(Mode::Car, single(many(15_001))).await;
    assert_eq!(result.unwrap_err(), EngineError::ExternalApi);

    // distance：80,000ちょうどは可、超過はroute_too_long
    let with_distance = |d: f64| path(600.0, d, &[(139.0, 35.0), (139.01, 35.0)]);
    let (result, _) = collect(Mode::Car, single(with_distance(80_000.0))).await;
    assert!(result.is_ok());
    let (result, _) = collect(Mode::Car, single(with_distance(80_000.001))).await;
    assert_eq!(result.unwrap_err(), EngineError::RouteTooLong);

    // geometry長（頂点間の直線距離の和）が80km超
    let long = path(
        600.0,
        1000.0,
        &[(139.0, 35.0), (139.0, 35.0 + 80_001.0 / METERS_PER_DEGREE)],
    );
    let (result, _) = collect(Mode::Car, single(long.clone())).await;
    assert_eq!(result.unwrap_err(), EngineError::RouteTooLong);
    let exact = path(
        600.0,
        1000.0,
        &[(139.0, 35.0), (139.0, 35.0 + 79_999.0 / METERS_PER_DEGREE)],
    );
    let (result, _) = collect(Mode::Car, single(exact)).await;
    assert!(result.is_ok());

    // 追加呼出しの80km超は警告にして既存候補を残す
    let (result, _) = collect(
        Mode::Car,
        vec![
            valid_routes(&[route(600.0, 0.0)]),
            valid_routes(&[long]),
            Err(RoutingError::NoRoute),
        ],
    )
    .await;
    let set = result.unwrap();
    assert_eq!(set.routes.len(), 1);
    assert_eq!(
        set.warnings,
        [
            ADDITIONAL_ROUTE_WARNING,
            ADDITIONAL_ROUTE_WARNING,
            SINGLE_CANDIDATE_WARNING
        ]
    );
}

#[tokio::test]
async fn fingerprint_rounding_boundary_and_orientation() {
    // 小数5桁に丸めて同じなら重複、境界をまたげば別候補
    let a = path(
        600.0,
        1000.0,
        &[(139.0, 35.0), (139.005, 35.000_004), (139.01, 35.0)],
    );
    let same = path(
        610.0,
        1000.0,
        &[(139.0, 35.0), (139.005, 35.000_001), (139.01, 35.0)],
    );
    let different = path(
        620.0,
        1000.0,
        &[(139.0, 35.0), (139.005, 35.000_006), (139.01, 35.0)],
    );
    let reversed = path(
        630.0,
        1000.0,
        &[(139.01, 35.0), (139.005, 35.000_004), (139.0, 35.0)],
    );
    let split = path(
        640.0,
        1000.0,
        &[
            (139.0, 35.0),
            (139.0, 35.0),
            (139.005, 35.000_004),
            (139.01, 35.0),
        ],
    );
    let (result, _) = collect(
        Mode::Walk,
        vec![valid_routes(&[a, same, different, reversed, split])],
    )
    .await;
    assert_eq!(
        result
            .unwrap()
            .routes
            .iter()
            .map(|r| r.duration_s)
            .collect::<Vec<_>>(),
        [600.0, 620.0, 630.0, 640.0]
    );
}

#[tokio::test]
async fn equal_durations_keep_insertion_order_and_top_six() {
    let routes: Vec<RoutedPath> = (0..6)
        .map(|i| {
            let mut r = route(if i == 0 { 900.0 } else { 600.0 }, 0.001 * i as f64);
            r.distance_m = 2000.0 - i as f64; // distanceでは並べ替えない
            r
        })
        .collect();
    let extra: Vec<RoutedPath> = (0..6)
        .map(|i| route(100.0 + i as f64, 0.01 + 0.001 * i as f64))
        .collect();
    // 初回だけで6本（追加なし）
    let (result, _) = collect(Mode::Walk, vec![valid_routes(&routes)]).await;
    let set = result.unwrap();
    assert_eq!(
        set.routes.iter().map(|r| r.distance_m).collect::<Vec<_>>(),
        [1999.0, 1998.0, 1997.0, 1996.0, 1995.0, 2000.0]
    );
    // 呼出しごとに先頭6本だけを検査する（7本目以降の不正は見ない）
    let mut seven = routes.clone();
    seven.push(path(f64::NAN, 1.0, &[(139.0, 35.0)]));
    let (result, _) = collect(Mode::Walk, vec![valid_routes(&seven)]).await;
    assert_eq!(result.unwrap().routes.len(), 6);
    // 最大18本を検証して6本に削る
    let (result, _) = collect(
        Mode::Walk,
        vec![
            valid_routes(&routes[..1]),
            valid_routes(&extra[..3]),
            valid_routes(&extra[3..]),
        ],
    )
    .await;
    let set = result.unwrap();
    assert_eq!(set.routes.len(), 6);
    assert_eq!(set.routes[0].duration_s, 100.0);
    assert!(set.routes.iter().all(|r| r.duration_s < 900.0));
}

#[tokio::test]
async fn cancelled_request_stops_before_calling() {
    let router = ScriptedRouter::new(vec![]);
    let cancel = CancelToken::unbounded();
    cancel.cancel();
    let result = collect_candidates(
        &router,
        &query(Mode::Walk, 10.0, ConditionFilter::default()),
        &cancel,
    )
    .await;
    assert_eq!(
        result.unwrap_err(),
        EngineError::Interrupted(Interrupt::Cancelled)
    );
    assert!(router.calls().is_empty());
}
