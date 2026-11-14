//! 許容時間Dの計算（behavior.md 4章「許容時間」）。

use accident_route_backend::domain::engine::{auto_detour_s, manual_detour_s};
use accident_route_backend::domain::routing::search::route::{
    auto_detour_ds, detour_limit_min_from_ds, manual_detour_ds,
};

#[test]
fn test_auto_detour_limit_from_fastest_duration() {
    // 新方式：T0（デシ秒）からD（デシ秒）を決める。5分→3分、20分→6分、45分→13.5分、120分→30分。
    for (t0_min, expected_min) in [(5.0, 3.0), (20.0, 6.0), (45.0, 13.5), (120.0, 30.0)] {
        let t0_ds = (t0_min * 600.0) as u64;
        let detour_ds = auto_detour_ds(t0_ds);
        assert_eq!(
            detour_limit_min_from_ds(detour_ds),
            expected_min,
            "{t0_min}分"
        );
    }
    // 旧方式：T0（秒）からD（秒）を決める。
    for (t0_min, expected_min) in [(5.0, 3.0), (20.0, 6.0), (45.0, 13.5), (120.0, 30.0)] {
        let t0_s = t0_min * 60.0;
        assert_eq!(auto_detour_s(t0_s), expected_min * 60.0, "{t0_min}分");
    }

    // 新方式はT0の20秒ごとに6秒ずつ増える（20分ちょうどと、その20秒後）。
    assert_eq!(auto_detour_ds(12_000), 3_600);
    assert_eq!(auto_detour_ds(12_199), 3_600);
    assert_eq!(auto_detour_ds(12_200), 3_660);
    // 旧方式も同じ境界。
    assert_eq!(auto_detour_s(1_219.999), 360.0);
    assert_eq!(auto_detour_s(1_220.0), 366.0);

    // 下限3分・上限30分。自動で0分にはならない。
    assert_eq!(auto_detour_ds(0), 1_800);
    assert_eq!(auto_detour_s(0.0), 180.0);
    assert_eq!(auto_detour_ds(u64::MAX), 18_000);
    assert_eq!(auto_detour_s(f64::MAX), 1_800.0);

    // 手動：新方式は floor(600×分)、旧方式は分×60秒。
    for (minutes, detour_ds, detour_s) in [
        (0.0, 0, 0.0),
        (7.5, 4_500, 450.0),
        (10.0, 6_000, 600.0),
        (30.0, 18_000, 1_800.0),
        (0.001, 0, 0.06),
    ] {
        assert_eq!(manual_detour_ds(minutes), detour_ds, "{minutes}分");
        assert_eq!(manual_detour_s(minutes), detour_s, "{minutes}分");
    }
}
