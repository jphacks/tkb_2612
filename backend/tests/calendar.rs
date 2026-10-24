//! 時間帯・曜日区分・祝日表（behavior.md 2章、11章 追加試験6〜8）。
//!
//! 前処理（`data::preprocess::rows::convert_row`）と `/api/conditions`（`Calendar::classify`）が
//! 同じ `HolidayCalendar` を使うことも確かめる。

use chrono::{NaiveDate, NaiveDateTime};

use accident_route_backend::contract::{Calendar, DayType, DayTypeReason, Period, TokyoMinute};
use accident_route_backend::data::holidays::{parse_holiday_csv, parse_holiday_date};
use accident_route_backend::data::preprocess::rows::{Column, convert_row};
use accident_route_backend::domain::HolidayCalendar;

/// 2009〜2026年の一部の祝日（人工の表。実際の祝日表から必要な日だけを抜いたもの）。
const TABLE: &str = "\
国民の祝日・休日月日,国民の祝日・休日名称
2009/1/1,元日
2009/5/3,憲法記念日
2009/5/4,みどりの日
2009/5/5,こどもの日
2009/5/6,休日
2023/1/1,元日
2023/1/2,休日
2025/1/1,元日
2025/5/3,憲法記念日
2025/11/23,勤労感謝の日
2025/11/24,休日
2026/1/1,元日
2026/9/21,敬老の日
2026/9/22,休日
2026/9/23,秋分の日
";

fn calendar() -> HolidayCalendar {
    parse_holiday_csv(TABLE.as_bytes()).expect("人工の祝日表")
}

fn date(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn at(s: &str) -> TokyoMinute {
    TokyoMinute::from_local(NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap())
}

#[test]
fn period_boundaries() {
    let c = calendar();
    for (time, period) in [
        ("2025-01-06 03:59", Period::LateNight),
        ("2025-01-06 04:00", Period::EarlyMorning),
        ("2025-01-06 08:59", Period::EarlyMorning),
        ("2025-01-06 09:00", Period::Daytime),
        ("2025-01-06 15:59", Period::Daytime),
        ("2025-01-06 16:00", Period::Evening),
        ("2025-01-06 18:59", Period::Evening),
        ("2025-01-06 19:00", Period::Night),
        ("2025-01-06 23:59", Period::Night),
        ("2025-01-07 00:00", Period::LateNight),
    ] {
        assert_eq!(c.classify(at(time)).period, period, "{time}");
    }
}

#[test]
fn day_type_reasons() {
    let c = calendar();
    let national = |name: &str| DayTypeReason::NationalHoliday {
        name: name.to_owned(),
    };
    for (d, reason) in [
        ("2025-01-06", DayTypeReason::Weekday),
        ("2025-01-11", DayTypeReason::Saturday),
        ("2025-01-12", DayTypeReason::Sunday),
        ("2025-11-23", national("勤労感謝の日")), // 日曜の祝日
        ("2025-05-03", national("憲法記念日")),   // 土曜の祝日
        ("2025-11-24", DayTypeReason::SubstituteHoliday),
        ("2009-05-06", DayTypeReason::SubstituteHoliday), // 連休明けの振替休日
        ("2026-09-22", DayTypeReason::CitizensHoliday),
        ("2025-12-28", DayTypeReason::Sunday),
        ("2026-12-28", DayTypeReason::Weekday),
        ("2025-12-29", DayTypeReason::YearEnd),
        ("2025-12-31", DayTypeReason::YearEnd),
        ("2026-01-01", national("元日")),
        ("2026-01-02", DayTypeReason::YearEnd),
        ("2026-01-03", DayTypeReason::YearEnd),
        ("2023-01-04", DayTypeReason::Weekday), // 1/4は平日
        ("2026-01-05", DayTypeReason::Weekday),
        ("2023-01-02", DayTypeReason::SubstituteHoliday), // 1/2が振替休日の年
    ] {
        let day = c.classify_date(date(d));
        assert_eq!(day.reason, reason, "{d}");
        assert!(day.holiday_table_covered, "{d}");
        let expected_type = if reason == DayTypeReason::Weekday {
            DayType::Weekday
        } else {
            DayType::Holiday
        };
        assert_eq!(day.day_type(), expected_type, "{d}");
    }
}

#[test]
fn late_night_uses_the_same_calendar_day() {
    let c = calendar();
    // 2025-11-24（振替休日）の深夜1時は前日扱いにしない
    let class = c.classify(at("2025-11-24 01:00"));
    assert_eq!(class.period, Period::LateNight);
    assert_eq!(class.day.reason, DayTypeReason::SubstituteHoliday);
}

#[test]
fn outside_table_coverage_uses_weekends_and_year_end_only() {
    let c = calendar();
    assert_eq!(c.coverage(), (date("2009-01-01"), date("2026-12-31")));
    for (d, reason) in [
        ("2008-05-06", DayTypeReason::Weekday), // 実際の振替休日でも表の範囲外なので平日
        ("2027-01-01", DayTypeReason::Weekday), // 金曜。範囲外なので元日として扱わない
        ("2027-01-02", DayTypeReason::YearEnd),
        ("2027-01-09", DayTypeReason::Saturday),
        ("2027-01-10", DayTypeReason::Sunday),
        ("2008-12-30", DayTypeReason::YearEnd),
    ] {
        let day = c.classify_date(date(d));
        assert_eq!(day.reason, reason, "{d}");
        assert!(!day.holiday_table_covered, "{d}");
    }
}

#[test]
fn holiday_csv_formats_and_encodings() {
    assert_eq!(parse_holiday_date("2025/1/1"), Some(date("2025-01-01")));
    assert_eq!(parse_holiday_date("2025/01/01"), Some(date("2025-01-01")));
    assert_eq!(parse_holiday_date(" 2025/11/3 "), Some(date("2025-11-03")));
    for bad in [
        "2025-01-01",
        "2025/2/30",
        "2025/13/1",
        "25/1/1",
        "2025/1",
        "2025/1/1/1",
        "２０２５/1/1",
        "",
    ] {
        assert_eq!(parse_holiday_date(bad), None, "{bad:?}");
    }

    let utf8 = "日付,名称\n2025/1/1,元日\n2025/01/13,成人の日\n";
    let bom = [b"\xEF\xBB\xBF".as_slice(), utf8.as_bytes()].concat();
    let (cp932, _, unmappable) = encoding_rs::SHIFT_JIS.encode(utf8);
    assert!(!unmappable);
    for bytes in [utf8.as_bytes().to_vec(), bom, cp932.into_owned()] {
        let c = parse_holiday_csv(&bytes).expect("UTF-8・BOM付き・cp932");
        assert_eq!(c.len(), 2);
        assert_eq!(
            c.classify_date(date("2025-01-13")).reason,
            DayTypeReason::NationalHoliday {
                name: "成人の日".to_owned()
            }
        );
    }
}

#[test]
fn invalid_holiday_tables_fail() {
    for (case, csv) in [
        ("重複日付", "日付,名称\n2025/1/1,元日\n2025/01/01,元日\n"),
        ("不正な日付", "日付,名称\n2025/2/29,休日\n"),
        ("空欄の名称", "日付,名称\n2025/1/1, \n"),
        ("列不足", "日付,名称\n2025/1/1\n"),
        ("空の表", "日付,名称\n"),
    ] {
        assert!(parse_holiday_csv(csv.as_bytes()).is_err(), "{case}");
    }
    // UTF-8でもcp932でも復号できないバイト列
    assert!(parse_holiday_csv(b"\xFF\xFF,\xFF\n2025/1/1,x\n").is_err());
}

#[test]
fn real_holiday_table_from_cabinet_office() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../accident/syukujitsu.csv");
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!(
            "SKIP: {} がないため実祝日表の試験を省略（合格ではない）",
            path.display()
        );
        return;
    };
    let c = parse_holiday_csv(&bytes).expect("内閣府の祝日表");
    assert_eq!(
        c.classify_date(date("2025-11-24")).reason,
        DayTypeReason::SubstituteHoliday
    );
    assert_eq!(
        c.classify_date(date("2009-05-06")).reason,
        DayTypeReason::SubstituteHoliday
    );
    assert_eq!(
        c.classify_date(date("2026-09-22")).reason,
        DayTypeReason::CitizensHoliday
    );
    assert_eq!(
        c.classify_date(date("2023-01-02")).reason,
        DayTypeReason::SubstituteHoliday
    );
    assert_eq!(c.coverage().0, date("1955-01-01"));
}

#[test]
fn preprocessing_and_conditions_share_the_calendar() {
    let c = calendar();
    // 2025-11-24 01:30 の事故：前処理の時間帯・曜日区分が classify と同じになる
    let value = |column: Column| -> Option<&'static str> {
        Some(match column {
            Column::Lat => "350000000",
            Column::Lon => "1390000000",
            Column::Weather => "1",
            Column::Daynight => "22",
            Column::AccidentType => "21",
            Column::Deaths => "0",
            Column::Injuries => "1",
            Column::Year => "2025",
            Column::Month => "11",
            Column::Day => "24",
            Column::Hour => "01",
            Column::Minute => "30",
            Column::PartyA => "3",
            Column::PartyB => "61",
            Column::InjuryA => "4",
            Column::InjuryB => "2",
        })
    };
    let record = convert_row(value, 2025, &c).expect("有効な行");
    let class = c.classify(at("2025-11-24 01:30"));
    assert_eq!(record.period, class.period);
    assert_eq!(record.day_type, class.day.day_type());
    assert!(record.holiday_table_covered);
}
