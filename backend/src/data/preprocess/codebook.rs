//! コード表の照合（behavior.md 2章「XLSX照合」）。
//!
//! 2019〜2021は `codebook_YEAR.pdf` の存在だけを確認する（内容はcodebook_notesの手動照合が根拠）。
//! 2022〜2025は `codebook_YEAR.xlsx` の天候・当事者種別・人身損傷程度の3シートを毎回照合する。
//! 照合の本体（抽出済みのコード表の比較）は [`verify_code_sheets`] で、XLSXの読取と分ける。

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use super::super::release::sha256_hex;
use super::rows::{CAR_CODES, INJURY_CODES, party_codes};

/// 内容を自動照合しない（PDFの存在だけを確認する）年度。
pub const PDF_YEARS: std::ops::RangeInclusive<u16> = 2019..=2021;
/// XLSXを照合する年度。
pub const XLSX_YEARS: std::ops::RangeInclusive<u16> = 2022..=2025;

pub const WEATHER_SHEET: &str = "天候";
pub const PARTY_SHEET: &str = "当事者種別";
pub const INJURY_SHEET: &str = "人身損傷程度";

/// 1シートのコード（B列が数字の行）と説明（C列）。
pub type CodeSheet = BTreeMap<u16, String>;

/// 3シートの内容。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeSheets {
    pub weather: CodeSheet,
    pub party: CodeSheet,
    pub injury: CodeSheet,
}

/// コード表の不一致・読取失敗。前処理全体の失敗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodebookError(pub String);

impl std::fmt::Display for CodebookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "コード表の照合に失敗しました: {}", self.0)
    }
}

impl std::error::Error for CodebookError {}

fn expect_prefix(
    year: u16,
    sheet: &str,
    codes: &CodeSheet,
    code: u16,
    prefix: &str,
) -> Result<(), CodebookError> {
    match codes.get(&code) {
        Some(description) if description.starts_with(prefix) => Ok(()),
        Some(_) => Err(CodebookError(format!(
            "{year}年 {sheet} のコード{code}の説明が「{prefix}」で始まりません"
        ))),
        None => Err(CodebookError(format!(
            "{year}年 {sheet} にコード{code}がありません"
        ))),
    }
}

/// 抽出済みのコード表を、前処理が使うコード集合と照合する。
///
/// - 当事者種別：集合全体が `party_codes(year)` と一致し、自動車コードの説明が乗用車/貨物車/
///   特殊車、61・51・52の説明が歩行者・自転車・駆動補助機付自転車で始まる。
/// - 天候1〜5の説明が晴・曇・雨・霧・雪で、損傷0/1/2/4の説明が対象外・死亡・負傷・損傷なしで
///   始まる。天候・損傷の余分なコードは拒否しない（実データの行では集合外を除外する）。
pub fn verify_code_sheets(year: u16, sheets: &CodeSheets) -> Result<(), CodebookError> {
    let expected: BTreeSet<u16> = party_codes(year).into_iter().collect();
    let actual: BTreeSet<u16> = sheets.party.keys().copied().collect();
    if actual != expected {
        let missing: Vec<_> = expected.difference(&actual).collect();
        let unexpected: Vec<_> = actual.difference(&expected).collect();
        return Err(CodebookError(format!(
            "{year}年 当事者種別の集合が一致しません（不足 {missing:?}、余分 {unexpected:?}）"
        )));
    }
    for code in CAR_CODES {
        let prefix = match code {
            1..=7 => "乗用車",
            11..=17 => "貨物車",
            _ => "特殊車",
        };
        expect_prefix(year, PARTY_SHEET, &sheets.party, code, prefix)?;
    }
    expect_prefix(year, PARTY_SHEET, &sheets.party, 61, "歩行者")?;
    expect_prefix(year, PARTY_SHEET, &sheets.party, 51, "軽車両－自転車")?;
    expect_prefix(
        year,
        PARTY_SHEET,
        &sheets.party,
        52,
        "軽車両－駆動補助機付自転車",
    )?;
    for (code, prefix) in [(1, "晴"), (2, "曇"), (3, "雨"), (4, "霧"), (5, "雪")] {
        expect_prefix(year, WEATHER_SHEET, &sheets.weather, code, prefix)?;
    }
    for (code, prefix) in INJURY_CODES
        .into_iter()
        .zip(["対象外", "死亡", "負傷", "損傷なし"])
    {
        expect_prefix(year, INJURY_SHEET, &sheets.injury, code, prefix)?;
    }
    Ok(())
}

// ---- XLSXの読取 ----

const MAIN_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn xlsx_error(e: impl std::fmt::Display) -> CodebookError {
    CodebookError(format!("XLSXを読めません: {e}"))
}

fn read_entry<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<Option<String>, CodebookError> {
    let mut file = match archive.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(xlsx_error(e)),
    };
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(xlsx_error)?;
    Ok(Some(text))
}

fn is_main(node: &roxmltree::Node, name: &str) -> bool {
    node.is_element()
        && node.tag_name().name() == name
        && node.tag_name().namespace() == Some(MAIN_NS)
}

/// 文字列要素の本文。`<t>` と rich text の `<r><t>` を連結し、ふりがな（`<rPh>`）は除く。
fn string_item_text(item: roxmltree::Node) -> String {
    let mut text = String::new();
    for child in item.children() {
        if is_main(&child, "t") {
            text.push_str(child.text().unwrap_or(""));
        } else if is_main(&child, "r") {
            for t in child.children().filter(|n| is_main(n, "t")) {
                text.push_str(t.text().unwrap_or(""));
            }
        }
    }
    text
}

fn shared_strings(xml: Option<&str>) -> Result<Vec<String>, CodebookError> {
    let Some(xml) = xml else {
        return Ok(Vec::new());
    };
    let doc = roxmltree::Document::parse(xml).map_err(xlsx_error)?;
    Ok(doc
        .root_element()
        .children()
        .filter(|n| is_main(n, "si"))
        .map(string_item_text)
        .collect())
}

/// `xl/workbook.xml` と relationships から、シート名→シートXMLのパス。
fn sheet_paths(workbook: &str, rels: &str) -> Result<BTreeMap<String, String>, CodebookError> {
    let rels = roxmltree::Document::parse(rels).map_err(xlsx_error)?;
    let targets: BTreeMap<&str, &str> = rels
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "Relationship")
        .filter_map(|n| Some((n.attribute("Id")?, n.attribute("Target")?)))
        .collect();
    let workbook = roxmltree::Document::parse(workbook).map_err(xlsx_error)?;
    let mut paths = BTreeMap::new();
    for sheet in workbook.descendants().filter(|n| is_main(n, "sheet")) {
        let (Some(name), Some(id)) = (sheet.attribute("name"), sheet.attribute((REL_NS, "id")))
        else {
            continue;
        };
        if let Some(target) = targets.get(id) {
            // 絶対パス（/xl/...）とxl/からの相対パスの両方がある
            let path = match target.strip_prefix('/') {
                Some(absolute) => absolute.to_owned(),
                None => format!("xl/{target}"),
            };
            paths.insert(name.to_owned(), path);
        }
    }
    Ok(paths)
}

/// 列名（`B12` の `B`）。
fn column_of(reference: &str) -> &str {
    let end = reference
        .find(|c: char| c.is_ascii_digit())
        .unwrap_or(reference.len());
    &reference[..end]
}

fn cell_text(cell: roxmltree::Node, strings: &[String]) -> Option<String> {
    let value = cell
        .children()
        .find(|n| is_main(n, "v"))
        .and_then(|v| v.text());
    match cell.attribute("t") {
        Some("s") => strings.get(value?.trim().parse::<usize>().ok()?).cloned(),
        Some("inlineStr") => cell
            .children()
            .find(|n| is_main(n, "is"))
            .map(string_item_text),
        _ => value.map(str::to_owned),
    }
}

/// シートの、B列が数字の行のコードとC列の説明。
fn code_sheet(xml: &str, strings: &[String]) -> Result<CodeSheet, CodebookError> {
    let doc = roxmltree::Document::parse(xml).map_err(xlsx_error)?;
    let mut sheet = CodeSheet::new();
    for row in doc.descendants().filter(|n| is_main(n, "row")) {
        let mut b = None;
        let mut c = None;
        for cell in row.children().filter(|n| is_main(n, "c")) {
            match cell.attribute("r").map(column_of) {
                Some("B") => b = cell_text(cell, strings),
                Some("C") => c = cell_text(cell, strings),
                _ => {}
            }
        }
        let Some(code) = b.map(|s| s.trim().to_owned()) else {
            continue;
        };
        if code.is_empty() || !code.bytes().all(|x| x.is_ascii_digit()) {
            continue;
        }
        let code: u16 = code
            .parse()
            .map_err(|_| CodebookError(format!("コード {code} が大きすぎます")))?;
        sheet.insert(code, c.unwrap_or_default().trim().to_owned());
    }
    Ok(sheet)
}

/// XLSXから3シートを抽出する。
pub fn read_code_sheets(bytes: &[u8]) -> Result<CodeSheets, CodebookError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(xlsx_error)?;
    let workbook = read_entry(&mut archive, "xl/workbook.xml")?
        .ok_or_else(|| xlsx_error("xl/workbook.xmlがありません"))?;
    let rels = read_entry(&mut archive, "xl/_rels/workbook.xml.rels")?
        .ok_or_else(|| xlsx_error("workbook.xml.relsがありません"))?;
    let strings = shared_strings(read_entry(&mut archive, "xl/sharedStrings.xml")?.as_deref())?;
    let paths = sheet_paths(&workbook, &rels)?;
    let mut sheet = |name: &str| -> Result<CodeSheet, CodebookError> {
        let path = paths
            .get(name)
            .ok_or_else(|| CodebookError(format!("シート {name} がありません")))?;
        let xml = read_entry(&mut archive, path)?
            .ok_or_else(|| CodebookError(format!("シート {name} の本体がありません")))?;
        code_sheet(&xml, &strings)
    };
    Ok(CodeSheets {
        weather: sheet(WEATHER_SHEET)?,
        party: sheet(PARTY_SHEET)?,
        injury: sheet(INJURY_SHEET)?,
    })
}

/// 照合したコード表のファイル。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodebookFile {
    pub source_year: u16,
    pub file: String,
    pub sha256: String,
}

/// コード表の照合器。前処理は照合器を引数にとり、CLIは常に [`FileCodebooks`] を使う。
pub trait CodebookVerifier {
    /// 照合したXLSXとPDFの一覧（年昇順）を返す。不一致・欠損は失敗。
    fn verify(
        &self,
        accident_dir: &Path,
    ) -> Result<(Vec<CodebookFile>, Vec<CodebookFile>), CodebookError>;
}

/// `accident/` のPDF・XLSXを実際に照合する。
#[derive(Debug, Clone, Copy, Default)]
pub struct FileCodebooks;

impl CodebookVerifier for FileCodebooks {
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
        let mut xlsxs = Vec::new();
        for year in XLSX_YEARS {
            let file = format!("codebook_{year}.xlsx");
            let bytes = read(&file)?;
            let sheets = read_code_sheets(&bytes)?;
            verify_code_sheets(year, &sheets)?;
            xlsxs.push(CodebookFile {
                source_year: year,
                sha256: sha256_hex(&bytes),
                file,
            });
        }
        Ok((xlsxs, pdfs))
    }
}
