//! `Calendar` のスタブ。

use chrono::NaiveDate;

use crate::contract::{Calendar, DayClassification};

/// どの日付にも同じ曜日区分を返す。時間帯は契約の `Calendar::classify` が時から決める。
#[derive(Debug, Clone)]
pub struct FixedCalendar(pub DayClassification);

impl Calendar for FixedCalendar {
    fn classify_date(&self, _date: NaiveDate) -> DayClassification {
        self.0.clone()
    }
}
