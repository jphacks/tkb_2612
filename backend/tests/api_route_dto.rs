//! S5：`RouteOutcome` からroute応答JSONへの変換。
//!
//! 比較規則（C1と共通。`common::json_diff`）：数値は数値として比べ（`0` と `0.0` を同一視）、
//! `aggregation_ms` は存在し有限の非負数であることを検査してから値の比較から除く。

mod common;

use std::sync::Arc;

use accident_route_backend::api::route_response::{
    MAX_NAVIGATION_BYTES, MAX_SAFE_INTEGER, NAV_MAX_SECTIONS, NAV_MAX_VERTICES,
    render_route_response,
};
use accident_route_backend::contract::{LonLat, NavigationData, NavigationSection, RouteOutcome};
use accident_route_backend::testing::fixtures::{
    check_outcome_invariants, expected_route_outcome, fixture_dataset_info, route_example,
    route_response_json, route_response_names,
};
use accident_route_backend::testing::{ScriptedRouter, StubEngine, StubEvaluation, valid_routes};
use axum::http::StatusCode;
use serde_json::Value;

use common::{TestAppBuilder, assert_json_matches, check_and_strip_aggregation_ms};

fn render(outcome: &RouteOutcome) -> Value {
    serde_json::from_slice(&render_route_response(outcome).unwrap()).unwrap()
}

#[test]
fn every_fixture_outcome_renders_to_the_fixture_json() {
    let names = route_response_names();
    assert!(!names.is_empty());
    for name in names {
        let outcome = expected_route_outcome(&name);
        check_outcome_invariants(&outcome).unwrap();
        let mut actual = render(&outcome);
        check_and_strip_aggregation_ms(&mut actual);
        let mut expected = route_response_json(&name);
        expected.as_object_mut().unwrap().remove("aggregation_ms");
        assert_json_matches(&actual, &expected);
    }
}

#[test]
fn aggregation_ms_check_rejects_missing_null_and_negative_values() {
    let mut actual = render(&expected_route_outcome("singleRoute"));
    for bad in [Value::Null, serde_json::json!(-1.0), serde_json::json!("1")] {
        let mut value = actual.clone();
        value["aggregation_ms"] = bad;
        let result = std::panic::catch_unwind(move || check_and_strip_aggregation_ms(&mut value));
        assert!(result.is_err());
    }
    actual.as_object_mut().unwrap().remove("aggregation_ms");
    assert!(std::panic::catch_unwind(move || check_and_strip_aggregation_ms(&mut actual)).is_err());
}

#[test]
fn numeric_comparison_treats_integer_and_float_forms_as_equal() {
    let a: Value = serde_json::from_str(r#"{"x":[0,1.5,2]}"#).unwrap();
    let b: Value = serde_json::from_str(r#"{"x":[0.0,1.5,2.0]}"#).unwrap();
    assert_eq!(common::json_diff(&a, &b), None);
    let c: Value = serde_json::from_str(r#"{"x":[0,1.5,2.0000001]}"#).unwrap();
    assert!(common::json_diff(&a, &c).is_some());
}

/// 安全整数を超える値を持つ結果。
fn unsafe_integer_outcome() -> RouteOutcome {
    let mut outcome = expected_route_outcome("singleRoute");
    let candidate = &mut outcome.candidates[0];
    assert!(!candidate.accident_points.is_empty());
    candidate.accident_points[0].id = MAX_SAFE_INTEGER + 1;
    outcome
}

/// navigationが5,000,000バイトを超える結果（頂点数を増やす）。
fn oversized_navigation_outcome() -> RouteOutcome {
    let mut outcome = expected_route_outcome("noAccidents");
    let candidate = &mut outcome.candidates[outcome.fastest];
    let n = 400_000;
    candidate.path.coordinates = (0..n)
        .map(|i| LonLat::new(139.0 + i as f64 * 1e-6, 35.0))
        .collect();
    let nav = candidate.navigation.as_mut().unwrap();
    nav.vertex_distances_m = (0..n).map(|i| i as f64 * 12.345_678_9).collect();
    nav.length_m = *nav.vertex_distances_m.last().unwrap();
    outcome
}

#[test]
fn json_safe_integer_overflow_is_rejected() {
    let mut outcome = expected_route_outcome("singleRoute");
    outcome.candidates[0].accident_points[0].id = MAX_SAFE_INTEGER;
    assert!(render_route_response(&outcome).is_ok());
    assert!(render_route_response(&unsafe_integer_outcome()).is_err());
}

/// 5,000,000バイトを超えるnavigationは拒否する。OpenAPIの範囲（頂点15,000・区間8,233）内では
/// 選定2本の合計は約3.7MBに収まるので、超える結果は頂点数の範囲外でもある。
#[test]
fn navigation_over_five_megabytes_is_rejected() {
    let outcome = oversized_navigation_outcome();
    let nav = outcome.candidates[outcome.fastest]
        .navigation
        .as_ref()
        .unwrap();
    let bytes = serde_json::to_vec(&nav.vertex_distances_m).unwrap().len() as u64;
    assert!(bytes > MAX_NAVIGATION_BYTES, "{bytes}");
    assert!(render_route_response(&outcome).is_err());
}

fn rejection(outcome: &RouteOutcome) -> String {
    render_route_response(outcome)
        .expect_err("契約違反として拒否するはず")
        .0
}

/// OpenAPIの `NavigationData`・`NavigationSection` の範囲外は拒否する（500）。
#[test]
fn navigation_outside_the_openapi_ranges_is_rejected() {
    let base = expected_route_outcome("twoSelected");
    let selected = base.safest.unwrap();
    let with = |edit: &dyn Fn(&mut NavigationData)| {
        let mut outcome = base.clone();
        edit(outcome.candidates[selected].navigation.as_mut().unwrap());
        outcome
    };
    assert!(
        !base.candidates[selected]
            .navigation
            .as_ref()
            .unwrap()
            .sections
            .is_empty()
    );
    for lat in [19.999, 46.001] {
        let message = rejection(&with(&|n| n.projection_lat_deg = lat));
        assert!(message.contains("projection_lat_deg"), "{message}");
    }
    for lat in [20.0, 46.0] {
        assert!(render_route_response(&with(&|n| n.projection_lat_deg = lat)).is_ok());
    }
    let message = rejection(&with(&|n| {
        let scale = 110_000.5 / n.length_m;
        for d in &mut n.vertex_distances_m {
            *d *= scale;
        }
        n.length_m = *n.vertex_distances_m.last().unwrap();
    }));
    assert!(message.contains("length_m"), "{message}");
    // sidの上限16,465（区間を後ろへずらす。相関検査だけでは通ってしまう）
    let message = rejection(&with(&|n| {
        for s in &mut n.sections {
            s.start_segment_index += 20_000;
            s.end_segment_index += 20_000;
        }
    }));
    assert!(message.contains("ナビ区間が不正"), "{message}");
    let message = rejection(&with(&|n| {
        let template = n.sections[0];
        let total: u64 = n
            .sections
            .iter()
            .map(|s| u64::from(s.end_segment_index - s.start_segment_index) + 1)
            .sum();
        assert_eq!(total, 1, "このfixtureでは1区間1本");
        n.sections = (0..NAV_MAX_SECTIONS as u32 + 1)
            .map(|j| NavigationSection {
                start_segment_index: j * 2,
                end_segment_index: j * 2,
                ..template
            })
            .collect();
    }));
    assert!(message.contains("ナビ区間数"), "{message}");
}

/// 頂点数の上限15,000（navigationの頂点距離は頂点と同数）。
#[test]
fn navigation_vertex_count_is_bounded() {
    let mut outcome = expected_route_outcome("noAccidents");
    let candidate = &mut outcome.candidates[outcome.fastest];
    let n = NAV_MAX_VERTICES + 1;
    candidate.path.coordinates = (0..n)
        .map(|i| LonLat::new(139.0 + i as f64 * 1e-6, 35.0))
        .collect();
    let nav = candidate.navigation.as_mut().unwrap();
    nav.vertex_distances_m = (0..n).map(|i| i as f64 * 0.1).collect();
    nav.length_m = *nav.vertex_distances_m.last().unwrap();
    let message = rejection(&outcome);
    assert!(
        message.contains("vertex_distances_mの長さが範囲外"),
        "{message}"
    );
}

#[test]
fn broken_correlations_are_rejected() {
    let mut no_nav = expected_route_outcome("twoSelected");
    let selected = no_nav.safest.unwrap();
    no_nav.candidates[selected].navigation = None;
    assert!(render_route_response(&no_nav).is_err());

    let mut bad_id = expected_route_outcome("singleRoute");
    bad_id.dataset_id = "A".repeat(64);
    assert!(render_route_response(&bad_id).is_err());

    let mut nan = expected_route_outcome("singleRoute");
    nan.candidates[0].weighted_risk = f64::NAN;
    assert!(render_route_response(&nan).is_err());
}

/// 内部契約違反はハンドラで500 internal_error（間引いた200を返さない）。
#[tokio::test]
async fn handler_returns_500_for_invalid_outcomes() {
    for outcome in [unsafe_integer_outcome(), oversized_navigation_outcome()] {
        let router = Arc::new(ScriptedRouter::new([valid_routes(
            &route_example("singleRoute").routes,
        )]));
        let app = TestAppBuilder {
            engine: Some(Arc::new(StubEngine::new(
                fixture_dataset_info(),
                StubEvaluation::Fixed(Box::new(outcome)),
            ))),
            router,
            ..Default::default()
        }
        .build();
        let response = app
            .post_json("/api/route", common::valid_route_body().to_string())
            .await;
        assert_eq!(response.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(response.error_code(), "internal_error");
    }
}
