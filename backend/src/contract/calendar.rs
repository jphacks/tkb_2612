//! 時間帯・曜日区分の判定の入口。
//!
//! 判定ロジックは②が `domain` に一つだけ実装し、CSV前処理と `/api/conditions` の両方が
//! 同じ実装を同じ祝日表（releaseに同梱した `syukujitsu.csv`）で呼ぶ。
//! 祝日表の読み込みは `data`（②）だけが行い、③は読み込み済みの `Arc<dyn Calendar>` を受け取る。
//! ③は時刻境界や祝日を独自に計算しない。

use chrono::{NaiveDate, NaiveDateTime, Timelike};

use super::conditions::{DayType, Period};

/// Asia/Tokyoのローカル日時を分単位に切り捨てた基準時刻。
///
/// オフセット付き入力からの変換（UTC+9へ換算）と、`/api/conditions` の受理範囲
/// （2000-01-01T00:00〜2100-12-31T23:59）の検査は③の入力検証で行う。
/// 前処理では原本の年月日時分をそのまま渡す（タイムゾーン変換なし）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TokyoMinute(NaiveDateTime);

impl TokyoMinute {
    /// 秒以下を切り捨てて作る。
    pub fn from_local(local: NaiveDateTime) -> Self {
        let truncated = local
            .with_nanosecond(0)
            .and_then(|t| t.with_second(0))
            .expect("秒・ナノ秒の0は常に有効");
        Self(truncated)
    }

    pub fn as_local(&self) -> NaiveDateTime {
        self.0
    }

    pub fn date(&self) -> NaiveDate {
        self.0.date()
    }

    pub fn hour(&self) -> u8 {
        // Timelike::hourは0〜23なのでu8に収まる
        self.0.hour() as u8
    }
}

/// 曜日区分の理由。behavior.md 2章の優先順で最初に当たったもの。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DayTypeReason {
    Weekday,
    Saturday,
    Sunday,
    /// 祝日表の名称が「休日」以外。`name` は祝日表の名称そのまま。
    NationalHoliday {
        name: String,
    },
    /// 祝日表の「休日」のうち振替休日。
    SubstituteHoliday,
    /// 祝日表の「休日」のうち国民の休日。
    CitizensHoliday,
    /// 12/29〜12/31、1/2、1/3。
    YearEnd,
}

impl DayTypeReason {
    pub fn day_type(&self) -> DayType {
        match self {
            DayTypeReason::Weekday => DayType::Weekday,
            _ => DayType::Holiday,
        }
    }

    /// `day_type_reason` に返す文字列。
    pub fn label(&self) -> String {
        match self {
            DayTypeReason::Weekday => "平日".to_owned(),
            DayTypeReason::Saturday => "土曜".to_owned(),
            DayTypeReason::Sunday => "日曜".to_owned(),
            DayTypeReason::NationalHoliday { name } => format!("祝日：{name}"),
            DayTypeReason::SubstituteHoliday => "振替休日".to_owned(),
            DayTypeReason::CitizensHoliday => "国民の休日".to_owned(),
            DayTypeReason::YearEnd => "年末年始".to_owned(),
        }
    }
}

/// 暦日の判定結果。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DayClassification {
    pub reason: DayTypeReason,
    /// falseなら祝日表の収録範囲外で、土日と年末年始だけで判定した。
    pub holiday_table_covered: bool,
}

impl DayClassification {
    pub fn day_type(&self) -> DayType {
        self.reason.day_type()
    }
}

/// 基準時刻の判定結果（`/api/conditions` の period・day_type 部分）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DepartureClass {
    pub period: Period,
    pub day: DayClassification,
}

/// 祝日表を読み込んだ判定器。実装は②の `domain` に一つだけ置く。
///
/// 前処理は発生年月日・時を、`/api/conditions` は基準時刻を渡し、どちらも
/// [`Calendar::classify`]（または同じ実装の [`Calendar::classify_date`]）を呼ぶ。
pub trait Calendar: Send + Sync {
    /// 暦日から曜日区分を判定する。深夜0〜3時もその日付で判定する（前日扱いにしない）。
    fn classify_date(&self, date: NaiveDate) -> DayClassification;

    /// 時間帯と曜日区分をまとめて判定する。上書きしない。
    fn classify(&self, at: TokyoMinute) -> DepartureClass {
        DepartureClass {
            period: Period::from_hour(at.hour()).expect("NaiveDateTimeの時は0〜23"),
            day: self.classify_date(at.date()),
        }
    }
}
