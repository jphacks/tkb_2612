//! 原本1行の解釈（behavior.md 2章「DMS変換」「日時・コード・重大度」「移動手段」）。

use chrono::NaiveDate;

use crate::contract::geo::{LAT_RANGE, LON_RANGE};
use crate::contract::{Calendar, DayType, Mode, Period, Weather};
use crate::domain::rounding::fixed;

/// 必須列（全空白を除いた列名）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Column {
    Lat,
    Lon,
    Weather,
    Daynight,
    AccidentType,
    Deaths,
    Injuries,
    Year,
    Month,
    Day,
    Hour,
    Minute,
    PartyA,
    PartyB,
    InjuryA,
    InjuryB,
}

impl Column {
    pub const ALL: [Column; 16] = [
        Column::Lat,
        Column::Lon,
        Column::Weather,
        Column::Daynight,
        Column::AccidentType,
        Column::Deaths,
        Column::Injuries,
        Column::Year,
        Column::Month,
        Column::Day,
        Column::Hour,
        Column::Minute,
        Column::PartyA,
        Column::PartyB,
        Column::InjuryA,
        Column::InjuryB,
    ];

    /// 原本の列名から全空白を除いたもの。
    pub const fn name(self) -> &'static str {
        match self {
            Column::Lat => "地点緯度（北緯）",
            Column::Lon => "地点経度（東経）",
            Column::Weather => "天候",
            Column::Daynight => "昼夜",
            Column::AccidentType => "事故類型",
            Column::Deaths => "死者数",
            Column::Injuries => "負傷者数",
            Column::Year => "発生日時年",
            Column::Month => "発生日時月",
            Column::Day => "発生日時日",
            Column::Hour => "発生日時時",
            Column::Minute => "発生日時分",
            Column::PartyA => "当事者種別（当事者A）",
            Column::PartyB => "当事者種別（当事者B）",
            Column::InjuryA => "人身損傷程度（当事者A）",
            Column::InjuryB => "人身損傷程度（当事者B）",
        }
    }
}

/// Pythonの `"".join(value.split())` 相当。全角空白を含む全空白を除く。
pub fn normalize_header(name: &str) -> String {
    name.chars().filter(|c| !c.is_whitespace()).collect()
}

/// ヘッダの不正。前処理全体の失敗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderError {
    Missing(&'static str),
    Duplicate(&'static str),
}

impl std::fmt::Display for HeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeaderError::Missing(name) => write!(f, "必須列 {name} がありません"),
            HeaderError::Duplicate(name) => write!(f, "必須列 {name} が重複しています"),
        }
    }
}

/// 必須列の位置。列は名前で引き、位置を固定しない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnIndex([usize; Column::ALL.len()]);

impl ColumnIndex {
    pub fn from_header<S: AsRef<str>>(header: &[S]) -> Result<Self, HeaderError> {
        let normalized: Vec<String> = header
            .iter()
            .map(|h| normalize_header(h.as_ref()))
            .collect();
        let mut positions = [0; Column::ALL.len()];
        for (slot, column) in positions.iter_mut().zip(Column::ALL) {
            let mut found = normalized
                .iter()
                .enumerate()
                .filter(|(_, h)| h.as_str() == column.name());
            *slot = found.next().ok_or(HeaderError::Missing(column.name()))?.0;
            if found.next().is_some() {
                return Err(HeaderError::Duplicate(column.name()));
            }
        }
        Ok(Self(positions))
    }

    pub fn position(&self, column: Column) -> usize {
        let i = Column::ALL
            .iter()
            .position(|c| *c == column)
            .expect("ALLに含まれる");
        self.0[i]
    }
}

/// 除外理由。複数ある行は列挙順で最初の理由に1回だけ計上する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ExclusionReason {
    /// 必須列の値が行にない（列数が足りない）。
    MissingField,
    InvalidCoordinates,
    InvalidDatetime,
    InvalidCasualties,
    UnknownPartyCode,
    UnknownWeatherCode,
    UnknownInjuryCode,
    UnknownDaynightCode,
    InvalidAccidentType,
}

impl ExclusionReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            ExclusionReason::MissingField => "missing_field",
            ExclusionReason::InvalidCoordinates => "invalid_coordinates",
            ExclusionReason::InvalidDatetime => "invalid_datetime",
            ExclusionReason::InvalidCasualties => "invalid_casualties",
            ExclusionReason::UnknownPartyCode => "unknown_party_code",
            ExclusionReason::UnknownWeatherCode => "unknown_weather_code",
            ExclusionReason::UnknownInjuryCode => "unknown_injury_code",
            ExclusionReason::UnknownDaynightCode => "unknown_daynight_code",
            ExclusionReason::InvalidAccidentType => "invalid_accident_type",
        }
    }
}

/// 前後の空白を除いた後、ASCII数字1文字以上だけからなる値を10進整数として読む。
/// 符号・小数点・指数表記・全角数字・桁区切り・空文字は不正。
pub fn parse_csv_int(value: &str) -> Option<u64> {
    let v = value.trim();
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // 先頭0は可。u64を超える桁数は不正として扱う
    v.parse().ok()
}

/// DMS（度・分・ミリ秒角を連結した整数）から十進度へ。
/// `0 < D < max_degrees`、`M < 60`、`S_ms < 60,000` を満たさなければ `None`。
pub fn parse_dms(value: &str, max_degrees: u64) -> Option<f64> {
    let v = value.trim();
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u64 = v.parse().ok()?;
    let d = n / 10_000_000;
    let r = n % 10_000_000;
    let m = r / 100_000;
    let s_ms = r % 100_000;
    if !(0 < d && d < max_degrees && m < 60 && s_ms < 60_000) {
        return None;
    }
    Some(d as f64 + m as f64 / 60.0 + s_ms as f64 / 3_600_000.0)
}

/// 緯度・経度のDMSを十進度へ変換し、日本周辺の範囲（両端を含む）に限る。
pub fn parse_coordinates(lat: &str, lon: &str) -> Option<(f64, f64)> {
    let lat = parse_dms(lat, 90)?;
    let lon = parse_dms(lon, 180)?;
    (LAT_RANGE.contains(&lat) && LON_RANGE.contains(&lon)).then_some((lat, lon))
}

/// 当事者種別の基本許容集合。
pub const BASE_PARTY_CODES: [u16; 32] = [
    0, 1, 2, 3, 4, 5, 7, 11, 12, 13, 14, 17, 21, 22, 23, 24, 31, 32, 33, 34, 35, 36, 41, 42, 51,
    52, 59, 61, 71, 72, 75, 76,
];
/// 特定小型原付。2024/2025（source_yearで判定）だけ許容する。
pub const SMALL_MOPED_CODE: u16 = 43;
pub const PEDESTRIAN_CODE: u16 = 61;
pub const BICYCLE_CODES: [u16; 2] = [51, 52];
pub const CAR_CODES: [u16; 15] = [1, 2, 3, 4, 5, 7, 11, 12, 13, 14, 17, 21, 22, 23, 24];
pub const DAYNIGHT_CODES: [u16; 6] = [11, 12, 13, 21, 22, 23];
pub const INJURY_CODES: [u16; 4] = [0, 1, 2, 4];

/// source_yearで許容する当事者種別。
pub fn party_codes(source_year: u16) -> Vec<u16> {
    let mut codes = BASE_PARTY_CODES.to_vec();
    if matches!(source_year, 2024 | 2025) {
        codes.push(SMALL_MOPED_CODE);
        codes.sort_unstable();
    }
    codes
}

/// 当事者1人の種別から移動手段のマスク。二輪・原付・43等は対象外で0。
pub fn party_mode_mask(code: u16) -> u8 {
    if code == PEDESTRIAN_CODE {
        Mode::Walk.mask()
    } else if BICYCLE_CODES.contains(&code) {
        Mode::Bike.mask()
    } else if CAR_CODES.contains(&code) {
        Mode::Car.mask()
    } else {
        0
    }
}

/// 当事者A/Bの種別のOR。
pub fn modes_of(party_a: u16, party_b: u16) -> u8 {
    party_mode_mask(party_a) | party_mode_mask(party_b)
}

/// 保持する1事故。gzip CSVの17列に対応する。
#[derive(Debug, Clone, PartialEq)]
pub struct AccidentRecord {
    pub lat: f64,
    pub lon: f64,
    pub modes: u8,
    pub period: Period,
    pub weather: Weather,
    pub day_type: DayType,
    pub deaths: u16,
    pub injuries: u16,
    pub source_year: u16,
    pub date: NaiveDate,
    pub hour: u8,
    pub minute: u8,
    pub daynight: u16,
    pub accident_type: u64,
    pub injury_a: u16,
    pub injury_b: u16,
    /// 祝日表の収録範囲内の日付か（falseは土日と年末年始だけで判定した）。
    pub holiday_table_covered: bool,
}

/// 出力の列（17列、この順）。
pub const OUTPUT_COLUMNS: [&str; 17] = [
    "lat",
    "lon",
    "modes",
    "period",
    "weather",
    "day_type",
    "deaths",
    "injuries",
    "source_year",
    "date",
    "hour",
    "minute",
    "daynight",
    "accident_type",
    "injury_a",
    "injury_b",
    "severity",
];

impl AccidentRecord {
    /// 事故全体の重大度。死者1以上なら死亡。
    pub fn severity(&self) -> &'static str {
        if self.deaths > 0 { "死亡" } else { "負傷" }
    }

    /// YYYYMMDD。
    pub fn date_number(&self) -> u32 {
        use chrono::Datelike;
        self.date.year() as u32 * 10_000 + self.date.month() * 100 + self.date.day()
    }

    /// 出力の17列。緯度経度はここでだけ小数7桁に丸める。
    pub fn output_fields(&self) -> [String; 17] {
        [
            fixed(self.lat, 7),
            fixed(self.lon, 7),
            self.modes.to_string(),
            self.period.code().to_string(),
            self.weather.code().to_string(),
            self.day_type.code().to_string(),
            self.deaths.to_string(),
            self.injuries.to_string(),
            self.source_year.to_string(),
            self.date_number().to_string(),
            self.hour.to_string(),
            self.minute.to_string(),
            self.daynight.to_string(),
            self.accident_type.to_string(),
            self.injury_a.to_string(),
            self.injury_b.to_string(),
            self.severity().to_owned(),
        ]
    }
}

fn code_in(value: &str, allowed: &[u16]) -> Option<u16> {
    let code = u16::try_from(parse_csv_int(value)?).ok()?;
    allowed.contains(&code).then_some(code)
}

/// 原本1行を解釈する。`field` は必須列の値（前後の空白は未処理）を返す。
///
/// 理由の優先順：読込・必須列→座標→日時→人数→分類値（当事者種別→天候→人身損傷程度→
/// 昼夜→事故類型）。時間帯は発生時から、曜日区分は発生暦日から `Calendar` で決める。
pub fn convert_row<'a>(
    field: impl Fn(Column) -> Option<&'a str>,
    source_year: u16,
    calendar: &dyn Calendar,
) -> Result<AccidentRecord, ExclusionReason> {
    let mut values = [""; Column::ALL.len()];
    for (slot, column) in values.iter_mut().zip(Column::ALL) {
        *slot = field(column).ok_or(ExclusionReason::MissingField)?;
    }
    let get = |column: Column| {
        let i = Column::ALL
            .iter()
            .position(|c| *c == column)
            .expect("ALLに含まれる");
        values[i]
    };

    let (lat, lon) = parse_coordinates(get(Column::Lat), get(Column::Lon))
        .ok_or(ExclusionReason::InvalidCoordinates)?;

    let datetime = (|| {
        let year = u16::try_from(parse_csv_int(get(Column::Year))?).ok()?;
        let month = u32::try_from(parse_csv_int(get(Column::Month))?).ok()?;
        let day = u32::try_from(parse_csv_int(get(Column::Day))?).ok()?;
        let hour = u8::try_from(parse_csv_int(get(Column::Hour))?).ok()?;
        let minute = u8::try_from(parse_csv_int(get(Column::Minute))?).ok()?;
        if !(1900..=source_year).contains(&year) || hour > 23 || minute > 59 {
            return None;
        }
        let date = NaiveDate::from_ymd_opt(i32::from(year), month, day)?;
        Some((date, hour, minute))
    })();
    let (date, hour, minute) = datetime.ok_or(ExclusionReason::InvalidDatetime)?;

    let casualties = (|| {
        let deaths = u16::try_from(parse_csv_int(get(Column::Deaths))?).ok()?;
        let injuries = u16::try_from(parse_csv_int(get(Column::Injuries))?).ok()?;
        (deaths <= 999 && injuries <= 999 && deaths + injuries >= 1).then_some((deaths, injuries))
    })();
    let (deaths, injuries) = casualties.ok_or(ExclusionReason::InvalidCasualties)?;

    let parties = party_codes(source_year);
    let party_a =
        code_in(get(Column::PartyA), &parties).ok_or(ExclusionReason::UnknownPartyCode)?;
    let party_b =
        code_in(get(Column::PartyB), &parties).ok_or(ExclusionReason::UnknownPartyCode)?;
    let weather = parse_csv_int(get(Column::Weather))
        .and_then(|c| u8::try_from(c).ok())
        .and_then(Weather::from_code)
        .ok_or(ExclusionReason::UnknownWeatherCode)?;
    let injury_a =
        code_in(get(Column::InjuryA), &INJURY_CODES).ok_or(ExclusionReason::UnknownInjuryCode)?;
    let injury_b =
        code_in(get(Column::InjuryB), &INJURY_CODES).ok_or(ExclusionReason::UnknownInjuryCode)?;
    let daynight = code_in(get(Column::Daynight), &DAYNIGHT_CODES)
        .ok_or(ExclusionReason::UnknownDaynightCode)?;
    let accident_type =
        parse_csv_int(get(Column::AccidentType)).ok_or(ExclusionReason::InvalidAccidentType)?;

    let day = calendar.classify_date(date);
    Ok(AccidentRecord {
        lat,
        lon,
        modes: modes_of(party_a, party_b),
        period: Period::from_hour(hour).expect("時は0〜23に検査済み"),
        weather,
        day_type: day.day_type(),
        deaths,
        injuries,
        source_year,
        date,
        hour,
        minute,
        daynight,
        accident_type,
        injury_a,
        injury_b,
        holiday_table_covered: day.holiday_table_covered,
    })
}
