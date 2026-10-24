//! 内閣府「国民の祝日」CSVの読込（behavior.md 2章）。
//!
//! 1列目が日付（`YYYY/M/D`、ゼロ埋めの有無を問わない）、2列目が名称。ヘッダ1行を読み飛ばす。
//! 実在しない日付・重複日付・名称の空欄は読込の失敗。判定は `domain::HolidayCalendar`。

use chrono::NaiveDate;

use crate::domain::HolidayCalendar;

use super::csv_syntax::ends_inside_quoted_field;
use super::encoding::decode_text;

/// 祝日表の読込の失敗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HolidayCsvError(pub String);

impl std::fmt::Display for HolidayCsvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "祝日表を読み込めません: {}", self.0)
    }
}

impl std::error::Error for HolidayCsvError {}

/// `YYYY/M/D`。各部はASCII数字だけ（月日は1〜2桁、年は4桁）。
pub fn parse_holiday_date(s: &str) -> Option<NaiveDate> {
    let mut parts = s.trim().split('/');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let digits = |p: &str, len: std::ops::RangeInclusive<usize>| {
        len.contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit())
    };
    if !(digits(y, 4..=4) && digits(m, 1..=2) && digits(d, 1..=2)) {
        return None;
    }
    NaiveDate::from_ymd_opt(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?)
}

/// 祝日CSVのバイト列から判定器を作る。
pub fn parse_holiday_csv(bytes: &[u8]) -> Result<HolidayCalendar, HolidayCsvError> {
    let (_, text) = decode_text(bytes).map_err(|e| HolidayCsvError(e.to_string()))?;
    if ends_inside_quoted_field(&text) {
        return Err(HolidayCsvError(
            "引用符が閉じないままファイルが終わっています".to_owned(),
        ));
    }
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut entries = Vec::new();
    for (i, record) in reader.records().enumerate() {
        // ヘッダが1行目なので、データの行番号は2から
        let line = i + 2;
        let record = record.map_err(|e| HolidayCsvError(format!("CSVの構文が不正です: {e}")))?;
        let (Some(date), Some(name)) = (record.get(0), record.get(1)) else {
            return Err(HolidayCsvError(format!("{line}行目の列が足りません")));
        };
        let date = parse_holiday_date(date)
            .ok_or_else(|| HolidayCsvError(format!("{line}行目の日付が不正です")))?;
        entries.push((date, name.to_owned()));
    }
    HolidayCalendar::new(entries).map_err(|e| HolidayCsvError(e.to_string()))
}
