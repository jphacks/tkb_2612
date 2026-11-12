//! 新方式（schema 3）の中間案の選定（behavior.md 5〜8章、docs/multi-route-design.md 4.1節）。
//!
//! `evaluate` の中間案の判定を純粋関数に切り出したものを使い、C0〜C5 の各条件と、
//! 採用しなかった中間案を候補から外したときの fastest・safest の添字を確かめる。

use accident_route_backend::domain::rounding::risk_reduction_percent;
use accident_route_backend::domain::routing::evaluate::{
    MIDDLE_MIN_DETOUR_MIN, MIDDLE_MIN_REDUCTION_RATIO, MIDDLE_MIN_TIME_GAP_S, MiddleSelection,
    middle_search_detour_ds, select_middle,
};

/// 採用する場合の基本値（fastest 0・中間案 1・safest 2）。
fn base() -> (Vec<f64>, Vec<f64>) {
    (vec![600.0, 700.0, 900.0], vec![10.0, 7.0, 4.0])
}

fn select(
    fastest: usize,
    safest: Option<usize>,
    any_accident: bool,
    matching: Option<usize>,
    durations: &[f64],
    risks: &[f64],
) -> MiddleSelection {
    select_middle(fastest, safest, any_accident, matching, durations, risks)
}

#[test]
fn middle_constants_are_the_provisional_values() {
    assert_eq!(MIDDLE_MIN_DETOUR_MIN, 4.0);
    assert_eq!(MIDDLE_MIN_TIME_GAP_S, 60.0);
    assert_eq!(MIDDLE_MIN_REDUCTION_RATIO, 0.5);
}

#[test]
fn middle_detour_is_skipped_below_four_minutes_and_halved_in_whole_minutes() {
    assert_eq!(middle_search_detour_ds(0.0), None);
    assert_eq!(middle_search_detour_ds(3.9), None);
    assert_eq!(middle_search_detour_ds(4.0), Some(1_200));
    assert_eq!(middle_search_detour_ds(4.9), Some(1_200));
    assert_eq!(middle_search_detour_ds(5.0), Some(1_200));
    assert_eq!(middle_search_detour_ds(9.9), Some(2_400));
    assert_eq!(middle_search_detour_ds(10.0), Some(3_000));
    assert_eq!(middle_search_detour_ds(15.0), Some(4_200));
}

#[test]
fn risk_reduction_percent_rounds_to_one_decimal_and_needs_a_positive_base() {
    assert_eq!(risk_reduction_percent(16.0, 15.0), Some(6.2));
    assert_eq!(risk_reduction_percent(16.0, 13.0), Some(18.8));
    assert_eq!(risk_reduction_percent(3.0, 2.0), Some(33.3));
    assert_eq!(risk_reduction_percent(4.0, 5.0), Some(-25.0));
    assert_eq!(risk_reduction_percent(12.21, 12.21), Some(0.0));
    assert_eq!(risk_reduction_percent(0.0, 0.0), None);
    assert_eq!(risk_reduction_percent(-1.0, 0.0), None);
}

#[test]
fn adopts_a_middle_route_when_all_conditions_hold() {
    let (durations, risks) = base();
    let result = select(0, Some(2), true, Some(1), &durations, &risks);
    assert_eq!(
        result,
        MiddleSelection {
            middle: Some(1),
            remove: None,
            fastest: 0,
            safest: Some(2),
        }
    );
}

#[test]
fn c1_requires_safest_to_differ_from_fastest() {
    let (durations, risks) = base();
    let result = select(0, Some(0), true, Some(1), &durations, &risks);
    assert_eq!(result.middle, None);
    assert_eq!(result.remove, Some(1));
    assert_eq!((result.fastest, result.safest), (0, Some(0)));
}

#[test]
fn c2_requires_at_least_one_accident_in_the_candidates() {
    let (durations, risks) = base();
    let result = select(0, Some(2), false, Some(1), &durations, &risks);
    assert_eq!(result.middle, None);
    assert_eq!(result.remove, Some(1));
}

#[test]
fn c3_requires_a_positive_fastest_risk() {
    let (durations, mut risks) = base();
    risks[0] = 0.0;
    let result = select(0, Some(2), true, Some(1), &durations, &risks);
    assert_eq!(result.middle, None);
    assert_eq!(result.remove, Some(1));
}

#[test]
fn c0_keeps_the_middle_route_when_it_is_fastest_or_safest() {
    let (durations, risks) = base();
    let fastest = select(1, Some(2), true, Some(1), &durations, &risks);
    assert_eq!(fastest.middle, None);
    assert_eq!(fastest.remove, None);

    let safest = select(0, Some(1), true, Some(1), &durations, &risks);
    assert_eq!(safest.middle, None);
    assert_eq!(safest.remove, None);
}

#[test]
fn c4_requires_time_and_risk_between_fastest_and_safest() {
    let (_, risks) = base();
    // 中間案がsafestより遅い。
    let result = select(0, Some(2), true, Some(1), &[600.0, 950.0, 900.0], &risks);
    assert_eq!(result.middle, None);
    assert_eq!(result.remove, Some(1));

    // 中間案のリスクがsafest以下。
    let result = select(
        0,
        Some(2),
        true,
        Some(1),
        &[600.0, 700.0, 900.0],
        &[10.0, 4.0, 7.0],
    );
    assert_eq!(result.middle, None);
    assert_eq!(result.remove, Some(1));
}

#[test]
fn c5_requires_a_time_gap_and_enough_reduction() {
    let (_, risks) = base();
    // safestとの差が59秒。
    let result = select(0, Some(2), true, Some(1), &[600.0, 841.0, 900.0], &risks);
    assert_eq!(result.middle, None);
    assert_eq!(result.remove, Some(1));

    // 削減率がsafestの削減率の半分未満（3 < 0.5 * 10）。
    let result = select(
        0,
        Some(2),
        true,
        Some(1),
        &[600.0, 700.0, 900.0],
        &[10.0, 7.0, 0.0],
    );
    assert_eq!(result.middle, None);
    assert_eq!(result.remove, Some(1));
}

#[test]
fn a_missing_match_keeps_the_candidates_unchanged() {
    let (durations, risks) = base();
    let result = select(0, Some(2), true, None, &durations, &risks);
    assert_eq!(
        result,
        MiddleSelection {
            middle: None,
            remove: None,
            fastest: 0,
            safest: Some(2),
        }
    );
}

#[test]
fn removing_a_rejected_middle_shifts_fastest_and_safest() {
    // 中間案の候補1を外すと、safestは2から1へずれる。
    let result = select(
        0,
        Some(2),
        true,
        Some(1),
        &[600.0, 850.0, 900.0],
        &[10.0, 7.0, 4.0],
    );
    assert_eq!(result.middle, None);
    assert_eq!(result.remove, Some(1));
    assert_eq!((result.fastest, result.safest), (0, Some(1)));
}
