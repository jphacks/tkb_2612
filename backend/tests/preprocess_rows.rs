//! 前処理の純関数（behavior.md 2章、11章 PreparationTests、#8 ステップ3）。

use accident_route_backend::contract::{DayType, Period, Weather};
use accident_route_backend::data::preprocess::rows::{
    CAR_CODES, Column, ExclusionReason, convert_row, modes_of, parse_coordinates, parse_csv_int,
    parse_dms, party_codes,
};
use accident_route_backend::domain::{HolidayCalendar, py_round};
use accident_route_backend::testing::FixedCalendar;
use chrono::NaiveDate;

#[test]
fn test_dms_examples_and_subsecond_precision() {
    let lat = parse_dms("430607590", 90).unwrap();
    let lon = parse_dms("1412109599", 180).unwrap();
    assert!((lat - 43.102_108_333_333_3).abs() < 1e-10, "{lat}");
    assert!((lon - 141.352_666_388_888_9).abs() < 1e-10, "{lon}");
    // 0.001秒角の差が保たれる
    let a = parse_dms("350000000", 90).unwrap();
    let b = parse_dms("350000001", 90).unwrap();
    assert!((b - a - 1.0 / 3_600_000.0).abs() < 1e-12);
    assert_eq!(parse_dms(" 350000000 ", 90), Some(35.0));
}

#[test]
fn test_invalid_dms() {
    for bad in [
        "",
        "0",
        "356000000",  // 分60
        "350060000",  // 秒60.000
        "35000000.0", // 小数
        "-350000000",
        "+350000000",
        "３５００００００００",
        "1e9",
    ] {
        assert_eq!(parse_dms(bad, 90), None, "{bad:?}");
    }
    // 緯度は0<D<90、経度は0<D<180
    assert_eq!(parse_dms("900000000", 90), None);
    assert!(parse_dms("1790000000", 180).is_some());
    assert_eq!(parse_dms("1800000000", 180), None);
    // 変換後に日本周辺の範囲（緯度20〜46、経度122〜154、両端を含む）
    assert!(parse_coordinates("200000000", "1220000000").is_some());
    assert!(parse_coordinates("460000000", "1540000000").is_some());
    assert_eq!(parse_coordinates("195959999", "1390000000"), None);
    assert_eq!(parse_coordinates("350000000", "1540000001"), None);
}

#[test]
fn test_modes_include_both_parties_without_motorcycle_leakage() {
    assert_eq!(modes_of(61, 3), 5);
    assert_eq!(modes_of(51, 52), 2);
    assert_eq!(modes_of(3, 52), 6);
    assert_eq!(modes_of(61, 61), 1);
    for code in CAR_CODES {
        assert_eq!(modes_of(code, 0), 4, "{code}");
    }
    for code in [0, 31, 32, 33, 34, 35, 36, 41, 42, 43, 59, 71, 72, 75, 76] {
        assert_eq!(modes_of(code, code), 0, "{code}");
    }
    assert!(!party_codes(2023).contains(&43));
    assert!(party_codes(2024).contains(&43));
    assert!(party_codes(2025).contains(&43));
}

#[test]
fn test_time_boundaries() {
    for hour in 0u8..=23 {
        let expected = match hour {
            0..=3 => Period::LateNight,
            4..=8 => Period::EarlyMorning,
            9..=15 => Period::Daytime,
            16..=18 => Period::Evening,
            _ => Period::Night,
        };
        assert_eq!(Period::from_hour(hour), Some(expected), "{hour}");
    }
    for bad in [24u8, 99] {
        assert_eq!(Period::from_hour(bad), None);
    }
    // 行としては -1/24/99 時を除外する
    for hour in ["-1", "24", "99"] {
        assert_eq!(
            convert(&[(Column::Hour, hour)], 2025),
            Err(ExclusionReason::InvalidDatetime),
            "{hour}"
        );
    }
}

#[test]
fn csv_integer_notation() {
    for (s, v) in [("0", 0), ("07", 7), (" 12 ", 12), ("000123", 123)] {
        assert_eq!(parse_csv_int(s), Some(v), "{s:?}");
    }
    for s in ["-1", "+1", "1.0", "１２", "1,000", "", "  ", "1e3", "0x1"] {
        assert_eq!(parse_csv_int(s), None, "{s:?}");
    }
}

#[test]
fn half_even_rounding_matches_python() {
    // Pythonの round(x, n) の結果
    for (x, digits, expected) in [
        (0.5, 0, 0.0),
        (1.5, 0, 2.0),
        (2.5, 0, 2.0),
        (2.675, 2, 2.67), // 二進表現は2.67499…
        (0.125, 2, 0.12), // ちょうど半分は偶数側
        (0.375, 2, 0.38),
        (12.345, 2, 12.35), // 二進表現は12.3450000…01
        (1.0000005, 6, 1.000001),
        (0.25, 1, 0.2),
        (0.35, 1, 0.3),
        (70.15, 1, 70.2),
        (139.000005, 5, 139.0), // 二進表現は139.00000499…
        (35.123456785, 7, 35.1234568),
        (3.3085, 3, 3.308),
    ] {
        assert_eq!(py_round(x, digits), expected, "round({x}, {digits})");
    }
}

/// 有効な2025年の行の値。`overrides` で列を差し替える。
fn base(column: Column) -> &'static str {
    match column {
        Column::Lat => "350000000",
        Column::Lon => "1390000000",
        Column::Weather => "3",
        Column::Daynight => "12",
        Column::AccidentType => "21",
        Column::Deaths => "1",
        Column::Injuries => "2",
        Column::Year => "2024",
        Column::Month => "12",
        Column::Day => "31",
        Column::Hour => "23",
        Column::Minute => "59",
        Column::PartyA => "03",
        Column::PartyB => "61",
        Column::InjuryA => "4",
        Column::InjuryB => "1",
    }
}

fn convert(
    overrides: &[(Column, &'static str)],
    source_year: u16,
) -> Result<accident_route_backend::data::preprocess::rows::AccidentRecord, ExclusionReason> {
    let calendar = HolidayCalendar::new([(
        NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
        "元日".to_owned(),
    )])
    .unwrap();
    convert_row(
        |c| {
            Some(
                overrides
                    .iter()
                    .find(|(k, _)| *k == c)
                    .map_or_else(|| base(c), |(_, v)| *v),
            )
        },
        source_year,
        &calendar,
    )
}

#[test]
fn valid_row_and_output_fields() {
    let r = convert(&[], 2025).unwrap();
    assert_eq!(r.modes, 5);
    assert_eq!(r.period, Period::Night);
    assert_eq!(r.weather, Weather::Rain);
    // 2024-12-31は年末年始。2024年は表の範囲外だが年末年始は表に依存しない
    assert_eq!(r.day_type, DayType::Holiday);
    assert!(!r.holiday_table_covered);
    // 発生年（2024）とsource_year（2025）を区別する
    assert_eq!(r.source_year, 2025);
    assert_eq!(r.date_number(), 20241231);
    assert_eq!(r.severity(), "死亡");
    assert_eq!(
        r.output_fields(),
        [
            "35.0000000",
            "139.0000000",
            "5",
            "3",
            "3",
            "1",
            "1",
            "2",
            "2025",
            "20241231",
            "23",
            "59",
            "12",
            "21",
            "4",
            "1",
            "死亡"
        ]
        .map(str::to_owned)
    );
}

#[test]
fn exclusion_reasons_and_priority() {
    use Column::*;
    use ExclusionReason::*;
    for (overrides, reason) in [
        (vec![(Lat, "0")], InvalidCoordinates),
        (vec![(Lon, "")], InvalidCoordinates),
        (vec![(Month, "00")], InvalidDatetime),
        (vec![(Month, "2"), (Day, "30")], InvalidDatetime),
        (vec![(Minute, "60")], InvalidDatetime),
        (vec![(Year, "1899")], InvalidDatetime),
        (vec![(Year, "2026")], InvalidDatetime), // source_yearより後
        (vec![(Deaths, "0"), (Injuries, "0")], InvalidCasualties),
        (vec![(Deaths, "1000")], InvalidCasualties),
        (vec![(Injuries, "1.0")], InvalidCasualties),
        (vec![(PartyA, "99")], UnknownPartyCode),
        (vec![(PartyB, "-1")], UnknownPartyCode),
        (vec![(Weather, "9")], UnknownWeatherCode),
        (vec![(Weather, "0")], UnknownWeatherCode),
        (vec![(InjuryA, "3")], UnknownInjuryCode),
        (vec![(Daynight, "14")], UnknownDaynightCode),
        (vec![(AccidentType, "x")], InvalidAccidentType),
        // 複数の不正は 座標→日時→人数→分類値 の順で最初の理由だけ
        (
            vec![(Lat, "0"), (Month, "00"), (Weather, "9")],
            InvalidCoordinates,
        ),
        (
            vec![(Month, "00"), (Deaths, "x"), (PartyA, "99")],
            InvalidDatetime,
        ),
        (vec![(Deaths, "x"), (PartyA, "99")], InvalidCasualties),
        (vec![(PartyA, "99"), (Weather, "9")], UnknownPartyCode),
        (vec![(Weather, "9"), (InjuryA, "3")], UnknownWeatherCode),
    ] {
        assert_eq!(
            convert(&overrides, 2025).map(|_| ()),
            Err(reason),
            "{overrides:?}"
        );
    }
    // 43（特定小型原付）はsource_yearが2024/2025のときだけ許容し、自転車に混ぜない
    assert_eq!(
        convert(&[(Column::PartyA, "43"), (Column::Year, "2023")], 2023).map(|_| ()),
        Err(UnknownPartyCode)
    );
    let r = convert(&[(Column::PartyA, "43"), (Column::Year, "2023")], 2024).unwrap();
    assert_eq!(r.modes, 1);
    // 列がない
    let calendar = FixedCalendar(accident_route_backend::contract::DayClassification {
        reason: accident_route_backend::contract::DayTypeReason::Weekday,
        holiday_table_covered: true,
    });
    let missing = convert_row(|c| (c != Column::InjuryB).then(|| base(c)), 2025, &calendar);
    assert_eq!(missing.map(|_| ()), Err(MissingField));
}
