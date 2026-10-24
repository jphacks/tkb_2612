//! 計算中の中断・決定性（README.md「期限・キャンセル」、#8 ステップ14）。

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use accident_route_backend::contract::{
    CancelToken, CandidateSet, ConditionFilter, EngineError, Interrupt, Mode, RouteEngine,
};

use common::{Acc, engine, query, straight_east};

const KX35: f64 =
    accident_route_backend::contract::geo::METERS_PER_DEGREE * 0.819_152_044_288_991_8;

/// 60kmの直線の周辺に事故を密に置いた重い入力（6候補）。
fn heavy() -> (
    Arc<accident_route_backend::domain::AccidentEngine>,
    CandidateSet,
) {
    let accidents: Vec<Acc> = (0..200_000)
        .map(|i| {
            let x = (i as f64 * 0.3) % 60_000.0;
            let north = ((i * 7919) % 140) as f64 - 70.0;
            Acc::at(
                139.0 + x / KX35,
                35.0 + north / accident_route_backend::contract::geo::METERS_PER_DEGREE,
            )
            .casualties((i % 50 == 0) as u16, 1)
        })
        .collect();
    let routes = (0..6)
        .map(|i| straight_east(3600.0 + i as f64, 60_000.0, 60_000.0))
        .collect();
    (
        engine(&accidents),
        CandidateSet {
            routes,
            warnings: vec![],
        },
    )
}

#[test]
fn cancelled_or_expired_token_interrupts_evaluate() {
    let (e, candidates) = heavy();
    let q = query(Mode::Walk, 10.0, ConditionFilter::default());
    let cancelled = CancelToken::unbounded();
    cancelled.cancel();
    assert_eq!(
        e.evaluate(&q, candidates.clone(), &cancelled).unwrap_err(),
        EngineError::Interrupted(Interrupt::Cancelled)
    );
    let expired = CancelToken::with_timeout(Duration::ZERO);
    assert_eq!(
        e.evaluate(&q, candidates, &expired).unwrap_err(),
        EngineError::Interrupted(Interrupt::DeadlineExceeded)
    );
}

#[test]
fn cancel_from_another_thread_stops_cpu_work_promptly() {
    let (e, candidates) = heavy();
    let q = query(Mode::Walk, 10.0, ConditionFilter::default());

    // 中断しない場合の所要時間（性能の目安として記録する。合否の基準ではない）
    let started = Instant::now();
    let outcome = e
        .evaluate(&q, candidates.clone(), &CancelToken::unbounded())
        .unwrap();
    let full = started.elapsed();
    eprintln!(
        "PERF: 200,000事故・60km×6候補の評価 {:.1}ms（aggregation_ms={}）",
        full.as_secs_f64() * 1000.0,
        outcome.aggregation_ms
    );
    assert_eq!(outcome.candidates[0].totals.accident_count, 200_000);

    let cancel = CancelToken::unbounded();
    let remote = cancel.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(5));
        remote.cancel();
        Instant::now()
    });
    let result = e.evaluate(&q, candidates, &cancel);
    let returned = Instant::now();
    let cancelled_at = canceller.join().unwrap();
    if full < Duration::from_millis(50) {
        eprintln!("SKIP: 評価が速すぎるため取消しの到達を確かめられない（{full:?}）");
        return;
    }
    assert_eq!(
        result.unwrap_err(),
        EngineError::Interrupted(Interrupt::Cancelled)
    );
    // 余裕を持たせた上限（約10msごとの確認の要件を、不安定にならない範囲で確かめる）
    let lag = returned.saturating_duration_since(cancelled_at);
    assert!(
        lag < Duration::from_millis(500),
        "取消しから{lag:?}かかった"
    );
}
