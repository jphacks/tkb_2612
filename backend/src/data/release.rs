//! release成果物の書き出しと識別子（behavior.md 2章・3章、architecture.md 5章）。
//!
//! release ディレクトリは `accidents.csv.gz`・`metadata.json`・`syukujitsu.csv` の3ファイル。
//! `dataset_id` は gzip 全バイトの SHA-256、`release_id` は `dataset_id` と祝日表の SHA-256 を
//! `\n` で連結したUTF-8文字列の SHA-256（いずれも小文字hex）。

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::Path;

use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::domain::HolidayCalendar;

use super::preprocess::rows::{AccidentRecord, OUTPUT_COLUMNS};

pub const ACCIDENTS_FILE: &str = "accidents.csv.gz";
pub const METADATA_FILE: &str = "metadata.json";
pub const HOLIDAYS_FILE: &str = "syukujitsu.csv";
/// 17列のgzip CSVの形式。
pub const SCHEMA_VERSION: u64 = 2;
/// gzipの圧縮レベル。
pub const GZIP_LEVEL: u32 = 3;
/// metadataの `processor.name`。
pub const PROCESSOR_NAME: &str = "prepare_release";

/// 小文字hexのSHA-256。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// `release_id`：`dataset_id` と祝日表のハッシュを `\n` で連結した文字列のSHA-256。
pub fn release_id(dataset_id: &str, holiday_sha256: &str) -> String {
    sha256_hex(format!("{dataset_id}\n{holiday_sha256}").as_bytes())
}

/// 保持行を順にgzip CSVへ書く。行数・年度・祝日表の収録範囲外の行数も数える。
pub struct AccidentCsvWriter {
    writer: csv::Writer<GzEncoder<Vec<u8>>>,
    rows: u64,
    years: BTreeSet<u16>,
    holiday_uncovered_rows: u64,
}

impl Default for AccidentCsvWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl AccidentCsvWriter {
    pub fn new() -> Self {
        let encoder = GzEncoder::new(Vec::new(), Compression::new(GZIP_LEVEL));
        let mut writer = csv::WriterBuilder::new()
            .terminator(csv::Terminator::Any(b'\n'))
            .from_writer(encoder);
        writer
            .write_record(OUTPUT_COLUMNS)
            .expect("メモリへの書込みは失敗しない");
        Self {
            writer,
            rows: 0,
            years: BTreeSet::new(),
            holiday_uncovered_rows: 0,
        }
    }

    pub fn push(&mut self, record: &AccidentRecord) {
        self.writer
            .write_record(record.output_fields())
            .expect("メモリへの書込みは失敗しない");
        self.rows += 1;
        self.years.insert(record.source_year);
        if !record.holiday_table_covered {
            self.holiday_uncovered_rows += 1;
        }
    }

    pub fn rows(&self) -> u64 {
        self.rows
    }

    /// gzipのバイト列と集計を返す。
    pub fn finish(self) -> AccidentCsv {
        let encoder = self
            .writer
            .into_inner()
            .unwrap_or_else(|_| panic!("メモリへの書込みは失敗しない"));
        let gzip = encoder.finish().expect("メモリへの書込みは失敗しない");
        AccidentCsv {
            gzip,
            rows: self.rows,
            data_years: self.years.into_iter().collect(),
            holiday_uncovered_rows: self.holiday_uncovered_rows,
        }
    }
}

/// 書き終えた事故CSV。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccidentCsv {
    pub gzip: Vec<u8>,
    pub rows: u64,
    pub data_years: Vec<u16>,
    pub holiday_uncovered_rows: u64,
}

/// releaseの3ファイルの内容。
#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseContent {
    pub dataset_id: String,
    pub release_id: String,
    pub gzip: Vec<u8>,
    pub holidays: Vec<u8>,
    pub metadata: Value,
}

/// metadata.jsonを組み立てる。`extra` のキー（前処理の監査等）は必須キーの後に加える。
///
/// キーの一覧は backend/README.md の「release形式」の表を参照。
pub fn build_release(
    accidents: AccidentCsv,
    holidays: Vec<u8>,
    calendar: &HolidayCalendar,
    generated_at: &str,
    extra: Map<String, Value>,
) -> ReleaseContent {
    let dataset_id = sha256_hex(&accidents.gzip);
    let holiday_sha256 = sha256_hex(&holidays);
    let release_id = release_id(&dataset_id, &holiday_sha256);
    let (first, last) = calendar.table_range();
    let (coverage_start, coverage_end) = calendar.coverage();
    let mut metadata = json!({
        "schema_version": SCHEMA_VERSION,
        "release_id": release_id,
        "dataset_id": dataset_id,
        "processor": { "name": PROCESSOR_NAME, "version": env!("CARGO_PKG_VERSION") },
        "generated_at": generated_at,
        "columns": OUTPUT_COLUMNS,
        "rows": accidents.rows,
        "gzip_bytes": accidents.gzip.len(),
        "data_years": accidents.data_years,
        "holiday_table": {
            "file": HOLIDAYS_FILE,
            "sha256": holiday_sha256,
            "first_date": first.to_string(),
            "last_date": last.to_string(),
            "rows": calendar.len(),
            "coverage_start": coverage_start.to_string(),
            "coverage_end": coverage_end.to_string(),
        },
        "holiday_uncovered_rows": accidents.holiday_uncovered_rows,
    });
    let object = metadata.as_object_mut().expect("objectで作った");
    for (key, value) in extra {
        object.insert(key, value);
    }
    ReleaseContent {
        dataset_id,
        release_id,
        gzip: accidents.gzip,
        holidays,
        metadata,
    }
}

/// 3ファイルをディレクトリへ書く。ディレクトリは無ければ作る。
pub fn write_release_files(dir: &Path, content: &ReleaseContent) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::write(dir.join(ACCIDENTS_FILE), &content.gzip)?;
    fs::write(dir.join(HOLIDAYS_FILE), &content.holidays)?;
    let mut json = serde_json::to_vec_pretty(&content.metadata).expect("JSONに書ける値");
    json.push(b'\n');
    let mut file = fs::File::create(dir.join(METADATA_FILE))?;
    file.write_all(&json)?;
    file.sync_all()
}

/// releaseの3ファイルを読み取り専用にする。
pub fn make_read_only(dir: &Path) -> std::io::Result<()> {
    for name in [ACCIDENTS_FILE, HOLIDAYS_FILE, METADATA_FILE] {
        let path = dir.join(name);
        let mut permissions = fs::metadata(&path)?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions)?;
    }
    Ok(())
}
