//! CSV前処理（behavior.md 2章）。原本 `accident/` は読むだけで書き込まない。
//!
//! 1. コード表を照合する（照合器は引数。CLIは常に実照合器）。
//! 2. `honhyo_2019.csv`〜`honhyo_2025.csv` を年昇順・入力行順に処理する。
//! 3. 保持行を17列のgzip CSVへ書き、監査（年度別・理由別の件数）をmetadataへ記録する。
//!
//! I/O・CSV構文・途中の復号失敗・必須ファイル欠損・必須列の欠損/重複・コード表不一致・
//! 全行除外は前処理全体の失敗。一部年度だけで成功扱いにしない。

pub mod codebook;
pub mod rows;

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::contract::Calendar;
use crate::domain::HolidayCalendar;

use self::codebook::{CodebookError, CodebookFile, CodebookVerifier};
use self::rows::{Column, ColumnIndex, ExclusionReason, convert_row};
use super::csv_syntax::ends_inside_quoted_field;
use super::encoding::decode_text;
use super::release::{AccidentCsvWriter, ReleaseContent, build_release, sha256_hex};

/// 必須の年度（この順に処理する）。未知年度は自動で追加しない。
pub const SOURCE_YEARS: std::ops::RangeInclusive<u16> = 2019..=2025;

pub fn source_file_name(year: u16) -> String {
    format!("honhyo_{year}.csv")
}

/// 前処理の失敗。
#[derive(Debug)]
pub enum PreprocessError {
    Io {
        file: String,
        error: std::io::Error,
    },
    Codebook(CodebookError),
    /// 文字コード・CSV構文・ヘッダ等、ファイル単位の失敗。
    File {
        file: String,
        message: String,
    },
    /// 保持行が0件。
    NoRetainedRows,
    Holidays(String),
}

impl std::fmt::Display for PreprocessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PreprocessError::Io { file, error } => write!(f, "{file} を読めません: {error}"),
            PreprocessError::Codebook(e) => write!(f, "{e}"),
            PreprocessError::File { file, message } => write!(f, "{file}: {message}"),
            PreprocessError::NoRetainedRows => write!(f, "保持する行が1件もありません"),
            PreprocessError::Holidays(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for PreprocessError {}

impl From<CodebookError> for PreprocessError {
    fn from(value: CodebookError) -> Self {
        PreprocessError::Codebook(value)
    }
}

/// 1ファイルの監査。除外した原本行の内容は持たない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileAudit {
    pub source_year: u16,
    pub file: String,
    pub sha256: String,
    pub encoding: &'static str,
    pub input_rows: u64,
    pub retained_rows: u64,
    pub excluded_rows: u64,
    pub excluded_by_reason: BTreeMap<ExclusionReason, u64>,
    /// 保持行のうち対象手段のない（modes=0）行。
    pub no_target_mode: u64,
}

impl FileAudit {
    fn to_json(&self) -> Value {
        let reasons: Map<String, Value> = self
            .excluded_by_reason
            .iter()
            .map(|(reason, count)| (reason.as_str().to_owned(), json!(count)))
            .collect();
        json!({
            "source_year": self.source_year,
            "file": self.file,
            "sha256": self.sha256,
            "encoding": self.encoding,
            "input_rows": self.input_rows,
            "retained_rows": self.retained_rows,
            "excluded_rows": self.excluded_rows,
            "excluded_by_reason": reasons,
            "no_target_mode": self.no_target_mode,
        })
    }
}

/// 1年度のCSVのバイト列を処理し、保持行を `writer` へ書く。
pub fn process_file(
    file: &str,
    bytes: &[u8],
    source_year: u16,
    calendar: &dyn Calendar,
    writer: &mut AccidentCsvWriter,
) -> Result<FileAudit, PreprocessError> {
    let file_error = |message: String| PreprocessError::File {
        file: file.to_owned(),
        message,
    };
    let (encoding, text) = decode_text(bytes).map_err(|e| file_error(e.to_string()))?;
    if ends_inside_quoted_field(&text) {
        return Err(file_error(
            "引用符が閉じないままファイルが終わっています".to_owned(),
        ));
    }
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(text.as_bytes());
    let header: Vec<String> = reader
        .headers()
        .map_err(|e| file_error(format!("ヘッダを読めません: {e}")))?
        .iter()
        .map(str::to_owned)
        .collect();
    let columns = ColumnIndex::from_header(&header).map_err(|e| file_error(e.to_string()))?;
    let mut audit = FileAudit {
        source_year,
        file: file.to_owned(),
        sha256: sha256_hex(bytes),
        encoding: encoding.as_str(),
        input_rows: 0,
        retained_rows: 0,
        excluded_rows: 0,
        excluded_by_reason: BTreeMap::new(),
        no_target_mode: 0,
    };
    for record in reader.records() {
        let record = record.map_err(|e| file_error(format!("CSVの構文が不正です: {e}")))?;
        audit.input_rows += 1;
        match convert_row(
            |column: Column| record.get(columns.position(column)),
            source_year,
            calendar,
        ) {
            Ok(accident) => {
                audit.retained_rows += 1;
                if accident.modes == 0 {
                    audit.no_target_mode += 1;
                }
                writer.push(&accident);
            }
            Err(reason) => {
                audit.excluded_rows += 1;
                *audit.excluded_by_reason.entry(reason).or_default() += 1;
            }
        }
    }
    if audit.input_rows != audit.retained_rows + audit.excluded_rows {
        return Err(file_error("入力行数が保持+除外と一致しません".to_owned()));
    }
    Ok(audit)
}

/// 前処理の結果。
pub struct Prepared {
    pub release: ReleaseContent,
    pub files: Vec<FileAudit>,
}

/// 原本ディレクトリと祝日CSVからreleaseの内容を作る（ファイルにはまだ書かない）。
///
/// `generated_at` はタイムゾーン付きの日時（RFC 3339）。経過秒はmetadataの `elapsed_seconds`。
pub fn prepare_release(
    accident_dir: &Path,
    holidays: Vec<u8>,
    verifier: &dyn CodebookVerifier,
    generated_at: &str,
) -> Result<Prepared, PreprocessError> {
    let started = std::time::Instant::now();
    let calendar: HolidayCalendar = super::holidays::parse_holiday_csv(&holidays)
        .map_err(|e| PreprocessError::Holidays(e.to_string()))?;
    let (codebooks_xlsx, codebooks_pdf) = verifier.verify(accident_dir)?;

    let mut writer = AccidentCsvWriter::new();
    let mut files = Vec::new();
    for year in SOURCE_YEARS {
        let file = source_file_name(year);
        let bytes =
            std::fs::read(accident_dir.join(&file)).map_err(|error| PreprocessError::Io {
                file: file.clone(),
                error,
            })?;
        files.push(process_file(&file, &bytes, year, &calendar, &mut writer)?);
    }
    if writer.rows() == 0 {
        return Err(PreprocessError::NoRetainedRows);
    }
    let input: u64 = files.iter().map(|f| f.input_rows).sum();
    let retained: u64 = files.iter().map(|f| f.retained_rows).sum();
    let excluded: u64 = files.iter().map(|f| f.excluded_rows).sum();
    let codebook_json = |list: &[CodebookFile]| -> Value {
        list.iter()
            .map(|c| json!({ "source_year": c.source_year, "file": c.file, "sha256": c.sha256 }))
            .collect()
    };
    let accidents = writer.finish();
    let mut extra = Map::new();
    extra.insert(
        "files".to_owned(),
        files.iter().map(FileAudit::to_json).collect(),
    );
    extra.insert(
        "totals".to_owned(),
        json!({ "input_rows": input, "retained_rows": retained, "excluded_rows": excluded }),
    );
    extra.insert("codebooks_xlsx".to_owned(), codebook_json(&codebooks_xlsx));
    extra.insert("codebooks_pdf".to_owned(), codebook_json(&codebooks_pdf));
    extra.insert(
        "elapsed_seconds".to_owned(),
        json!(crate::domain::py_round(started.elapsed().as_secs_f64(), 3)),
    );
    let release = build_release(accidents, holidays, &calendar, generated_at, extra);
    Ok(Prepared { release, files })
}
