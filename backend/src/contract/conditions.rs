//! 移動手段・時間帯・天候・曜日区分。
//!
//! 内部コードと日本語ラベルの対応はここだけに置く（behavior.md 2章、openapi.yaml）。
//! 整数コード・null・`時間帯` キー等の互換入力の解釈と優先順はAPI層（③）が行う。

/// 移動手段。CSVの `modes` はこのマスクのOR。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Walk,
    Bike,
    Car,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Walk, Mode::Bike, Mode::Car];

    /// 事故の `modes` とANDを取るビット。徒歩1・自転車2・自動車4。
    pub const fn mask(self) -> u8 {
        match self {
            Mode::Walk => 1,
            Mode::Bike => 2,
            Mode::Car => 4,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::Walk => "walk",
            Mode::Bike => "bike",
            Mode::Car => "car",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == s)
    }
}

/// 時間帯（5区分）。内部コードは0〜4。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Period {
    /// 4:00〜8:59
    EarlyMorning,
    /// 9:00〜15:59
    Daytime,
    /// 16:00〜18:59
    Evening,
    /// 19:00〜23:59
    Night,
    /// 0:00〜3:59
    LateNight,
}

impl Period {
    pub const ALL: [Period; 5] = [
        Period::EarlyMorning,
        Period::Daytime,
        Period::Evening,
        Period::Night,
        Period::LateNight,
    ];

    pub const fn code(self) -> u8 {
        match self {
            Period::EarlyMorning => 0,
            Period::Daytime => 1,
            Period::Evening => 2,
            Period::Night => 3,
            Period::LateNight => 4,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.code() == code)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Period::EarlyMorning => "早朝",
            Period::Daytime => "昼",
            Period::Evening => "夕",
            Period::Night => "夜",
            Period::LateNight => "深夜",
        }
    }

    pub fn from_label(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.label() == s)
    }

    /// 時（0〜23）から時間帯を決める唯一の関数。分は使わない。
    /// 前処理・`/api/conditions`・routeの `hour` 入力のすべてがこれを使う。
    pub const fn from_hour(hour: u8) -> Option<Self> {
        match hour {
            0..=3 => Some(Period::LateNight),
            4..=8 => Some(Period::EarlyMorning),
            9..=15 => Some(Period::Daytime),
            16..=18 => Some(Period::Evening),
            19..=23 => Some(Period::Night),
            _ => None,
        }
    }
}

/// 天候。内部コードは1〜5（霧4・雪5の順に注意）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weather {
    Clear,
    Cloudy,
    Rain,
    Fog,
    Snow,
}

impl Weather {
    pub const ALL: [Weather; 5] = [
        Weather::Clear,
        Weather::Cloudy,
        Weather::Rain,
        Weather::Fog,
        Weather::Snow,
    ];

    pub const fn code(self) -> u8 {
        match self {
            Weather::Clear => 1,
            Weather::Cloudy => 2,
            Weather::Rain => 3,
            Weather::Fog => 4,
            Weather::Snow => 5,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|w| w.code() == code)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Weather::Clear => "晴",
            Weather::Cloudy => "曇",
            Weather::Rain => "雨",
            Weather::Fog => "霧",
            Weather::Snow => "雪",
        }
    }

    pub fn from_label(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|w| w.label() == s)
    }
}

/// 曜日区分。内部コードは平日0・休日1。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DayType {
    Weekday,
    Holiday,
}

impl DayType {
    pub const ALL: [DayType; 2] = [DayType::Weekday, DayType::Holiday];

    pub const fn code(self) -> u8 {
        match self {
            DayType::Weekday => 0,
            DayType::Holiday => 1,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.code() == code)
    }

    pub const fn label(self) -> &'static str {
        match self {
            DayType::Weekday => "平日",
            DayType::Holiday => "休日",
        }
    }

    pub fn from_label(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.label() == s)
    }
}

/// 正規化済みの集計条件。`None` は all（その条件で絞らない）を表す。
///
/// 天候は集合Bを減らさず、一致集計と倍率にだけ使う（behavior.md 7章）。
/// 条件の自動緩和はないので、エンジンはこの値を変えずに結果へ返す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ConditionFilter {
    pub period: Option<Period>,
    pub day_type: Option<DayType>,
    pub weather: Option<Weather>,
}
