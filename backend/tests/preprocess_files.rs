//! 原本CSVのファイル単位の処理とコード表の照合（behavior.md 2章、#8 ステップ4・5）。

mod common;

use std::io::Write;

use accident_route_backend::data::holidays::parse_holiday_csv;
use accident_route_backend::data::preprocess::codebook::{
    CodeSheets, CodebookVerifier, FileCodebooks, read_code_sheets, verify_code_sheets,
};
use accident_route_backend::data::preprocess::rows::{ExclusionReason, party_codes};
use accident_route_backend::data::preprocess::{
    PreprocessError, SOURCE_YEARS, prepare_release, process_file, source_file_name,
};
use accident_route_backend::data::release::AccidentCsvWriter;
use accident_route_backend::domain::HolidayCalendar;

use common::source::{AcceptAll, HEADER, HOLIDAYS, SourceRow, cp932, csv_text, write_source_dir};

fn calendar() -> HolidayCalendar {
    parse_holiday_csv(HOLIDAYS.as_bytes()).unwrap()
}

fn run(
    bytes: &[u8],
) -> Result<(accident_route_backend::data::preprocess::FileAudit, u64), PreprocessError> {
    let mut writer = AccidentCsvWriter::new();
    let audit = process_file("honhyo_2025.csv", bytes, 2025, &calendar(), &mut writer)?;
    Ok((audit, writer.rows()))
}

#[test]
fn test_encoding_utf8_cp932_and_bom() {
    let text = csv_text(&HEADER, &[SourceRow::valid(2025, 1, 6, 12)], None);
    let utf8 = text.as_bytes().to_vec();
    let bom = [b"\xEF\xBB\xBF".as_slice(), text.as_bytes()].concat();
    for (bytes, encoding) in [
        (utf8, "utf-8-sig"),
        (bom, "utf-8-sig"),
        (cp932(&text), "cp932"),
    ] {
        let (audit, rows) = run(&bytes).unwrap();
        assert_eq!(audit.encoding, encoding);
        assert_eq!((audit.input_rows, audit.retained_rows, rows), (1, 1, 1));
    }
}

#[test]
fn decode_error_after_the_first_five_lines_fails_the_file() {
    let rows: Vec<SourceRow> = (0..6).map(|_| SourceRow::valid(2025, 1, 6, 12)).collect();
    let mut bytes = cp932(&csv_text(&HEADER, &rows, None));
    // 7行目（データ6行目）に復号できないバイトを入れる。行の除外にしない
    bytes.extend_from_slice(b"\x81\x20,\r\n");
    let error = run(&bytes).unwrap_err();
    assert!(matches!(error, PreprocessError::File { .. }), "{error}");
}

#[test]
fn headers_are_matched_by_name_with_whitespace_removed() {
    let rows = [SourceRow::valid(2025, 1, 6, 12)];
    let mut order: Vec<usize> = (0..HEADER.len()).collect();
    order.reverse();
    let (audit, _) = run(csv_text(&HEADER, &rows, Some(&order)).as_bytes()).unwrap();
    assert_eq!(audit.retained_rows, 1);
    // 全角・半角空白の違いは同じ列名
    let mut header = HEADER;
    header[3] = "発生日時 年";
    let (audit, _) = run(csv_text(&header, &rows, None).as_bytes()).unwrap();
    assert_eq!(audit.retained_rows, 1);
}

#[test]
fn missing_or_duplicate_required_columns_fail() {
    let rows = [SourceRow::valid(2025, 1, 6, 12)];
    let mut missing = HEADER;
    missing[9] = "天気";
    assert!(matches!(
        run(csv_text(&missing, &rows, None).as_bytes()),
        Err(PreprocessError::File { .. })
    ));
    let mut duplicate = HEADER;
    duplicate[0] = "天　候";
    assert!(matches!(
        run(csv_text(&duplicate, &rows, None).as_bytes()),
        Err(PreprocessError::File { .. })
    ));
}

#[test]
fn excluded_rows_are_counted_by_first_reason() {
    let ok = || SourceRow::valid(2025, 1, 6, 12);
    let rows = vec![
        ok(),
        ok().set("地点　緯度（北緯）", "0"),
        ok().set("天候", "9"),
        ok().set("当事者種別（当事者B）", "99"),
        ok().set("発生日時　　時", "99"),
        ok().set("発生日時　　月", "00"),
        ok().set("発生日時　　月", "00").set("天候", "9"),
        ok().set("当事者種別（当事者A）", "36")
            .set("当事者種別（当事者B）", "00"), // modes=0は保持
        ok().set("死者数", "１"),
    ];
    let (audit, written) = run(csv_text(&HEADER, &rows, None).as_bytes()).unwrap();
    assert_eq!(audit.input_rows, 9);
    assert_eq!(audit.retained_rows, 2);
    assert_eq!(audit.excluded_rows, 7);
    assert_eq!(audit.input_rows, audit.retained_rows + audit.excluded_rows);
    assert_eq!(written, 2);
    assert_eq!(audit.no_target_mode, 1);
    use ExclusionReason::*;
    let count = |r| audit.excluded_by_reason.get(&r).copied().unwrap_or(0);
    assert_eq!(count(InvalidCoordinates), 1);
    assert_eq!(count(InvalidDatetime), 3);
    assert_eq!(count(UnknownWeatherCode), 1);
    assert_eq!(count(UnknownPartyCode), 1);
    assert_eq!(count(InvalidCasualties), 1);
}

#[test]
fn quoted_fields_and_newlines_are_parsed_as_csv() {
    let mut text = csv_text(&HEADER, &[SourceRow::valid(2025, 1, 6, 12)], None);
    // 引用符で囲んだ改行を含む値（必須列以外）
    let row = SourceRow::valid(2025, 1, 7, 12).set("資料区分", "\"1\r\n2\"");
    text.push_str(
        csv_text(&HEADER, &[row], None)
            .split_once("\r\n")
            .unwrap()
            .1,
    );
    let (audit, _) = run(text.as_bytes()).unwrap();
    assert_eq!((audit.input_rows, audit.retained_rows), (2, 2));
}

#[test]
fn unterminated_quote_fails_the_file() {
    // 引用符が閉じないまま終わると、CSVパーサは残りを1つのフィールドにして行を失う
    let mut text = csv_text(&HEADER, &[SourceRow::valid(2025, 1, 6, 12)], None);
    text.push_str("\"1,");
    let next = csv_text(&HEADER, &[SourceRow::valid(2025, 1, 7, 12)], None);
    text.push_str(next.split_once("\r\n").unwrap().1);
    assert!(matches!(
        run(text.as_bytes()),
        Err(PreprocessError::File { .. })
    ));
    // 閉じた引用符・引用中の "" ・引用を閉じた後の文字は構文の不正ではない
    use accident_route_backend::data::csv_syntax::ends_inside_quoted_field;
    for ok in [
        "a,\"b\"\n",
        "a,\"b\"\"c\"\n",
        "a,\"b\nc\"\n",
        "a,b\"c\n",
        "a,\"b\"c\n",
        "",
    ] {
        assert!(!ends_inside_quoted_field(ok), "{ok:?}");
    }
    for bad in ["a,\"b\n", "a,\"b\"\"\n", "\""] {
        assert!(ends_inside_quoted_field(bad), "{bad:?}");
    }
}

#[test]
fn whole_run_fails_on_missing_year_or_no_retained_rows() {
    let dir = tempfile::tempdir().unwrap();
    write_source_dir(dir.path(), |year| vec![SourceRow::valid(year, 1, 6, 12)]);
    std::fs::remove_file(dir.path().join(source_file_name(2022))).unwrap();
    let error = prepare_release(dir.path(), HOLIDAYS.as_bytes().to_vec(), &AcceptAll, "t")
        .err()
        .unwrap();
    assert!(matches!(error, PreprocessError::Io { .. }), "{error}");

    let dir = tempfile::tempdir().unwrap();
    write_source_dir(dir.path(), |year| {
        vec![SourceRow::valid(year, 1, 6, 12).set("天候", "9")]
    });
    let error = prepare_release(dir.path(), HOLIDAYS.as_bytes().to_vec(), &AcceptAll, "t")
        .err()
        .unwrap();
    assert!(matches!(error, PreprocessError::NoRetainedRows), "{error}");
}

fn matching_sheets(year: u16) -> CodeSheets {
    let mut sheets = CodeSheets::default();
    for (code, name) in [(1, "晴"), (2, "曇"), (3, "雨"), (4, "霧"), (5, "雪")] {
        sheets.weather.insert(code, name.to_owned());
    }
    for (code, name) in [
        (0, "対象外当事者"),
        (1, "死亡"),
        (2, "負傷"),
        (4, "損傷なし"),
    ] {
        sheets.injury.insert(code, name.to_owned());
    }
    for code in party_codes(year) {
        let name = match code {
            1..=7 => "乗用車－普通車",
            11..=17 => "貨物車－普通車",
            21..=24 => "特殊車－大型",
            51 => "軽車両－自転車",
            52 => "軽車両－駆動補助機付自転車",
            61 => "歩行者",
            _ => "その他",
        };
        sheets.party.insert(code, name.to_owned());
    }
    sheets
}

#[test]
fn code_sheet_comparison() {
    for year in 2022..=2025 {
        verify_code_sheets(year, &matching_sheets(year)).unwrap();
    }
    // 43の有無の年度差
    assert!(verify_code_sheets(2023, &matching_sheets(2024)).is_err());
    assert!(verify_code_sheets(2024, &matching_sheets(2023)).is_err());
    // 説明の先頭が違う
    let mut s = matching_sheets(2025);
    s.party.insert(51, "自転車".to_owned());
    assert!(verify_code_sheets(2025, &s).is_err());
    let mut s = matching_sheets(2025);
    s.weather.insert(4, "雪".to_owned());
    assert!(verify_code_sheets(2025, &s).is_err());
    let mut s = matching_sheets(2025);
    s.injury.remove(&4);
    assert!(verify_code_sheets(2025, &s).is_err());
    // 天候・損傷の余分なコードは拒否しない
    let mut s = matching_sheets(2025);
    s.weather.insert(9, "不明".to_owned());
    s.injury.insert(9, "不明".to_owned());
    verify_code_sheets(2025, &s).unwrap();
}

/// 3シートだけの最小のXLSX。sharedStringsにrich textとふりがな（rPh）を含める。
fn tiny_xlsx() -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let ns = r#"xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
    let files = [
        (
            "xl/workbook.xml".to_owned(),
            format!(
                r#"<workbook {ns}><sheets><sheet name="天候" sheetId="1" r:id="rId1"/><sheet name="当事者種別" sheetId="2" r:id="rId2"/><sheet name="人身損傷程度" sheetId="3" r:id="rId3"/></sheets></workbook>"#
            ),
        ),
        (
            "xl/_rels/workbook.xml.rels".to_owned(),
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Target="worksheets/sheet9.xml"/><Relationship Id="rId2" Target="/xl/worksheets/sheet2.xml"/><Relationship Id="rId3" Target="worksheets/sheet3.xml"/></Relationships>"#.to_owned(),
        ),
        (
            "xl/sharedStrings.xml".to_owned(),
            format!(
                r#"<sst {ns}><si><t>コード</t></si><si><t>晴</t><rPh sb="0" eb="1"><t>ハレ</t></rPh></si><si><r><t>歩行</t></r><r><t>者</t></r></si></sst>"#
            ),
        ),
        (
            "xl/worksheets/sheet9.xml".to_owned(),
            format!(
                r#"<worksheet {ns}><sheetData><row r="6"><c r="B6" t="s"><v>0</v></c></row><row r="7"><c r="B7" t="inlineStr"><is><t>1</t></is></c><c r="C7" t="s"><v>1</v></c></row><row r="8"><c r="B8"><v>2</v></c><c r="C8" t="inlineStr"><is><t>曇</t></is></c></row></sheetData></worksheet>"#
            ),
        ),
        (
            "xl/worksheets/sheet2.xml".to_owned(),
            format!(
                r#"<worksheet {ns}><sheetData><row r="8"><c r="B8" t="str"><v>61</v></c><c r="C8" t="s"><v>2</v></c></row><row r="9"><c r="B9" t="inlineStr"><is><t>01</t></is></c><c r="C9" t="inlineStr"><is><t>乗用車－大型車</t></is></c></row></sheetData></worksheet>"#
            ),
        ),
        (
            "xl/worksheets/sheet3.xml".to_owned(),
            format!(r#"<worksheet {ns}><sheetData/></worksheet>"#),
        ),
    ];
    for (name, body) in files {
        zip.start_file(name, options).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn xlsx_sheets_are_read_through_workbook_relationships() {
    let sheets = read_code_sheets(&tiny_xlsx()).unwrap();
    assert_eq!(
        sheets.weather.into_iter().collect::<Vec<_>>(),
        [(1, "晴".to_owned()), (2, "曇".to_owned())]
    );
    assert_eq!(
        sheets.party.into_iter().collect::<Vec<_>>(),
        [(1, "乗用車－大型車".to_owned()), (61, "歩行者".to_owned())]
    );
    assert!(sheets.injury.is_empty());
}

/// 実コード表の照合と各年度の先頭行。`accident/` がなければ省略を明示する（合格と混同しない）。
#[test]
fn test_real_codebooks_and_row_from_each_year() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../accident");
    if !dir.join(source_file_name(2025)).exists() {
        eprintln!(
            "SKIP: {} に原本がないため実データの試験を省略（合格ではない）",
            dir.display()
        );
        return;
    }
    let (xlsx, pdf) = FileCodebooks.verify(&dir).expect("実コード表の照合");
    assert_eq!(
        xlsx.iter().map(|c| c.source_year).collect::<Vec<_>>(),
        [2022, 2023, 2024, 2025]
    );
    assert_eq!(
        pdf.iter().map(|c| c.source_year).collect::<Vec<_>>(),
        [2019, 2020, 2021]
    );
    // 天候集合の完全一致と43の年度差
    for year in 2022..=2025 {
        let bytes = std::fs::read(dir.join(format!("codebook_{year}.xlsx"))).unwrap();
        let sheets = read_code_sheets(&bytes).unwrap();
        assert_eq!(
            sheets.weather.keys().copied().collect::<Vec<_>>(),
            [1, 2, 3, 4, 5],
            "{year}"
        );
        assert_eq!(sheets.party.contains_key(&43), year >= 2024, "{year}");
    }

    let holidays = std::fs::read(dir.join("syukujitsu.csv")).expect("祝日表");
    let calendar = parse_holiday_csv(&holidays).unwrap();
    // 各年度の先頭行（2019〜2025）。先頭6行だけを処理する
    // (source_year, 緯度, 発生年, modes)。原本の先頭データ行をDMSと当事者種別から手で計算した値
    let expected = [
        (2019, 43.236_933_055_555_55, 2018, 4),
        (2020, 42.943_723_055_555_55, 2020, 4),
        (2021, 42.815_867_777_777_775, 2020, 4),
        (2022, 43.042_996_944_444_44, 2022, 4),
        (2023, 43.168_785_555_555_55, 2022, 4),
        (2024, 43.102_108_333_333_334, 2023, 4),
        (2025, 43.102_108_333_333_334, 2025, 4),
    ];
    for (year, lat, occurred, modes) in expected {
        assert!(SOURCE_YEARS.contains(&year));
        let bytes = std::fs::read(dir.join(source_file_name(year))).unwrap();
        let head: Vec<&[u8]> = bytes.split_inclusive(|&b| b == b'\n').take(2).collect();
        let mut writer = AccidentCsvWriter::new();
        let audit = process_file("head", &head.concat(), year, &calendar, &mut writer).unwrap();
        assert_eq!(audit.encoding, "cp932", "{year}");
        assert_eq!(audit.retained_rows, 1, "{year}");
        let gzip = writer.finish().gzip;
        let columns = accident_route_backend::data::load::parse_accidents(&gzip).unwrap();
        assert!(
            (columns.lat[0] - lat).abs() < 1e-6,
            "{year}: {}",
            columns.lat[0]
        );
        assert_eq!(columns.source_year[0], year);
        assert_eq!(
            columns.date[0] / 10_000,
            occurred,
            "{year}: 発生年≠source_year を保つ"
        );
        assert_eq!(columns.modes[0], modes, "{year}");
        assert!(columns.injuries[0] >= 1 || columns.deaths[0] >= 1);
    }
}
