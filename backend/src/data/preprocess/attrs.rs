//! 事故の属性の取り出し（形式書12章、作業書M2a）。
//!
//! 既存の行処理（`convert_row`）と同じ保持判定を通った行から、割り当てに使う属性
//! （路線コード、道路形状、車道幅員、歩車道区分、地形）を、原本の列名（全空白を除いたもの）で
//! 引いて読む。保持判定は `scan_file` を共有するので、保持する行の集合と順番は
//! `prepare_release` と必ず同じになる。既存の必須列（`Column::ALL`）は変えない。
//!
//! コードの意味は年ごとのコード表（`accident/codebook_*`）で確かめた。2022〜2025年のXLSXは
//! [`AttrCodebooks`] が毎回照合する。2019〜2021年のPDFは、既存の方針（`FileCodebooks`）と同じく
//! 存在だけを確認し、内容は2024年のコード表との照合結果（形式書12章の表と同じ）を根拠にする。
//!
//! 欠損と未知：値が空（または行にその列がない）なら欠損、空でなくコード表にない値なら未知。
//! どちらも `.bin` では同じ値（`0xFF`／`0xFFFF`）になり、件数は別々に監査へ記録する。

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;
use serde_json::{Map, Value, json};

use crate::data::holidays::parse_holiday_csv;
use crate::data::load::read_metadata;
use crate::data::release::{
    ACCIDENTS_FILE, AccidentCsv, AccidentCsvWriter, HOLIDAYS_FILE, release_id, sha256_hex,
};
use crate::domain::HolidayCalendar;
use crate::domain::routing::accident::{
    AccidentAttr, AccidentAttrTable, CARRIAGEWAY_WIDTH_CODES, MISSING_U8, MISSING_U16,
    ROAD_SHAPE_CODES, RouteClass, SIDEWALK_DIVISION_CODES, TERRAIN_CODES,
};

use super::FileAudit;
use super::PreprocessError;
use super::RowSink;
use super::SOURCE_YEARS;
use super::codebook::{
    CodebookError, CodebookFile, CodebookVerifier, PDF_YEARS, XLSX_YEARS, read_sheet_rows,
};
use super::rows::{AccidentRecord, HeaderError, normalize_header, parse_csv_int};
use super::scan_file;
use super::source_file_name;

/// 属性の取り出し規則の版。コードの写し方や欠損・未知の扱いを変えたら上げる（`PARAMS` に記録する）。
pub const ATTRS_RULES_VERSION: &str = "jp-attrs-1";

/// 属性の列（原本の列名から全空白を除いたもの）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AttrColumn {
    Route,
    RoadShape,
    CarriagewayWidth,
    SidewalkDivision,
    Terrain,
}

impl AttrColumn {
    pub const ALL: [AttrColumn; 5] = [
        AttrColumn::Route,
        AttrColumn::RoadShape,
        AttrColumn::CarriagewayWidth,
        AttrColumn::SidewalkDivision,
        AttrColumn::Terrain,
    ];

    /// 原本の列名から全空白を除いたもの。
    pub const fn name(self) -> &'static str {
        match self {
            AttrColumn::Route => "路線コード",
            AttrColumn::RoadShape => "道路形状",
            AttrColumn::CarriagewayWidth => "車道幅員",
            AttrColumn::SidewalkDivision => "歩車道区分",
            AttrColumn::Terrain => "地形",
        }
    }

    /// 監査・`PARAMS` のキー。
    pub const fn key(self) -> &'static str {
        match self {
            AttrColumn::Route => "route",
            AttrColumn::RoadShape => "road_shape",
            AttrColumn::CarriagewayWidth => "carriageway_width",
            AttrColumn::SidewalkDivision => "sidewalk_division",
            AttrColumn::Terrain => "terrain",
        }
    }

    /// コード表のXLSXのシート名。
    pub const fn sheet(self) -> &'static str {
        match self {
            AttrColumn::Route => "路線",
            other => other.name(),
        }
    }
}

/// 属性の列の位置。列は名前で引き、位置を固定しない（2019〜2021年は58列、2022年以降は68列）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttrColumnIndex([usize; AttrColumn::ALL.len()]);

impl AttrColumnIndex {
    /// 必須列として扱う。列がない、または全空白を除いた名前が重複すれば失敗する
    /// （属性を読む前処理全体の失敗。`prepare_release` の失敗条件には影響しない）。
    pub fn from_header<S: AsRef<str>>(header: &[S]) -> Result<Self, HeaderError> {
        let normalized: Vec<String> = header
            .iter()
            .map(|h| normalize_header(h.as_ref()))
            .collect();
        let mut positions = [0; AttrColumn::ALL.len()];
        for (slot, column) in positions.iter_mut().zip(AttrColumn::ALL) {
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

    pub fn position(&self, column: AttrColumn) -> usize {
        self.0[column as usize]
    }
}

/// 原本1行の属性の値（前後の空白は未処理）。行に列がなければ `None`（欠損として扱う）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AttrFields<'a> {
    pub route: Option<&'a str>,
    pub road_shape: Option<&'a str>,
    pub carriageway_width: Option<&'a str>,
    pub sidewalk_division: Option<&'a str>,
    pub terrain: Option<&'a str>,
}

impl<'a> AttrFields<'a> {
    pub fn from_record(record: &'a csv::StringRecord, columns: &AttrColumnIndex) -> Self {
        let get = |column: AttrColumn| record.get(columns.position(column));
        Self {
            route: get(AttrColumn::Route),
            road_shape: get(AttrColumn::RoadShape),
            carriageway_width: get(AttrColumn::CarriagewayWidth),
            sidewalk_division: get(AttrColumn::SidewalkDivision),
            terrain: get(AttrColumn::Terrain),
        }
    }

    fn get(&self, column: AttrColumn) -> Option<&'a str> {
        match column {
            AttrColumn::Route => self.route,
            AttrColumn::RoadShape => self.road_shape,
            AttrColumn::CarriagewayWidth => self.carriageway_width,
            AttrColumn::SidewalkDivision => self.sidewalk_division,
            AttrColumn::Terrain => self.terrain,
        }
    }
}

/// 1つの属性の読み取り結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// コード表にある値。
    Valid,
    /// 値が空、または行にその列がない。
    Missing,
    /// 空でないが、コード表にない値。
    Unknown,
}

/// 1行の属性と、属性ごとの読み取り結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttrReading {
    pub attr: AccidentAttr,
    /// 路線コード全体（5桁の数字か）。
    pub route: Reading,
    /// 上4桁が `RouteClass` の表にあるか。`route` が `Valid` でなければ `route` と同じ。
    pub route_class: Reading,
    pub road_shape: Reading,
    pub carriageway_width: Reading,
    pub sidewalk_division: Reading,
    pub terrain: Reading,
    /// 車道幅員を、その年のコード表の値から2024年のコードへ写した場合の（元の値, 写した値）。
    pub width_remapped: Option<(u8, u8)>,
}

/// 2022年の車道幅員のコード表は、交差点の組合せを複数のコードで表す（`12,14`・`13,17`・`16,18`）。
/// 2024年のコード表の値（右）へ写す。ほかの年にはこれらのコードがない（あれば未知）。
pub const CARRIAGEWAY_WIDTH_ALIASES_2022: [(u8, u8); 3] = [(12, 14), (13, 17), (16, 18)];

/// その年のコード表で、2024年のコードへ写す対象の（元の値, 写した値）。
pub fn carriageway_width_aliases(source_year: u16) -> &'static [(u8, u8)] {
    if source_year == 2022 {
        &CARRIAGEWAY_WIDTH_ALIASES_2022
    } else {
        &[]
    }
}

/// 前後の空白を除き、空なら `None`。
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

/// 数値コードの属性。`accept` は数値をその年の表で2024年のコードに直す（表になければ `None`）。
fn read_code(value: Option<&str>, accept: impl FnMut(u8) -> Option<u8>) -> (u8, Reading) {
    let Some(value) = non_empty(value) else {
        return (MISSING_U8, Reading::Missing);
    };
    match parse_csv_int(value)
        .and_then(|n| u8::try_from(n).ok())
        .and_then(accept)
    {
        Some(code) => (code, Reading::Valid),
        None => (MISSING_U8, Reading::Unknown),
    }
}

fn code_in(codes: &'static [u8]) -> impl Fn(u8) -> Option<u8> {
    move |code| codes.contains(&code).then_some(code)
}

/// 路線コード（ASCII数字5桁：上4桁＋下1桁）。戻り値は（上4桁, 下1桁, route_class, route, route_class）。
fn read_route(value: Option<&str>) -> (u16, u8, u8, Reading, Reading) {
    let Some(value) = non_empty(value) else {
        return (
            MISSING_U16,
            MISSING_U8,
            MISSING_U8,
            Reading::Missing,
            Reading::Missing,
        );
    };
    // 先頭0を含む固定5桁（コード表は「上4桁」と「下1桁」）。桁が足りない値は読み違えを避けて未知にする
    if value.len() != 5 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return (
            MISSING_U16,
            MISSING_U8,
            MISSING_U8,
            Reading::Unknown,
            Reading::Unknown,
        );
    }
    let n: u32 = value.parse().expect("5桁の数字");
    let (upper, lower) = ((n / 10) as u16, (n % 10) as u8);
    match RouteClass::from_route_upper(upper) {
        Some(class) => (upper, lower, class.code(), Reading::Valid, Reading::Valid),
        // 上4桁と下1桁は読めたが、表にない区分（0000、9501〜9899、9901以上）
        None => (upper, lower, MISSING_U8, Reading::Valid, Reading::Unknown),
    }
}

/// 原本1行の属性を読む。`source_year` はファイルの年（コード表の年）。
pub fn read_attrs(fields: &AttrFields, source_year: u16) -> AttrReading {
    let (route_upper, route_lower, route_class, route, route_class_reading) =
        read_route(fields.route);
    let (road_shape, road_shape_reading) = read_code(fields.road_shape, code_in(&ROAD_SHAPE_CODES));
    let aliases = carriageway_width_aliases(source_year);
    let mut width_remapped = None;
    let (carriageway_width, carriageway_width_reading) =
        read_code(fields.carriageway_width, |raw| {
            let code = aliases
                .iter()
                .find(|(from, _)| *from == raw)
                .map_or(raw, |(_, to)| *to);
            CARRIAGEWAY_WIDTH_CODES.contains(&code).then(|| {
                if code != raw {
                    width_remapped = Some((raw, code));
                }
                code
            })
        });
    let (sidewalk_division, sidewalk_division_reading) =
        read_code(fields.sidewalk_division, code_in(&SIDEWALK_DIVISION_CODES));
    let (terrain, terrain_reading) = read_code(fields.terrain, code_in(&TERRAIN_CODES));
    AttrReading {
        attr: AccidentAttr {
            route_upper,
            route_lower,
            route_class,
            road_shape,
            carriageway_width,
            sidewalk_division,
            terrain,
        },
        route,
        route_class: route_class_reading,
        road_shape: road_shape_reading,
        carriageway_width: carriageway_width_reading,
        sidewalk_division: sidewalk_division_reading,
        terrain: terrain_reading,
        width_remapped,
    }
}

/// 未知の値を記録する異なる値の数の上限（超えた分は `(その他)` にまとめる）。
const UNKNOWN_VALUES_LIMIT: usize = 50;
/// 記録する未知の値の最大文字数。
const UNKNOWN_VALUE_CHARS: usize = 16;

/// 1年度（または全期間）の属性の分布と、欠損・未知の件数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttrAudit {
    /// 属性を読んだ行数（保持した行数）。
    pub rows: u64,
    /// 属性キー→欠損の件数。
    pub missing: BTreeMap<&'static str, u64>,
    /// 属性キー（と `route_class`）→未知の件数。
    pub unknown: BTreeMap<&'static str, u64>,
    /// 属性キー（`route_class` を含む）→コード→件数。コード表にある値だけ。
    pub distribution: BTreeMap<&'static str, BTreeMap<u8, u64>>,
    /// 路線コードの下1桁が1〜9（バイパス）の行数。
    pub route_lower_bypass: u64,
    /// 2024年のコードへ写した（元の値, 写した値）→件数。
    pub width_remapped: BTreeMap<(u8, u8), u64>,
    /// 属性キー（と `route_class`＝上4桁）→未知の値（文字列）→件数。
    pub unknown_values: BTreeMap<&'static str, BTreeMap<String, u64>>,
}

/// 欠損・未知を数える属性のキー（`route_class` は路線コードの上4桁が表にない場合）。
const UNKNOWN_KEYS: [&str; 6] = [
    "route",
    "route_class",
    "road_shape",
    "carriageway_width",
    "sidewalk_division",
    "terrain",
];

/// 分布を数える属性のキー。
const DISTRIBUTION_KEYS: [&str; 5] = [
    "route_class",
    "road_shape",
    "carriageway_width",
    "sidewalk_division",
    "terrain",
];

impl Default for AttrAudit {
    fn default() -> Self {
        Self::new()
    }
}

impl AttrAudit {
    pub fn new() -> Self {
        Self {
            rows: 0,
            missing: BTreeMap::new(),
            unknown: BTreeMap::new(),
            distribution: BTreeMap::new(),
            route_lower_bypass: 0,
            width_remapped: BTreeMap::new(),
            unknown_values: BTreeMap::new(),
        }
    }

    fn note_unknown(&mut self, key: &'static str, raw: &str) {
        *self.unknown.entry(key).or_default() += 1;
        let values = self.unknown_values.entry(key).or_default();
        let value: String = raw.trim().chars().take(UNKNOWN_VALUE_CHARS).collect();
        let value = if values.contains_key(&value) || values.len() < UNKNOWN_VALUES_LIMIT {
            value
        } else {
            "(その他)".to_owned()
        };
        *values.entry(value).or_default() += 1;
    }

    /// 保持した1行の読み取り結果を数える。
    pub fn record(&mut self, fields: &AttrFields, reading: &AttrReading) {
        self.rows += 1;
        for (column, result) in [
            (AttrColumn::Route, reading.route),
            (AttrColumn::RoadShape, reading.road_shape),
            (AttrColumn::CarriagewayWidth, reading.carriageway_width),
            (AttrColumn::SidewalkDivision, reading.sidewalk_division),
            (AttrColumn::Terrain, reading.terrain),
        ] {
            match result {
                Reading::Valid => {}
                Reading::Missing => *self.missing.entry(column.key()).or_default() += 1,
                Reading::Unknown => {
                    self.note_unknown(column.key(), fields.get(column).unwrap_or(""));
                }
            }
        }
        let attr = &reading.attr;
        if reading.route == Reading::Valid && reading.route_class == Reading::Unknown {
            // 上4桁が表にない。値（上4桁）も残す
            *self.unknown.entry("route_class").or_default() += 1;
            let values = self.unknown_values.entry("route_class").or_default();
            let key = format!("{:04}", attr.route_upper);
            if values.contains_key(&key) || values.len() < UNKNOWN_VALUES_LIMIT {
                *values.entry(key).or_default() += 1;
            } else {
                *values.entry("(その他)".to_owned()).or_default() += 1;
            }
        }
        if reading.route_class == Reading::Valid {
            self.count("route_class", attr.route_class);
        }
        for (key, result, code) in [
            ("road_shape", reading.road_shape, attr.road_shape),
            (
                "carriageway_width",
                reading.carriageway_width,
                attr.carriageway_width,
            ),
            (
                "sidewalk_division",
                reading.sidewalk_division,
                attr.sidewalk_division,
            ),
            ("terrain", reading.terrain, attr.terrain),
        ] {
            if result == Reading::Valid {
                self.count(key, code);
            }
        }
        if (1..=9).contains(&attr.route_lower) {
            self.route_lower_bypass += 1;
        }
        if let Some(pair) = reading.width_remapped {
            *self.width_remapped.entry(pair).or_default() += 1;
        }
    }

    fn count(&mut self, key: &'static str, code: u8) {
        *self
            .distribution
            .entry(key)
            .or_default()
            .entry(code)
            .or_default() += 1;
    }

    /// 年ごとの監査を足し合わせる（全期間の合計）。
    pub fn merge(&mut self, other: &AttrAudit) {
        self.rows += other.rows;
        for (key, n) in &other.missing {
            *self.missing.entry(*key).or_default() += n;
        }
        for (key, n) in &other.unknown {
            *self.unknown.entry(*key).or_default() += n;
        }
        for (key, codes) in &other.distribution {
            let mine = self.distribution.entry(*key).or_default();
            for (code, n) in codes {
                *mine.entry(*code).or_default() += n;
            }
        }
        self.route_lower_bypass += other.route_lower_bypass;
        for (pair, n) in &other.width_remapped {
            *self.width_remapped.entry(*pair).or_default() += n;
        }
        for (key, values) in &other.unknown_values {
            let mine = self.unknown_values.entry(*key).or_default();
            for (value, n) in values {
                *mine.entry(value.clone()).or_default() += n;
            }
        }
    }

    /// 分布の件数（コードなし＝その属性が有効な行数の合計ではなく、そのコードの件数）。
    pub fn distribution_count(&self, key: &str, code: u8) -> u64 {
        self.distribution
            .get(key)
            .and_then(|codes| codes.get(&code))
            .copied()
            .unwrap_or(0)
    }

    /// 監査のJSON。欠損・未知の件数は0でもすべてのキーを書く。
    pub fn to_json(&self) -> Value {
        let missing: Map<String, Value> = AttrColumn::ALL
            .iter()
            .map(|c| {
                (
                    c.key().to_owned(),
                    json!(self.missing.get(c.key()).copied().unwrap_or(0)),
                )
            })
            .collect();
        let unknown: Map<String, Value> = UNKNOWN_KEYS
            .iter()
            .map(|k| {
                (
                    (*k).to_owned(),
                    json!(self.unknown.get(k).copied().unwrap_or(0)),
                )
            })
            .collect();
        let distribution: Map<String, Value> = DISTRIBUTION_KEYS
            .iter()
            .map(|k| {
                let codes: Map<String, Value> = self
                    .distribution
                    .get(k)
                    .into_iter()
                    .flatten()
                    .map(|(code, n)| (code.to_string(), json!(n)))
                    .collect();
                ((*k).to_owned(), Value::Object(codes))
            })
            .collect();
        let width_remapped: Map<String, Value> = self
            .width_remapped
            .iter()
            .map(|((from, to), n)| (format!("{from}->{to}"), json!(n)))
            .collect();
        let unknown_values: Map<String, Value> = self
            .unknown_values
            .iter()
            .map(|(key, values)| {
                let values: Map<String, Value> =
                    values.iter().map(|(v, n)| (v.clone(), json!(n))).collect();
                ((*key).to_owned(), Value::Object(values))
            })
            .collect();
        json!({
            "rows": self.rows,
            "missing": missing,
            "unknown": unknown,
            "distribution": distribution,
            "route_lower_bypass": self.route_lower_bypass,
            "width_remapped": width_remapped,
            "unknown_values": unknown_values,
        })
    }
}

/// 1年度の結果。
#[derive(Debug, Clone, PartialEq)]
pub struct YearAttrs {
    pub source_year: u16,
    /// 既存の行処理と同じ監査（入力・保持・除外の行数）。
    pub file: FileAudit,
    /// ヘッダの列数。
    pub header_columns: usize,
    /// 属性の列名（全空白を除いたもの）→ヘッダ内の位置（0始まり）。
    pub column_positions: BTreeMap<&'static str, usize>,
    pub audit: AttrAudit,
}

/// 保持した行を、17列のgzip CSV（照合用）と属性の表へ同時に書く。
struct AttrsSink<'a> {
    source_year: u16,
    writer: &'a mut AccidentCsvWriter,
    attrs: &'a mut Vec<AccidentAttr>,
    columns: Option<AttrColumnIndex>,
    header_columns: usize,
    audit: AttrAudit,
}

impl RowSink for AttrsSink<'_> {
    fn begin(&mut self, header: &[String]) -> Result<(), String> {
        self.columns = Some(AttrColumnIndex::from_header(header).map_err(|e| e.to_string())?);
        self.header_columns = header.len();
        Ok(())
    }

    fn retained(&mut self, accident: &AccidentRecord, record: &csv::StringRecord) {
        self.writer.push(accident);
        let columns = self.columns.as_ref().expect("beginが先に呼ばれる");
        let fields = AttrFields::from_record(record, columns);
        let reading = read_attrs(&fields, self.source_year);
        self.audit.record(&fields, &reading);
        self.attrs.push(reading.attr);
    }
}

/// 7年度の原本を走査した結果。
#[derive(Debug, Clone, PartialEq)]
pub struct AttrScan {
    /// 保持行から作り直した17列のgzip CSV。既存の `prepare_release` と同じバイト列になる。
    pub accidents: AccidentCsv,
    /// 事故ID（保持行の通し番号）の順の属性。
    pub attrs: Vec<AccidentAttr>,
    pub years: Vec<YearAttrs>,
}

/// 原本ディレクトリの7年度を `prepare_release` と同じ順・同じ保持判定で走査し、属性を読む。
///
/// 失敗条件は `prepare_release` と同じ（ファイル欠損・文字コード・CSV構文・必須列・全行除外）。
/// これに加えて、属性の列の欠損・重複も失敗にする。コード表の照合は呼び出し側で行う。
pub fn scan_accident_attrs(
    accident_dir: &Path,
    calendar: &HolidayCalendar,
) -> Result<AttrScan, PreprocessError> {
    let mut writer = AccidentCsvWriter::new();
    let mut attrs = Vec::new();
    let mut years = Vec::new();
    for source_year in SOURCE_YEARS {
        let file = source_file_name(source_year);
        let bytes =
            std::fs::read(accident_dir.join(&file)).map_err(|error| PreprocessError::Io {
                file: file.clone(),
                error,
            })?;
        let start = attrs.len();
        let mut sink = AttrsSink {
            source_year,
            writer: &mut writer,
            attrs: &mut attrs,
            columns: None,
            header_columns: 0,
            audit: AttrAudit::new(),
        };
        let audit = scan_file(&file, &bytes, source_year, calendar, &mut sink)?;
        let AttrsSink {
            columns,
            header_columns,
            audit: attr_audit,
            ..
        } = sink;
        if audit.retained_rows == 0 {
            return Err(PreprocessError::NoRetainedRows { file });
        }
        debug_assert_eq!((attrs.len() - start) as u64, audit.retained_rows);
        let columns = columns.expect("beginで作る");
        years.push(YearAttrs {
            source_year,
            file: audit,
            header_columns,
            column_positions: AttrColumn::ALL
                .iter()
                .map(|c| (c.name(), columns.position(*c)))
                .collect(),
            audit: attr_audit,
        });
    }
    Ok(AttrScan {
        accidents: writer.finish(),
        attrs,
        years,
    })
}

// ---- コード表（XLSX）の照合 ----

/// シート名→（B列, C列）。
pub type AttrSheets = BTreeMap<String, Vec<(String, String)>>;

fn digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// B列が数字だけの行のコード。
fn numeric_codes(rows: &[(String, String)]) -> BTreeSet<u8> {
    rows.iter()
        .filter(|(b, _)| digits(b))
        .filter_map(|(b, _)| b.parse().ok())
        .collect()
}

/// その年の車道幅員のコード表の、同じ区分を表すコードの組（2022年は `[12, 14]` など）。
fn expected_width_groups(source_year: u16) -> BTreeSet<Vec<u8>> {
    let aliases = carriageway_width_aliases(source_year);
    CARRIAGEWAY_WIDTH_CODES
        .iter()
        .map(|&code| {
            let mut group: Vec<u8> = aliases
                .iter()
                .filter(|(_, to)| *to == code)
                .map(|(from, _)| *from)
                .collect();
            group.push(code);
            group.sort_unstable();
            group
        })
        .collect()
}

/// B列が `12,14` のようなコードの列の行を、コードの組にする。
fn width_groups(rows: &[(String, String)]) -> BTreeSet<Vec<u8>> {
    rows.iter()
        .filter_map(|(b, _)| {
            let parts: Vec<&str> = b.split(',').map(str::trim).collect();
            if !parts.iter().all(|p| digits(p)) {
                return None;
            }
            let mut group: Vec<u8> = parts
                .iter()
                .map(|p| p.parse().ok())
                .collect::<Option<_>>()?;
            group.sort_unstable();
            Some(group)
        })
        .collect()
}

/// B列が `0001～0999`（または `9500` のような1つの値）の行を、上4桁の範囲にする。
fn route_ranges(rows: &[(String, String)]) -> Vec<(u16, u16)> {
    rows.iter()
        .filter_map(|(b, _)| {
            let (lo, hi) = match b.split_once(['～', '〜']) {
                Some((lo, hi)) => (lo.trim(), hi.trim()),
                None => (b.as_str(), b.as_str()),
            };
            (lo.len() == 4 && hi.len() == 4 && digits(lo) && digits(hi))
                .then(|| Some((lo.parse().ok()?, hi.parse().ok()?)))
                .flatten()
        })
        .collect()
}

/// 1年度のコード表（XLSXの5シート）が、このコードの表（2024年のコード表と、年ごとの写し方）と
/// 一致するか確かめる。
pub fn verify_attr_sheets(source_year: u16, sheets: &AttrSheets) -> Result<(), CodebookError> {
    let sheet = |column: AttrColumn| -> Result<&[(String, String)], CodebookError> {
        sheets
            .get(column.sheet())
            .map(Vec::as_slice)
            .ok_or_else(|| {
                CodebookError(format!(
                    "{source_year}年 シート {} がありません",
                    column.sheet()
                ))
            })
    };
    let mismatch = |name: &str, detail: String| {
        CodebookError(format!(
            "{source_year}年 {name} のコード表が一致しません（{detail}）"
        ))
    };
    let expected_ranges: Vec<(u16, u16)> =
        RouteClass::ALL.iter().map(|c| c.upper_range()).collect();
    let actual_ranges = route_ranges(sheet(AttrColumn::Route)?);
    if actual_ranges != expected_ranges {
        return Err(mismatch(
            AttrColumn::Route.sheet(),
            format!("期待 {expected_ranges:?}、実際 {actual_ranges:?}"),
        ));
    }
    for (column, codes) in [
        (AttrColumn::RoadShape, &ROAD_SHAPE_CODES[..]),
        (AttrColumn::SidewalkDivision, &SIDEWALK_DIVISION_CODES[..]),
        (AttrColumn::Terrain, &TERRAIN_CODES[..]),
    ] {
        let expected: BTreeSet<u8> = codes.iter().copied().collect();
        let actual = numeric_codes(sheet(column)?);
        if actual != expected {
            return Err(mismatch(
                column.sheet(),
                format!("期待 {expected:?}、実際 {actual:?}"),
            ));
        }
    }
    let expected = expected_width_groups(source_year);
    let actual = width_groups(sheet(AttrColumn::CarriagewayWidth)?);
    if actual != expected {
        return Err(mismatch(
            AttrColumn::CarriagewayWidth.sheet(),
            format!("期待 {expected:?}、実際 {actual:?}"),
        ));
    }
    Ok(())
}

/// `accident/` のコード表で、属性のコードを照合する。2022〜2025年のXLSXは5シートの内容を毎回照合する。
/// 2019〜2021年のPDFは存在だけを確認する（既存の `FileCodebooks` と同じ方針）。
#[derive(Debug, Clone, Copy, Default)]
pub struct AttrCodebooks;

impl CodebookVerifier for AttrCodebooks {
    fn verify(
        &self,
        accident_dir: &Path,
    ) -> Result<(Vec<CodebookFile>, Vec<CodebookFile>), CodebookError> {
        let read = |file: &str| {
            std::fs::read(accident_dir.join(file))
                .map_err(|e| CodebookError(format!("{file} を読めません: {e}")))
        };
        let mut pdfs = Vec::new();
        for year in PDF_YEARS {
            let file = format!("codebook_{year}.pdf");
            let bytes = read(&file)?;
            pdfs.push(CodebookFile {
                source_year: year,
                sha256: sha256_hex(&bytes),
                file,
            });
        }
        let sheet_names: Vec<&str> = AttrColumn::ALL.iter().map(|c| c.sheet()).collect();
        let mut xlsxs = Vec::new();
        for year in XLSX_YEARS {
            let file = format!("codebook_{year}.xlsx");
            let bytes = read(&file)?;
            let sheets = read_sheet_rows(&bytes, &sheet_names)?;
            verify_attr_sheets(year, &sheets)?;
            xlsxs.push(CodebookFile {
                source_year: year,
                sha256: sha256_hex(&bytes),
                file,
            });
        }
        Ok((xlsxs, pdfs))
    }
}

// ---- accident_attrs.bin の組み立て ----

/// `accident-attrs` の失敗。
#[derive(Debug)]
pub enum AttrsError {
    /// 原本の走査の失敗（`prepare_release` と同じ条件）。
    Preprocess(PreprocessError),
    Codebook(CodebookError),
    /// 入力のschema 2のreleaseが読めない、または内部で食い違っている。
    Release(String),
    /// 原本から作り直した事故CSVが、releaseの `accidents.csv.gz` と一致しない（事故IDを保証できない）。
    Mismatch(String),
}

impl std::fmt::Display for AttrsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttrsError::Preprocess(e) => write!(f, "{e}"),
            AttrsError::Codebook(e) => write!(f, "{e}"),
            AttrsError::Release(m) => write!(f, "入力のreleaseが不正です: {m}"),
            AttrsError::Mismatch(m) => write!(f, "原本とreleaseが一致しません: {m}"),
        }
    }
}

impl std::error::Error for AttrsError {}

impl From<PreprocessError> for AttrsError {
    fn from(value: PreprocessError) -> Self {
        AttrsError::Preprocess(value)
    }
}

impl From<CodebookError> for AttrsError {
    fn from(value: CodebookError) -> Self {
        AttrsError::Codebook(value)
    }
}

/// `accident-attrs` の出力。
#[derive(Debug, Clone, PartialEq)]
pub struct AccidentAttrsOutput {
    /// `accident_attrs.bin` に書く内容（`PARAMS` を含む）。
    pub table: AccidentAttrTable,
    /// `DEPS`：`accidents.csv.gz`→SHA-256。
    pub deps: BTreeMap<String, String>,
    /// 監査のJSON（`PARAMS` に、計画書4.2節の表との照合を足したもの）。
    pub audit: Value,
    /// 入力の `accidents.csv.gz` の `dataset_id`。
    pub dataset_id: String,
    /// 原本から作り直したgzipが、releaseの `accidents.csv.gz` とバイト列まで同じか。
    pub gzip_identical: bool,
}

fn release_error(e: impl std::fmt::Display) -> AttrsError {
    AttrsError::Release(e.to_string())
}

fn metadata_str<'a>(metadata: &'a Value, path: &[&str]) -> Result<&'a str, AttrsError> {
    let mut v = metadata;
    for key in path {
        v = v.get(key).ok_or_else(|| {
            AttrsError::Release(format!("metadataに{}がありません", path.join(".")))
        })?;
    }
    v.as_str().ok_or_else(|| {
        AttrsError::Release(format!(
            "metadataの{}が文字列ではありません",
            path.join(".")
        ))
    })
}

/// 2つの読み取り元の内容が同じか（全体をメモリに置かずに比べる）。
fn same_content(mut a: impl Read, mut b: impl Read) -> std::io::Result<bool> {
    let (mut buf_a, mut buf_b) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let n = a.read(&mut buf_a)?;
        if n == 0 {
            return Ok(b.read(&mut buf_b)? == 0);
        }
        if b.read_exact(&mut buf_b[..n]).is_err() || buf_a[..n] != buf_b[..n] {
            return Ok(false);
        }
    }
}

/// 計画書4.2節の表（2024年の実測）。（名前, 該当するコード, 割合%）。
const PLAN_ROUTE_CLASS: [(&str, &[u8], f64); 5] = [
    ("市町村道", &[4], 44.3),
    ("国道", &[0], 20.7),
    ("主要地方道", &[1, 2], 16.8),
    ("一般県道", &[3], 10.8),
    ("高速・自専道", &[5, 6, 7], 2.2),
];
const PLAN_ROAD_SHAPE: [(&str, &[u8], f64); 4] = [
    ("交差点", &[1, 31], 43.4),
    ("交差点付近", &[7, 37], 14.8),
    ("単路", &[11, 12, 13, 14], 36.9),
    ("一般交通の場所", &[0], 4.9),
];
/// 計画書4.2節の表との差の許容（ポイント）。
const PLAN_TOLERANCE_POINTS: f64 = 1.0;

/// 2024年の分布を計画書4.2節の表と照合する。割合は、2024年の保持行すべてに対する割合。
fn plan_check(audit: &AttrAudit) -> Value {
    let rows = audit.rows;
    let section = |key: &str, table: &[(&str, &[u8], f64)]| -> Value {
        let items: Vec<Value> = table
            .iter()
            .map(|(name, codes, plan)| {
                let count: u64 = codes
                    .iter()
                    .map(|c| audit.distribution_count(key, *c))
                    .sum();
                let measured = if rows == 0 {
                    0.0
                } else {
                    100.0 * count as f64 / rows as f64
                };
                json!({
                    "name": name,
                    "codes": codes,
                    "count": count,
                    "plan_percent": plan,
                    "measured_percent": (measured * 1000.0).round() / 1000.0,
                    "diff_points": ((measured - plan) * 1000.0).round() / 1000.0,
                    "within_tolerance": (measured - plan).abs() <= PLAN_TOLERANCE_POINTS,
                })
            })
            .collect();
        Value::Array(items)
    };
    json!({
        "source_year": 2024,
        "denominator": "2024年の保持行すべて",
        "rows": rows,
        "tolerance_points": PLAN_TOLERANCE_POINTS,
        "route_class": section("route_class", &PLAN_ROUTE_CLASS),
        "road_shape": section("road_shape", &PLAN_ROAD_SHAPE),
    })
}

/// 各コードの意味（`PARAMS` の `legend`）。
fn legend() -> Value {
    json!({
        "route_class": [
            "一般国道", "主要地方道－都道府県道", "主要地方道－市道", "一般都道府県道",
            "一般市町村道", "高速自動車国道", "自動車専用道－指定", "自動車専用道－その他",
            "道路運送法上の道路", "農（免）道", "林道", "港湾道", "私道", "その他",
            "一般の交通の用に供するその他の道路"
        ],
        "road_shape": {
            "0": "一般交通の場所", "1": "交差点－その他", "7": "交差点付近－その他",
            "11": "単路－トンネル", "12": "単路－橋", "13": "単路－カーブ・屈折",
            "14": "単路－その他", "21": "踏切－第一種", "22": "踏切－第三種",
            "23": "踏切－第四種", "31": "交差点－環状交差点", "37": "交差点付近－環状交差点付近"
        },
        "carriageway_width": {
            "0": "一般交通の場所", "1": "単路－3.5m未満", "2": "単路－3.5m以上", "3": "単路－5.5m以上",
            "4": "単路－9.0m以上", "5": "単路－13.0m以上", "6": "単路－19.5m以上",
            "11": "交差点－小－小", "14": "交差点－中－小", "15": "交差点－中－中",
            "17": "交差点－大－小", "18": "交差点－大－中", "19": "交差点－大－大"
        },
        "sidewalk_division": {
            "1": "区分あり－防護柵等", "2": "区分あり－縁石・ブロック等",
            "3": "区分あり－路側帯", "4": "区分なし"
        },
        "terrain": { "1": "市街地－人口集中", "2": "市街地－その他", "3": "非市街地" },
    })
}

fn codebook_json(list: &[CodebookFile]) -> Value {
    list.iter()
        .map(|c| json!({ "source_year": c.source_year, "file": c.file, "sha256": c.sha256 }))
        .collect()
}

/// schema 2のrelease（`base_release_dir`）の事故IDに対応する属性を、原本から作る。
///
/// 1. releaseの `metadata.json`・`accidents.csv.gz`・`syukujitsu.csv` を読み、ハッシュと識別子を確かめる。
/// 2. コード表を照合する（`verifier`）。
/// 3. 原本を `prepare_release` と同じ保持判定で走査し、17列の事故CSVを作り直して、
///    releaseの `accidents.csv.gz` と照合する。バイト列まで同じなら `dataset_id` が一致する。
///    gzipの圧縮結果だけが違う場合は、展開した内容が同じなら進み、`gzip_identical` を偽にする。
///    内容が違えば失敗する（事故IDの行番号を保証できない）。
pub fn build_accident_attrs(
    accident_dir: &Path,
    base_release_dir: &Path,
    verifier: &dyn CodebookVerifier,
) -> Result<AccidentAttrsOutput, AttrsError> {
    let metadata = read_metadata(base_release_dir).map_err(release_error)?;
    let read = |file: &str| {
        std::fs::read(base_release_dir.join(file))
            .map_err(|e| AttrsError::Release(format!("{file} を読めません: {e}")))
    };
    let release_gzip = read(ACCIDENTS_FILE)?;
    let holidays = read(HOLIDAYS_FILE)?;
    let dataset_id = sha256_hex(&release_gzip);
    let holiday_sha256 = sha256_hex(&holidays);
    if metadata_str(&metadata, &["dataset_id"])? != dataset_id {
        return Err(AttrsError::Release(
            "dataset_idがaccidents.csv.gzと一致しません".to_owned(),
        ));
    }
    if metadata_str(&metadata, &["holiday_table", "sha256"])? != holiday_sha256 {
        return Err(AttrsError::Release(
            "祝日表のハッシュがmetadataと一致しません".to_owned(),
        ));
    }
    let base_release_id = release_id(&dataset_id, &holiday_sha256);
    if metadata_str(&metadata, &["release_id"])? != base_release_id {
        return Err(AttrsError::Release(
            "release_idが実ファイルと一致しません".to_owned(),
        ));
    }
    let calendar = parse_holiday_csv(&holidays).map_err(release_error)?;

    let (codebooks_xlsx, codebooks_pdf) = verifier.verify(accident_dir)?;
    let scan = scan_accident_attrs(accident_dir, &calendar)?;

    let rows = scan.accidents.rows;
    if metadata.get("rows").and_then(Value::as_u64) != Some(rows) {
        return Err(AttrsError::Mismatch(format!(
            "原本から数えた保持行数{rows}がmetadataのrowsと一致しません"
        )));
    }
    let gzip_identical = sha256_hex(&scan.accidents.gzip) == dataset_id;
    if !gzip_identical {
        let same = same_content(
            GzDecoder::new(scan.accidents.gzip.as_slice()),
            GzDecoder::new(release_gzip.as_slice()),
        )
        .map_err(|e| AttrsError::Mismatch(format!("accidents.csv.gzを展開できません: {e}")))?;
        if !same {
            return Err(AttrsError::Mismatch(
                "原本から作り直した事故CSVが、releaseのaccidents.csv.gzと一致しません".to_owned(),
            ));
        }
    }
    if scan.attrs.len() as u64 != rows {
        return Err(AttrsError::Mismatch(
            "属性の件数が保持行数と一致しません".to_owned(),
        ));
    }

    let mut total = AttrAudit::new();
    let mut years = Map::new();
    for year in &scan.years {
        total.merge(&year.audit);
        let codebook = if XLSX_YEARS.contains(&year.source_year) {
            json!({ "check": "xlsx", "file": format!("codebook_{}.xlsx", year.source_year) })
        } else {
            json!({ "check": "pdf_exists_only", "file": format!("codebook_{}.pdf", year.source_year) })
        };
        let aliases: Map<String, Value> = carriageway_width_aliases(year.source_year)
            .iter()
            .map(|(from, to)| (from.to_string(), json!(to)))
            .collect();
        let positions: Map<String, Value> = year
            .column_positions
            .iter()
            .map(|(name, position)| ((*name).to_owned(), json!(position)))
            .collect();
        let mut entry = year.audit.to_json();
        let object = entry.as_object_mut().expect("objectで作った");
        object.insert("file".to_owned(), json!(year.file.file));
        object.insert("sha256".to_owned(), json!(year.file.sha256));
        object.insert("encoding".to_owned(), json!(year.file.encoding));
        object.insert("header_columns".to_owned(), json!(year.header_columns));
        object.insert("column_positions".to_owned(), Value::Object(positions));
        object.insert("input_rows".to_owned(), json!(year.file.input_rows));
        object.insert("retained_rows".to_owned(), json!(year.file.retained_rows));
        object.insert("excluded_rows".to_owned(), json!(year.file.excluded_rows));
        object.insert("codebook".to_owned(), codebook);
        object.insert(
            "carriageway_width_aliases".to_owned(),
            Value::Object(aliases),
        );
        years.insert(year.source_year.to_string(), entry);
    }
    let columns: Map<String, Value> = AttrColumn::ALL
        .iter()
        .map(|c| (c.key().to_owned(), json!(c.name())))
        .collect();
    let params = json!({
        "rules_version": ATTRS_RULES_VERSION,
        "rows": rows,
        "base_release": {
            "schema_version": metadata.get("schema_version"),
            "release_id": base_release_id,
            "dataset_id": dataset_id,
            "rows": rows,
        },
        "columns": columns,
        "code_tables": {
            "road_shape": ROAD_SHAPE_CODES,
            "carriageway_width": CARRIAGEWAY_WIDTH_CODES,
            "sidewalk_division": SIDEWALK_DIVISION_CODES,
            "terrain": TERRAIN_CODES,
            "carriageway_width_aliases": {
                "2022": CARRIAGEWAY_WIDTH_ALIASES_2022
                    .iter()
                    .map(|(from, to)| (from.to_string(), json!(to)))
                    .collect::<Map<String, Value>>(),
            },
            "route_code_digits": 5,
        },
        "legend": legend(),
        "codebooks": {
            "xlsx": codebook_json(&codebooks_xlsx),
            "pdf": codebook_json(&codebooks_pdf),
        },
        "years": years,
        "totals": total.to_json(),
    });
    let mut audit = params.clone();
    let year_2024 = scan
        .years
        .iter()
        .find(|y| y.source_year == 2024)
        .map(|y| plan_check(&y.audit));
    audit.as_object_mut().expect("objectで作った").insert(
        "plan_4_2_check".to_owned(),
        year_2024.unwrap_or(Value::Null),
    );
    let mut deps = BTreeMap::new();
    deps.insert(ACCIDENTS_FILE.to_owned(), dataset_id.clone());
    Ok(AccidentAttrsOutput {
        table: AccidentAttrTable {
            params,
            attrs: scan.attrs,
        },
        deps,
        audit,
        dataset_id,
        gzip_identical,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields<'a>(
        route: &'a str,
        shape: &'a str,
        width: &'a str,
        sidewalk: &'a str,
        terrain: &'a str,
    ) -> AttrFields<'a> {
        AttrFields {
            route: Some(route),
            road_shape: Some(shape),
            carriageway_width: Some(width),
            sidewalk_division: Some(sidewalk),
            terrain: Some(terrain),
        }
    }

    #[test]
    fn reads_a_normal_row() {
        let r = read_attrs(&fields("30000", "01", "15", "2", "1"), 2024);
        assert_eq!(
            r.attr,
            AccidentAttr {
                route_upper: 3000,
                route_lower: 0,
                route_class: RouteClass::MunicipalRoad.code(),
                road_shape: 1,
                carriageway_width: 15,
                sidewalk_division: 2,
                terrain: 1,
            }
        );
        assert!(
            [
                r.route,
                r.route_class,
                r.road_shape,
                r.carriageway_width,
                r.sidewalk_division,
                r.terrain
            ]
            .iter()
            .all(|x| *x == Reading::Valid)
        );
        assert_eq!(r.width_remapped, None);
        assert_eq!(r.attr.validate(), Ok(()));
    }

    #[test]
    fn route_code_splits_into_upper_four_and_lower_one() {
        for (text, upper, lower, class) in [
            ("00010", 1, 0, Some(RouteClass::NationalRoute)),
            ("00171", 17, 1, Some(RouteClass::NationalRoute)),
            ("09999", 999, 9, Some(RouteClass::NationalRoute)),
            ("10001", 1000, 1, Some(RouteClass::MajorPrefecturalRoad)),
            ("40000", 4000, 0, Some(RouteClass::Expressway)),
            ("99000", 9900, 0, Some(RouteClass::GeneralTrafficOther)),
            (" 37990 ", 3799, 0, Some(RouteClass::MunicipalRoad)),
            // 表にない上4桁：上4桁と下1桁は読み、区分は未知
            ("00000", 0, 0, None),
            ("97003", 9700, 3, None),
            ("99999", 9999, 9, None),
        ] {
            let r = read_attrs(&fields(text, "14", "03", "4", "3"), 2024);
            assert_eq!(r.attr.route_upper, upper, "{text}");
            assert_eq!(r.attr.route_lower, lower, "{text}");
            assert_eq!(r.attr.route_class_kind(), class, "{text}");
            assert_eq!(r.route, Reading::Valid, "{text}");
            assert_eq!(
                r.route_class,
                if class.is_some() {
                    Reading::Valid
                } else {
                    Reading::Unknown
                },
                "{text}"
            );
            assert_eq!(r.attr.validate(), Ok(()), "{text}");
        }
    }

    #[test]
    fn missing_and_unknown_values() {
        // 空・空白・行に列がない：欠損
        let r = read_attrs(&AttrFields::default(), 2024);
        assert_eq!(r.attr, AccidentAttr::MISSING);
        for x in [
            r.route,
            r.route_class,
            r.road_shape,
            r.carriageway_width,
            r.sidewalk_division,
            r.terrain,
        ] {
            assert_eq!(x, Reading::Missing);
        }
        let r = read_attrs(&fields("", "  ", "", "", ""), 2024);
        assert_eq!(r.attr, AccidentAttr::MISSING);
        assert_eq!(r.road_shape, Reading::Missing);

        // 空でないがコード表にない：未知（値は欠損と同じ0xFF）
        let r = read_attrs(&fields("1234", "02", "07", "0", "4"), 2024);
        assert_eq!(r.attr, AccidentAttr::MISSING);
        for x in [
            r.route,
            r.road_shape,
            r.carriageway_width,
            r.sidewalk_division,
            r.terrain,
        ] {
            assert_eq!(x, Reading::Unknown);
        }
        // 数字以外・符号・全角・桁あふれ
        for text in ["abc", "-1", "+1", "1.0", "１４", "300", "1e1"] {
            let r = read_attrs(&fields("30000", text, text, text, text), 2024);
            assert_eq!(r.road_shape, Reading::Unknown, "{text}");
            assert_eq!(r.carriageway_width, Reading::Unknown, "{text}");
            assert_eq!(r.attr.sidewalk_division, MISSING_U8, "{text}");
        }
        for text in ["3000", "300000", "3000a", "３００００", "-3000"] {
            let r = read_attrs(&fields(text, "14", "03", "4", "3"), 2024);
            assert_eq!(r.route, Reading::Unknown, "{text}");
            assert_eq!(r.attr.route_upper, MISSING_U16, "{text}");
            assert_eq!(r.attr.route_lower, MISSING_U8, "{text}");
        }
    }

    #[test]
    fn carriageway_width_codes_of_2022_are_mapped_to_the_2024_codes() {
        for (raw, mapped) in [("12", 14), ("13", 17), ("16", 18), ("14", 14), ("15", 15)] {
            let r = read_attrs(&fields("30000", "01", raw, "2", "1"), 2022);
            assert_eq!(r.carriageway_width, Reading::Valid, "{raw}");
            assert_eq!(r.attr.carriageway_width, mapped, "{raw}");
            let remapped = raw.parse::<u8>().unwrap() != mapped;
            assert_eq!(
                r.width_remapped,
                remapped.then(|| (raw.parse().unwrap(), mapped))
            );
        }
        // 2022年以外には12・13・16のコードがない
        for year in [2019, 2020, 2021, 2023, 2024, 2025] {
            for raw in ["12", "13", "16"] {
                let r = read_attrs(&fields("30000", "01", raw, "2", "1"), year);
                assert_eq!(r.carriageway_width, Reading::Unknown, "{year} {raw}");
                assert_eq!(r.width_remapped, None);
            }
        }
    }

    #[test]
    fn columns_are_found_by_normalized_name() {
        let header = [
            "資料区分",
            "路線 コード",
            "道路　形状",
            "車道幅員",
            "歩車道区分",
            "地形",
        ];
        let columns = AttrColumnIndex::from_header(&header).unwrap();
        assert_eq!(
            AttrColumn::ALL.map(|c| columns.position(c)),
            [1, 2, 3, 4, 5]
        );
        assert_eq!(
            AttrColumnIndex::from_header(&header[..5]),
            Err(HeaderError::Missing("地形"))
        );
        let mut duplicate = header.to_vec();
        duplicate.push("地　形");
        assert_eq!(
            AttrColumnIndex::from_header(&duplicate),
            Err(HeaderError::Duplicate("地形"))
        );
    }

    #[test]
    fn audit_counts_distribution_missing_and_unknown() {
        let mut audit = AttrAudit::new();
        let rows: [(AttrFields, u16); 4] = [
            (fields("30000", "01", "15", "2", "1"), 2022),
            (fields("00011", "14", "12", "4", "3"), 2022),
            (fields("97000", "99", "", "9", ""), 2022),
            (AttrFields::default(), 2022),
        ];
        for (f, year) in &rows {
            audit.record(f, &read_attrs(f, *year));
        }
        let json = audit.to_json();
        assert_eq!(json["rows"], 4);
        assert_eq!(json["missing"]["route"], 1);
        assert_eq!(json["missing"]["carriageway_width"], 2);
        assert_eq!(json["missing"]["terrain"], 2);
        assert_eq!(json["unknown"]["route"], 0);
        assert_eq!(json["unknown"]["route_class"], 1);
        assert_eq!(json["unknown"]["road_shape"], 1);
        assert_eq!(json["unknown"]["sidewalk_division"], 1);
        assert_eq!(json["unknown_values"]["route_class"]["9700"], 1);
        assert_eq!(json["unknown_values"]["road_shape"]["99"], 1);
        assert_eq!(json["distribution"]["route_class"]["4"], 1);
        assert_eq!(json["distribution"]["route_class"]["0"], 1);
        assert_eq!(json["distribution"]["carriageway_width"]["14"], 1);
        assert_eq!(json["route_lower_bypass"], 1);
        assert_eq!(json["width_remapped"]["12->14"], 1);
        // 合計
        let mut total = AttrAudit::new();
        total.merge(&audit);
        total.merge(&audit);
        assert_eq!(total.to_json()["rows"], 8);
        assert_eq!(total.to_json()["distribution"]["route_class"]["4"], 2);
    }

    #[test]
    fn sheet_verification_detects_changed_codes() {
        let row = |b: &str| (b.to_owned(), String::new());
        let sheets = |width: &[&str]| -> AttrSheets {
            let mut s = AttrSheets::new();
            s.insert("路線".to_owned(), {
                let mut rows = vec![row("コード（上４桁）")];
                rows.extend(RouteClass::ALL.iter().map(|c| {
                    let (lo, hi) = c.upper_range();
                    row(&if lo == hi {
                        format!("{lo:04}")
                    } else {
                        format!("{lo:04}～{hi:04}")
                    })
                }));
                rows.push(row("*1 路線（高速自動車国道）コード表を参照"));
                rows
            });
            s.insert(
                "道路形状".to_owned(),
                ROAD_SHAPE_CODES
                    .iter()
                    .map(|c| row(&format!("{c:02}")))
                    .collect(),
            );
            s.insert(
                "歩車道区分".to_owned(),
                SIDEWALK_DIVISION_CODES
                    .iter()
                    .map(|c| row(&c.to_string()))
                    .collect(),
            );
            s.insert(
                "地形".to_owned(),
                TERRAIN_CODES.iter().map(|c| row(&c.to_string())).collect(),
            );
            s.insert(
                "車道幅員".to_owned(),
                width.iter().map(|c| row(c)).collect(),
            );
            s
        };
        let width_2024 = [
            "00", "01", "02", "03", "04", "05", "06", "11", "14", "15", "17", "18", "19",
        ];
        let width_2022 = [
            "00", "01", "02", "03", "04", "05", "06", "11", "12,14", "15", "13,17", "16,18", "19",
        ];
        verify_attr_sheets(2024, &sheets(&width_2024)).unwrap();
        verify_attr_sheets(2022, &sheets(&width_2022)).unwrap();
        // 年によるコード表の違いを取り違えると失敗する
        assert!(verify_attr_sheets(2022, &sheets(&width_2024)).is_err());
        assert!(verify_attr_sheets(2024, &sheets(&width_2022)).is_err());
        // コードが増える・減る
        let mut changed = sheets(&width_2024);
        changed.get_mut("地形").unwrap().push(row("4"));
        assert!(verify_attr_sheets(2024, &changed).is_err());
        let mut changed = sheets(&width_2024);
        changed.get_mut("道路形状").unwrap().pop();
        assert!(verify_attr_sheets(2024, &changed).is_err());
        let mut changed = sheets(&width_2024);
        changed.get_mut("路線").unwrap()[2] = row("1000～1500");
        assert!(verify_attr_sheets(2024, &changed).is_err());
        let mut changed = sheets(&width_2024);
        changed.remove("歩車道区分");
        assert!(verify_attr_sheets(2024, &changed).is_err());
    }
}
