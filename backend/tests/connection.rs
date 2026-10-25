//! スタブと共通fixtureによる接続・中断の検証。
//!
//! `run_route` は③のrouteハンドラが取る呼出し順の見本で、②の実装に差し替えても同じ形で動く。

use std::sync::Arc;
use std::time::{Duration, Instant};

use accident_route_backend::contract::{
    CancelToken, DatasetInfo, EngineError, Interrupt, MalformedRoute, Mode, RouteEngine,
    RouteOutcome, RouteQuery, Router, RoutingError, RoutingRequest,
};
use accident_route_backend::testing::engine::LATE_RETURN;
use accident_route_backend::testing::fixtures::{
    check_outcome_invariants, expected_route_outcome, fixture_dataset_info, route_example,
    route_example_accidents, route_examples, route_response_names,
};
use accident_route_backend::testing::{
    ScriptedRouter, StallingRouter, StubEngine, StubEvaluation, valid_routes,
};

/// ③のrouteハンドラの呼出し順。
///
/// 1. 候補取得（非同期）。上流待ちは要求全体の期限で打ち切る。
/// 2. 集計（CPU）。`spawn_blocking` で動かし、futureが破棄されたらガードで取り消す。
async fn run_route(
    engine: Arc<dyn RouteEngine>,
    router: Arc<dyn Router>,
    query: RouteQuery,
    cancel: CancelToken,
) -> Result<RouteOutcome, EngineError> {
    let deadline = cancel.deadline().expect("route要求は期限付き");
    let guard = cancel.cancel_on_drop();
    let work = async {
        let candidates = engine
            .collect_candidates(router.as_ref(), &query, &cancel)
            .await?;
        let (engine, cancel) = (engine.clone(), cancel.clone());
        tokio::task::spawn_blocking(move || engine.evaluate(&query, candidates, &cancel))
            .await
            .map_err(|e| EngineError::ContractViolation(format!("集計タスクの失敗: {e}")))?
    };
    let result = match tokio::time::timeout_at(deadline.into(), work).await {
        Ok(result) => result,
        Err(_) => Err(EngineError::Interrupted(Interrupt::DeadlineExceeded)),
    };
    // 完了と期限切れが同時に観測されたら期限切れを優先する。
    // 実際のハンドラではJSONの準備・圧縮の後に呼ぶ。
    let result = cancel.settle(result);
    // 期限切れ等で集計が残っていればガードの破棄で止める。正常終了だけ解除する。
    if result.is_ok() {
        guard.disarm();
    }
    result
}

fn stub(evaluation: StubEvaluation) -> Arc<StubEngine> {
    Arc::new(StubEngine::new(fixture_dataset_info(), evaluation))
}

fn fixed(name: &str) -> StubEvaluation {
    StubEvaluation::Fixed(Box::new(expected_route_outcome(name)))
}

// ---- 共通fixture -------------------------------------------------------------

#[test]
fn inputs_and_expected_outcomes_cover_the_same_examples() {
    let mut inputs: Vec<_> = route_examples().into_iter().map(|e| e.name).collect();
    let mut outputs = route_response_names();
    inputs.sort();
    outputs.sort();
    assert_eq!(inputs, outputs);
    // 既存3例と、天候all・天候一致0件・事故点の間引きの例が含まれる（例の追加は妨げない）
    for name in [
        "noAccidents",
        "noWeatherMatch",
        "singleRoute",
        "thinnedPoints",
        "twoSelected",
        "weatherAll",
    ] {
        assert!(inputs.iter().any(|n| n == name), "{name}がありません");
    }
}

#[test]
fn expected_outcomes_satisfy_correlation_constraints() {
    for name in route_response_names() {
        let outcome = expected_route_outcome(&name);
        check_outcome_invariants(&outcome).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
fn expected_outcomes_are_consistent_with_inputs() {
    let accidents = route_example_accidents();
    for example in route_examples() {
        let outcome = expected_route_outcome(&example.name);
        // 固定経路はduration昇順で、そのまま候補順になる
        let paths: Vec<_> = outcome.candidates.iter().map(|c| c.path.clone()).collect();
        assert_eq!(paths, example.routes, "{}", example.name);
        assert_eq!(outcome.mode, example.query.mode());
        assert_eq!(outcome.conditions, example.query.conditions());
        // 表示事故点は入力事故そのもの（ID・座標・属性）
        for point in outcome.candidates.iter().flat_map(|c| &c.accident_points) {
            assert!(example.accident_ids.contains(&point.id));
            let source = accidents.iter().find(|a| a.point.id == point.id).unwrap();
            assert_eq!(&source.point, point);
            assert_ne!(source.modes & example.query.mode().mask(), 0);
        }
    }
}

#[test]
fn two_selected_expected_values() {
    let outcome = expected_route_outcome("twoSelected");
    let risks: Vec<_> = outcome.candidates.iter().map(|c| c.weighted_risk).collect();
    assert_eq!(risks, [12.21, 3.64, 8.34]);
    assert_eq!((outcome.fastest, outcome.safest), (0, Some(1)));
    assert_eq!(outcome.risk_reduction_percent, Some(70.2));
    let nav = outcome.candidates[0].navigation.as_ref().unwrap();
    let sids: Vec<_> = nav
        .sections
        .iter()
        .map(|s| {
            (
                s.start_segment_index,
                s.end_segment_index,
                s.severity_weight,
            )
        })
        .collect();
    assert_eq!(sids, [(3, 3, 3), (6, 6, 12)]);
    assert!(outcome.candidates[2].navigation.is_none());
}

#[test]
fn no_accidents_has_no_safest_route() {
    let outcome = expected_route_outcome("noAccidents");
    assert_eq!(outcome.safest, None);
    assert_eq!(outcome.extra_minutes, None);
    assert_eq!(outcome.candidates[0].totals.accident_count, 0);
    assert!(outcome.candidates[0].navigation.is_some());
}

#[test]
fn invariant_check_rejects_broken_outcomes() {
    let rejects = |name: &str, broken: &dyn Fn(&mut RouteOutcome), message: &str| {
        let mut outcome = expected_route_outcome(name);
        broken(&mut outcome);
        let error = check_outcome_invariants(&outcome).expect_err(message);
        assert!(error.contains(message), "{name}: {error}");
    };
    // 全候補0件なのにsafestがある（選定候補のnavigationは揃えておく）
    rejects(
        "noAccidents",
        &|o| {
            o.safest = Some(0);
            o.extra_minutes = Some(0.0);
        },
        "全候補で集合Bが空なのにsafestがある",
    );
    rejects(
        "twoSelected",
        &|o| {
            o.safest = None;
            o.extra_minutes = None;
            o.risk_reduction_percent = None;
        },
        "集合Bが空でない候補があるのにsafestがnull",
    );
    rejects(
        "twoSelected",
        &|o| o.extra_minutes = None,
        "extra_minutesの有無",
    );
    rejects("twoSelected", &|o| o.safest = Some(9), "safestが候補にない");
    rejects(
        "twoSelected",
        &|o| o.candidates[2].navigation = o.candidates[0].navigation.clone(),
        "未選定なのにnavigationがある",
    );
    rejects(
        "twoSelected",
        &|o| o.candidates[1].navigation = None,
        "選定候補なのにnavigationがない",
    );
    rejects(
        "singleRoute",
        &|o| o.candidates[0].accident_points.clear(),
        "事故点数",
    );
}

// ---- ②：OSRMなしで候補生成を検証する --------------------------------------------

#[tokio::test]
async fn scripted_router_returns_fixture_routes_and_records_requests() {
    let example = route_example("twoSelected");
    let (a, b) = (example.routes[0].clone(), example.routes[1].clone());
    // 追加の呼出しで、正常な経路の後に形の不正な経路が来る場合
    let partial = vec![Ok(b.clone()), Err(MalformedRoute)];
    let router = ScriptedRouter::new([
        valid_routes(&example.routes),
        Ok(partial.clone()),
        Err(RoutingError::ExternalApi),
    ]);
    let cancel = CancelToken::unbounded();
    let initial = RoutingRequest {
        origin: example.query.origin(),
        dest: example.query.dest(),
        mode: Mode::Walk,
        via: None,
    };
    let initial_routes = router.route(&initial, &cancel).await.unwrap();
    assert_eq!(initial_routes[..2], [Ok(a), Ok(b.clone())]);
    let left = RoutingRequest {
        via: Some(example.query.origin()),
        ..initial
    };
    let right = RoutingRequest {
        via: Some(example.query.dest()),
        ..initial
    };
    // 不正な経路の前の正常な経路は②が受け取れる
    let left_routes = router.route(&left, &cancel).await.unwrap();
    assert_eq!(left_routes, partial);
    let valid_prefix: Vec<_> = left_routes.iter().map_while(|r| r.clone().ok()).collect();
    assert_eq!(valid_prefix, [b]);
    assert_eq!(
        router.route(&right, &cancel).await,
        Err(RoutingError::ExternalApi)
    );
    assert_eq!(router.calls(), [initial, left, right]);
    assert_eq!(router.remaining(), 0);

    // 送信前に中断した呼出しは上流へ届かない
    cancel.cancel();
    assert_eq!(
        router.route(&initial, &cancel).await,
        Err(RoutingError::Interrupted(Interrupt::Cancelled))
    );
    assert_eq!(router.calls().len(), 3);
}

#[tokio::test]
#[should_panic(expected = "用意した応答を超えて")]
async fn scripted_router_rejects_unexpected_calls() {
    let example = route_example("singleRoute");
    let router = ScriptedRouter::new([]);
    let request = RoutingRequest {
        origin: example.query.origin(),
        dest: example.query.dest(),
        mode: Mode::Walk,
        via: None,
    };
    let _ = router.route(&request, &CancelToken::unbounded()).await;
}

// ---- ③：計算本体なしでハンドラの流れを検証する ------------------------------------

#[tokio::test]
async fn stub_engine_returns_fixture_outcome_through_handler_flow() {
    for name in route_response_names() {
        let example = route_example(&name);
        let engine = stub(fixed(&name));
        let router = Arc::new(ScriptedRouter::new([valid_routes(&example.routes)]));
        let cancel = CancelToken::with_timeout(Duration::from_secs(35));
        let outcome = run_route(
            engine.clone(),
            router.clone(),
            example.query,
            cancel.clone(),
        )
        .await
        .unwrap();
        assert_eq!(outcome, expected_route_outcome(&name));
        assert_eq!(router.calls().len(), 1);
        assert!(!cancel.is_cancelled(), "正常終了ではガードを解除する");
        assert!(engine.evaluate_finished());
    }
}

#[tokio::test]
async fn health_reads_dataset_info_from_engine() {
    let engine: Arc<dyn RouteEngine> = stub(fixed("singleRoute"));
    let info: &DatasetInfo = engine.dataset();
    assert_eq!(info.dataset_id.len(), 64);
    assert_eq!(info.release_id.len(), 64);
    assert_eq!(info.data_years, [2025]);
    assert_eq!(info.accidents, 3);
}

#[tokio::test]
async fn upstream_errors_surface_as_engine_errors() {
    let example = route_example("singleRoute");
    for (upstream, expected) in [
        (RoutingError::NoRoute, EngineError::NoRoute),
        (RoutingError::ExternalApi, EngineError::ExternalApi),
        (RoutingError::Busy, EngineError::Busy),
    ] {
        let engine = stub(fixed("singleRoute"));
        let router = Arc::new(ScriptedRouter::new([Err(upstream)]));
        let cancel = CancelToken::with_timeout(Duration::from_secs(35));
        let result = run_route(engine.clone(), router, example.query.clone(), cancel).await;
        assert_eq!(result, Err(expected));
        assert!(!engine.evaluate_started());
    }
}

#[tokio::test]
async fn malformed_route_in_initial_call_is_external_api_error() {
    let example = route_example("twoSelected");
    // 正常な経路が先にあっても、初回の呼出しでは要求全体の失敗にする
    let response = Ok(vec![Ok(example.routes[0].clone()), Err(MalformedRoute)]);
    let engine = stub(fixed("twoSelected"));
    let router = Arc::new(ScriptedRouter::new([response]));
    let cancel = CancelToken::with_timeout(Duration::from_secs(35));
    let result = run_route(engine.clone(), router, example.query, cancel).await;
    assert_eq!(result, Err(EngineError::ExternalApi));
    assert!(!engine.evaluate_started());
}

// ---- 中断 ------------------------------------------------------------------

const SHORT: Duration = Duration::from_millis(50);
/// 期限後に止まるまでの許容時間（CI等の遅い環境を考慮）。
const STOP_MARGIN: Duration = Duration::from_secs(2);

#[tokio::test]
async fn deadline_while_waiting_upstream_is_deadline_exceeded() {
    let example = route_example("singleRoute");
    let engine = stub(fixed("singleRoute"));
    let router = Arc::new(StallingRouter::new());
    let started = Instant::now();
    let cancel = CancelToken::with_timeout(SHORT);
    let result = run_route(engine.clone(), router.clone(), example.query, cancel).await;
    assert_eq!(
        result,
        Err(EngineError::Interrupted(Interrupt::DeadlineExceeded))
    );
    assert!(started.elapsed() < SHORT + STOP_MARGIN);
    assert_eq!(router.call_count(), 1);
    assert!(!engine.evaluate_started());
}

#[tokio::test]
async fn deadline_during_cpu_aggregation_stops_the_blocking_task() {
    let example = route_example("singleRoute");
    let engine = stub(StubEvaluation::SpinUntilInterrupted);
    let router = Arc::new(ScriptedRouter::new([valid_routes(&example.routes)]));
    let started = Instant::now();
    let cancel = CancelToken::with_timeout(SHORT);
    let result = run_route(engine.clone(), router, example.query, cancel).await;
    assert_eq!(
        result,
        Err(EngineError::Interrupted(Interrupt::DeadlineExceeded))
    );
    wait_until(|| engine.evaluate_finished()).await;
    assert!(started.elapsed() < SHORT + STOP_MARGIN);
}

/// 集計の完了と期限切れを、ハンドラが同時に観測する場合。
/// 期限後に完了した結果を200にせず、期限切れを優先する（behavior.md 5章）。
#[tokio::test(flavor = "current_thread")]
async fn completion_observed_after_deadline_is_deadline_exceeded() {
    let example = route_example("singleRoute");
    let engine = stub(StubEvaluation::ReturnAfterInterrupt(Box::new(
        expected_route_outcome("singleRoute"),
    )));
    let router = Arc::new(ScriptedRouter::new([valid_routes(&example.routes)]));
    let cancel = CancelToken::with_timeout(SHORT);
    let deadline = cancel.deadline().unwrap();
    // 期限の直前から集計の完了後までランタイムのスレッドを止め、
    // 再開時にタイマーと集計結果の両方が準備済みになるようにする。
    let blocker = tokio::spawn(async move {
        tokio::time::sleep_until((deadline - Duration::from_millis(10)).into()).await;
        std::thread::sleep(Duration::from_millis(10) + LATE_RETURN * 3);
    });
    let result = run_route(engine.clone(), router, example.query, cancel.clone()).await;
    blocker.await.unwrap();
    assert!(engine.evaluate_finished());
    assert_eq!(
        result,
        Err(EngineError::Interrupted(Interrupt::DeadlineExceeded))
    );
}

#[tokio::test]
async fn dropping_the_request_cancels_the_blocking_task() {
    let example = route_example("singleRoute");
    let engine = stub(StubEvaluation::SpinUntilInterrupted);
    let router = Arc::new(ScriptedRouter::new([valid_routes(&example.routes)]));
    let cancel = CancelToken::with_timeout(Duration::from_secs(35));
    let task = tokio::spawn(run_route(
        engine.clone(),
        router,
        example.query,
        cancel.clone(),
    ));
    wait_until(|| engine.evaluate_started()).await;
    // クライアント切断でAxumがハンドラのfutureを破棄するのと同じ
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    wait_until(|| engine.evaluate_finished()).await;
    assert!(cancel.is_cancelled());
    assert_eq!(cancel.check(), Err(Interrupt::Cancelled));
}

async fn wait_until(condition: impl Fn() -> bool) {
    let limit = Instant::now() + STOP_MARGIN;
    while !condition() {
        assert!(Instant::now() < limit, "待機が時間内に終わりませんでした");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}
