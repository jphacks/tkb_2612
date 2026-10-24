//! 人工releaseの生成ヘルパー（#8 ステップ8、M1）。③がC3・C4で同じものを使う。

use accident_route_backend::contract::DayTypeReason;
use accident_route_backend::data;
use accident_route_backend::data::holidays::parse_holiday_csv;
use accident_route_backend::testing::fixtures::{
    fixture_dataset_info, route_example, route_response_names,
};
use accident_route_backend::testing::release::{TEST_HOLIDAY_CSV, example_records};
use accident_route_backend::testing::{TestRelease, example_router};
use chrono::NaiveDate;

#[test]
fn every_example_release_loads_with_its_own_accident_count() {
    for name in route_response_names() {
        let example = route_example(&name);
        let release = TestRelease::from_example(&example);
        let calendar = data::load_calendar(release.path()).unwrap();
        let engine = data::load_engine(release.path()).unwrap();
        let info = engine.dataset();
        // accidentsはその例のaccident_idsの件数（fixture_dataset_info().accidentsとは比べない）
        assert_eq!(info.accidents, example.accident_ids.len() as u64, "{name}");
        assert_eq!(info.data_years, fixture_dataset_info().data_years, "{name}");
        assert_eq!(info.dataset_id, release.ids.dataset_id);
        assert_eq!(info.release_id, release.ids.release_id);
        assert_ne!(info.dataset_id, "0".repeat(64), "実際のハッシュ");
        // 同梱の祝日表（2025年）で判定できる
        let d = NaiveDate::from_ymd_opt(2025, 11, 24).unwrap();
        assert_eq!(
            calendar.classify_date(d).reason,
            DayTypeReason::SubstituteHoliday
        );
        let outside = NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
        assert!(!calendar.classify_date(outside).holiday_table_covered);
    }
    // 現行の3例の件数
    let count = |name: &str| route_example(name).accident_ids.len();
    assert_eq!(
        (
            count("singleRoute"),
            count("noAccidents"),
            count("twoSelected")
        ),
        (1, 3, 3)
    );
}

#[test]
fn release_directory_is_removed_on_drop() {
    let release = TestRelease::for_example("singleRoute");
    let path = release.path().to_owned();
    assert!(path.join("accidents.csv.gz").exists());
    assert!(path.join("metadata.json").exists());
    assert!(path.join("syukujitsu.csv").exists());
    drop(release);
    assert!(!path.exists());
}

#[test]
fn non_sequential_accident_ids_are_rejected() {
    let calendar = parse_holiday_csv(TEST_HOLIDAY_CSV.as_bytes()).unwrap();
    let mut example = route_example("twoSelected");
    example.accident_ids = vec![0, 2];
    assert!(example_records(&example, &calendar).is_err());
    example.accident_ids = vec![1];
    assert!(example_records(&example, &calendar).is_err());
    example.accident_ids = vec![0, 1, 2];
    // 座標は7桁に丸めて書かれる（35.001000000000005 → 35.0010000）
    let records = example_records(&example, &calendar).unwrap();
    assert_eq!(records[2].output_fields()[0], "35.0010000");
}

#[test]
fn example_router_repeats_routes_for_short_examples() {
    for name in route_response_names() {
        let example = route_example(&name);
        let router = example_router(&example);
        let expected = if example.routes.len() < 3 { 3 } else { 1 };
        assert_eq!(router.remaining(), expected, "{name}");
    }
}
