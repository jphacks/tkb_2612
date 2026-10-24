//! 曜日区分の判定（behavior.md 2章「曜日区分と祝日表」）。
//!
//! `contract::Calendar` の唯一の実装。祝日表のファイルを読むのは `data` で、ここは
//! 読み込み済みの (日付, 名称) の組から判定器を作る。前処理と `/api/conditions` が同じ型を使う。

use std::collections::BTreeMap;

use chrono::{Datelike, Days, NaiveDate, Weekday};

use crate::contract::{Calendar, DayClassification, DayTypeReason};

/// 祝日表で振替休日・国民の休日を表す名称。
pub const GENERIC_HOLIDAY_NAME: &str = "休日";

/// 祝日表の内容が不正。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HolidayTableError {
    Empty,
    DuplicateDate(NaiveDate),
    EmptyName(NaiveDate),
}

impl std::fmt::Display for HolidayTableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HolidayTableError::Empty => write!(f, "祝日表に日付がありません"),
            HolidayTableError::DuplicateDate(d) => write!(f, "祝日表の日付が重複しています: {d}"),
            HolidayTableError::EmptyName(d) => write!(f, "祝日表の名称が空です: {d}"),
        }
    }
}

impl std::error::Error for HolidayTableError {}

/// 祝日表を読み込んだ判定器。
#[derive(Debug, Clone)]
pub struct HolidayCalendar {
    holidays: BTreeMap<NaiveDate, String>,
    coverage_start: NaiveDate,
    coverage_end: NaiveDate,
}

impl HolidayCalendar {
    /// 祝日表の行から作る。重複日付・空欄の名称・空の表は失敗。名称は前後の空白を除いて保持する。
    pub fn new(
        entries: impl IntoIterator<Item = (NaiveDate, String)>,
    ) -> Result<Self, HolidayTableError> {
        let mut holidays = BTreeMap::new();
        for (date, name) in entries {
            let name = name.trim().to_owned();
            if name.is_empty() {
                return Err(HolidayTableError::EmptyName(date));
            }
            if holidays.insert(date, name).is_some() {
                return Err(HolidayTableError::DuplicateDate(date));
            }
        }
        let (Some(first), Some(last)) = (holidays.keys().next(), holidays.keys().next_back())
        else {
            return Err(HolidayTableError::Empty);
        };
        // 収録範囲は最小日付の年の1月1日〜最大日付の年の12月31日
        let coverage_start = NaiveDate::from_ymd_opt(first.year(), 1, 1).expect("1月1日は有効");
        let coverage_end = NaiveDate::from_ymd_opt(last.year(), 12, 31).expect("12月31日は有効");
        Ok(Self {
            holidays,
            coverage_start,
            coverage_end,
        })
    }

    /// 表中の最小日付と最大日付。
    pub fn table_range(&self) -> (NaiveDate, NaiveDate) {
        (
            *self.holidays.keys().next().expect("空の表は作れない"),
            *self.holidays.keys().next_back().expect("空の表は作れない"),
        )
    }

    /// 収録範囲（両端を含む）。範囲外は土日と年末年始だけで判定する。
    pub fn coverage(&self) -> (NaiveDate, NaiveDate) {
        (self.coverage_start, self.coverage_end)
    }

    /// 祝日表の行数。
    pub fn len(&self) -> usize {
        self.holidays.len()
    }

    pub fn is_empty(&self) -> bool {
        self.holidays.is_empty()
    }

    pub fn is_covered(&self, date: NaiveDate) -> bool {
        (self.coverage_start..=self.coverage_end).contains(&date)
    }

    /// 「休日」の日が振替休日か。直前の日から日曜日まで連続して祝日表にあり、
    /// その日曜日自体も祝日表の日なら振替休日、それ以外は国民の休日。
    fn is_substitute(&self, date: NaiveDate) -> bool {
        let mut day = date;
        while let Some(previous) = day.checked_sub_days(Days::new(1)) {
            if !self.holidays.contains_key(&previous) {
                return false;
            }
            if previous.weekday() == Weekday::Sun {
                return true;
            }
            day = previous;
        }
        false
    }
}

/// 12月29日〜12月31日、1月2日、1月3日。1月1日は祝日表の元日として扱う。
fn is_year_end(date: NaiveDate) -> bool {
    matches!((date.month(), date.day()), (12, 29..=31) | (1, 2..=3))
}

impl Calendar for HolidayCalendar {
    fn classify_date(&self, date: NaiveDate) -> DayClassification {
        let holiday_table_covered = self.is_covered(date);
        let listed = if holiday_table_covered {
            self.holidays.get(&date)
        } else {
            None
        };
        let reason = match listed {
            Some(name) if name != GENERIC_HOLIDAY_NAME => {
                DayTypeReason::NationalHoliday { name: name.clone() }
            }
            Some(_) if self.is_substitute(date) => DayTypeReason::SubstituteHoliday,
            Some(_) => DayTypeReason::CitizensHoliday,
            None if is_year_end(date) => DayTypeReason::YearEnd,
            None => match date.weekday() {
                Weekday::Sat => DayTypeReason::Saturday,
                Weekday::Sun => DayTypeReason::Sunday,
                _ => DayTypeReason::Weekday,
            },
        };
        DayClassification {
            reason,
            holiday_table_covered,
        }
    }
}
