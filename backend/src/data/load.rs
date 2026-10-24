//! releaseの起動時ロードと検証（behavior.md 3章）。
//!
//! gzipと祝日表の実ファイルから `dataset_id`・`release_id` を再計算してmetadataと照合し、
//! schema_version・件数・列挙値を再検証する。メッセージに位置や原本行を含めない。

use std::fs;
use std::path::Path;
use std::time::Instant;

use chrono::NaiveDate;
use flate2::read::GzDecoder;
use serde_json::Value;

use crate::contract::geo::{LAT_RANGE, LON_RANGE};
use crate::contract::{DatasetInfo, DayType, Period, Weather};
use crate::domain::{AccidentColumns, AccidentSnapshot, HolidayCalendar};

use super::LoadError;
use super::holidays::parse_holiday_csv;
use super::preprocess::rows::OUTPUT_COLUMNS;
use super::release::{
    ACCIDENTS_FILE, HOLIDAYS_FILE, METADATA_FILE, SCHEMA_VERSION, release_id, sha256_hex,
};

fn invalid(message: impl Into<String>) -> LoadError {
    LoadError::Invalid(message.into())
}

/// metadata.jsonを読み、schema_versionを確かめる。
pub fn read_metadata(release_dir: &Path) -> Result<Value, LoadError> {
    let bytes = fs::read(release_dir.join(METADATA_FILE))?;
    let metadata: Value =
        serde_json::from_slice(&bytes).map_err(|_| invalid("metadata.jsonがJSONではありません"))?;
    if metadata.get("schema_version").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
        return Err(invalid("schema_versionが2ではありません"));
    }
    Ok(metadata)
}

fn metadata_str<'a>(metadata: &'a Value, path: &[&str]) -> Result<&'a str, LoadError> {
    let mut v = metadata;
    for key in path {
        v = v
            .get(key)
            .ok_or_else(|| invalid(format!("metadataに{}がありません", path.join("."))))?;
    }
    v.as_str().ok_or_else(|| {
        invalid(format!(
            "metadataの{}が文字列ではありません",
            path.join(".")
        ))
    })
}

/// 祝日表を読み、metadataのハッシュと照合する。ハッシュも返す。
fn read_holidays(
    release_dir: &Path,
    metadata: &Value,
) -> Result<(HolidayCalendar, String), LoadError> {
    let bytes = fs::read(release_dir.join(HOLIDAYS_FILE))?;
    let sha256 = sha256_hex(&bytes);
    if metadata_str(metadata, &["holiday_table", "sha256"])? != sha256 {
        return Err(invalid("祝日表のハッシュがmetadataと一致しません"));
    }
    let calendar = parse_holiday_csv(&bytes).map_err(|e| invalid(e.to_string()))?;
    Ok((calendar, sha256))
}

/// 祝日表だけを読む（`load_calendar`）。事故データの全件ロードを待たない。
pub fn load_holiday_calendar(release_dir: &Path) -> Result<HolidayCalendar, LoadError> {
    let metadata = read_metadata(release_dir)?;
    read_holidays(release_dir, &metadata).map(|(calendar, _)| calendar)
}

fn parse_field<T: std::str::FromStr>(value: &str) -> Result<T, LoadError> {
    value
        .parse()
        .map_err(|_| invalid("accidents.csv.gzに不正な値があります"))
}

fn check(ok: bool) -> Result<(), LoadError> {
    if ok {
        Ok(())
    } else {
        Err(invalid("accidents.csv.gzに範囲外の値があります"))
    }
}

/// gzip CSVを列配列に読む。先頭10列（`day_type` まで）だけを数値化して検証する。
pub fn parse_accidents(gzip: &[u8]) -> Result<AccidentColumns, LoadError> {
    // 展開しながら読む（展開後の全体をメモリに置かない）
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(GzDecoder::new(gzip));
    let header = reader
        .headers()
        .map_err(|_| invalid("accidents.csv.gzのヘッダを読めません"))?;
    if header.iter().ne(OUTPUT_COLUMNS) {
        return Err(invalid("accidents.csv.gzの列が17列の形式と一致しません"));
    }
    let mut c = AccidentColumns::default();
    for record in reader.records() {
        // 列数の不一致もここでCSVのエラーになる
        let record = record.map_err(|_| invalid("accidents.csv.gzの行を読めません"))?;
        let lat: f64 = parse_field(&record[0])?;
        let lon: f64 = parse_field(&record[1])?;
        let modes: u8 = parse_field(&record[2])?;
        let period: u8 = parse_field(&record[3])?;
        let weather: u8 = parse_field(&record[4])?;
        let day_type: u8 = parse_field(&record[5])?;
        let deaths: u16 = parse_field(&record[6])?;
        let injuries: u16 = parse_field(&record[7])?;
        let source_year: u16 = parse_field(&record[8])?;
        let date: u32 = parse_field(&record[9])?;
        check(LAT_RANGE.contains(&lat) && LON_RANGE.contains(&lon))?;
        check(modes <= 7)?;
        check(Period::from_code(period).is_some())?;
        check(Weather::from_code(weather).is_some())?;
        check(DayType::from_code(day_type).is_some())?;
        // 人数は各0〜999で合計1以上（behavior.md 2章）
        check(deaths <= 999 && injuries <= 999 && deaths + injuries >= 1)?;
        // 発生日は実在し、1900 <= 発生年 <= source_year（behavior.md 2章）
        let year = date / 10_000;
        check(
            (1900..=u32::from(source_year)).contains(&year)
                && NaiveDate::from_ymd_opt(year as i32, (date / 100) % 100, date % 100).is_some(),
        )?;
        c.lat.push(lat);
        c.lon.push(lon);
        c.modes.push(modes);
        c.period.push(period);
        c.weather.push(weather);
        c.day_type.push(day_type);
        c.deaths.push(deaths);
        c.injuries.push(injuries);
        c.source_year.push(source_year);
        c.date.push(date);
    }
    if c.is_empty() {
        return Err(invalid("accidents.csv.gzに事故がありません"));
    }
    Ok(c)
}

/// releaseを読み、識別子と件数を検証してスナップショットを作る。
pub fn load_snapshot(release_dir: &Path) -> Result<(AccidentSnapshot, DatasetInfo), LoadError> {
    let started = Instant::now();
    let metadata = read_metadata(release_dir)?;
    let (_, holiday_sha256) = read_holidays(release_dir, &metadata)?;
    let gzip = fs::read(release_dir.join(ACCIDENTS_FILE))?;
    let dataset_id = sha256_hex(&gzip);
    if metadata_str(&metadata, &["dataset_id"])? != dataset_id {
        return Err(invalid("dataset_idがaccidents.csv.gzと一致しません"));
    }
    let release_id = release_id(&dataset_id, &holiday_sha256);
    if metadata_str(&metadata, &["release_id"])? != release_id {
        return Err(invalid("release_idが実ファイルと一致しません"));
    }
    let columns = parse_accidents(&gzip)?;
    if metadata.get("rows").and_then(Value::as_u64) != Some(columns.len() as u64) {
        return Err(invalid("事故の件数がmetadataと一致しません"));
    }
    let mut data_years = columns.source_year.clone();
    data_years.sort_unstable();
    data_years.dedup();
    let snapshot = AccidentSnapshot::new(columns);
    let dataset = DatasetInfo {
        release_id,
        dataset_id,
        accidents: snapshot.len() as u64,
        data_years,
        load_seconds: started.elapsed().as_secs_f64(),
        grid_cells: Some(snapshot.grid_cells()),
    };
    Ok((snapshot, dataset))
}
