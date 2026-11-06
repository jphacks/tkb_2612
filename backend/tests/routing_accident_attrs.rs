//! 事故の属性の前処理（形式書12章、作業書M2a）：`accident_attrs.bin` の事故IDへの対応、
//! encode/decodeの往復、欠損・未知コード、年ごとの列・コードの違い、監査。

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use accident_route_backend::data::binfile::{BinFile, BinFileWriter};
use accident_route_backend::data::preprocess::attrs::{
    AttrCodebooks, AttrSheets, AttrsError, build_accident_attrs, verify_attr_sheets,
};
use accident_route_backend::data::preprocess::codebook::{CodebookVerifier, read_sheet_rows};
use accident_route_backend::data::preprocess::{PreprocessError, prepare_release};
use accident_route_backend::data::release::{
    ACCIDENT_ATTRS_FILE, ACCIDENT_ATTRS_MAGIC, ACCIDENTS_FILE, sha256_hex, write_release_files,
};
use accident_route_backend::data::routing_files::{
    Deps, decode_accident_attrs, encode_accident_attrs,
};
use accident_route_backend::domain::routing::accident::{
    AccidentAttr, MISSING_U8, MISSING_U16, RouteClass,
};
use serde_json::{Value, json};

use common::source::{AcceptAll, HEADER, HOLIDAYS, SourceRow, cp932};

/// 属性の列。全空白を除いた名前が `路線コード`・`道路形状`・`車道幅員`・`歩車道区分`・`地形`。
const ATTR_HEADER: [&str; 5] = [
    "路線　コード",
    "道路形状",
    "車道 幅員",
    "歩車道区分",
    "地　形",
];

fn attr(
    upper: u16,
    lower: u8,
    class: u8,
    shape: u8,
    width: u8,
    sidewalk: u8,
    terrain: u8,
) -> AccidentAttr {
    AccidentAttr {
        route_upper: upper,
        route_lower: lower,
        route_class: class,
        road_shape: shape,
        carriageway_width: width,
        sidewalk_division: sidewalk,
        terrain,
    }
}

/// 原本の1行と、そこから期待する属性（保持される行だけ）。
struct Case {
    row: SourceRow,
    /// 属性の列の値。`None` は属性の列を持たない短い行。
    attrs: Option<[&'static str; 5]>,
    /// 保持される行の期待値。除外される行は `None`。
    expect: Option<AccidentAttr>,
}

const MISSING: AccidentAttr = AccidentAttr::MISSING;

/// 年ごとの原本。除外される行を間に挟み、事故IDが保持行の通し番号であることを確かめる。
fn cases(year: u16) -> Vec<Case> {
    let row = |day: u32| SourceRow::valid(year, 1, day, 12);
    let case = |day: u32, attrs: [&'static str; 5], expect: AccidentAttr| Case {
        row: row(day),
        attrs: Some(attrs),
        expect: Some(expect),
    };
    // 2022年のコード表だけにある車道幅員のコード（12・13・16）は、2024年のコードに写す
    let width = |code22: u8| if year == 2022 { code22 } else { MISSING_U8 };
    let mut cases = vec![
        case(
            1,
            ["30000", "01", "15", "2", "1"],
            attr(3000, 0, 4, 1, 15, 2, 1),
        ),
        // 除外される行（天候が範囲外）。属性は別の値にして、混ざらないことを確かめる
        Case {
            row: row(2).set("天候", "9"),
            attrs: Some(["00010", "14", "03", "1", "2"]),
            expect: None,
        },
        // 国道17号のバイパス区間
        case(
            3,
            ["00171", "14", "03", "4", "3"],
            attr(17, 1, 0, 14, 3, 4, 3),
        ),
        // 高速自動車国道のコード（4000〜4999）
        case(
            4,
            ["49990", "11", "01", "3", "2"],
            attr(4999, 0, 5, 11, 1, 3, 2),
        ),
        // 上4桁が表にない（9501〜9899）：上4桁と下1桁は読み、route_classは未知。ほかの属性も表にない値
        case(
            5,
            ["97003", "02", "07", "0", "4"],
            attr(
                9700, 3, MISSING_U8, MISSING_U8, MISSING_U8, MISSING_U8, MISSING_U8,
            ),
        ),
        // 空の値：欠損
        case(6, ["", "", "", "", ""], MISSING),
        // 数字でない路線コードは未知（上4桁・下1桁・route_classとも欠損・未知の値）
        case(
            7,
            ["abc", "1", "0", "9", "x"],
            attr(
                MISSING_U16,
                MISSING_U8,
                MISSING_U8,
                1,
                0,
                MISSING_U8,
                MISSING_U8,
            ),
        ),
        // 2022年のコード表では12は14と同じ区分。ほかの年にはない
        case(
            8,
            ["30000", "01", "12", "2", "1"],
            attr(
                3000,
                0,
                4,
                1,
                if year == 2022 { 14 } else { MISSING_U8 },
                2,
                1,
            ),
        ),
    ];
    if year == 2022 {
        cases.push(case(
            9,
            ["30000", "01", "13", "2", "1"],
            attr(3000, 0, 4, 1, width(17), 2, 1),
        ));
        cases.push(case(
            10,
            ["30000", "01", "16", "2", "1"],
            attr(3000, 0, 4, 1, width(18), 2, 1),
        ));
    }
    if year == 2025 {
        // 属性の列まで届かない短い行：欠損
        cases.push(Case {
            row: row(11),
            attrs: None,
            expect: Some(MISSING),
        });
    }
    cases
}

/// 属性の列を足した原本のCSV。2025年だけ列の並びをそのまま（属性の列が末尾）にし、
/// ほかの年は並びを回して、列の位置を固定していないことを確かめる。
fn write_sources(dir: &Path) {
    write_sources_with(dir, |_, _| {});
}

/// `mutate(year, cases)` で年ごとの行を書き換えてから書く。
fn write_sources_with(dir: &Path, mutate: impl Fn(u16, &mut Vec<Case>)) {
    std::fs::create_dir_all(dir).unwrap();
    for year in 2019..=2025u16 {
        let header: Vec<&str> = HEADER.iter().copied().chain(ATTR_HEADER).collect();
        let mut order: Vec<usize> = (0..header.len()).collect();
        if year != 2025 {
            order.rotate_left(usize::from(year - 2019) + 3);
        }
        let join = |fields: Vec<&str>| fields.join(",");
        let mut text = join(order.iter().map(|&i| header[i]).collect());
        text.push_str("\r\n");
        let mut year_cases = cases(year);
        mutate(year, &mut year_cases);
        for case in year_cases {
            let mut fields: Vec<&str> = case.row.0.iter().map(String::as_str).collect();
            match case.attrs {
                Some(attrs) => fields.extend(attrs),
                // 短い行は並びを回していない年（2025年）だけ
                None => assert_eq!(year, 2025),
            }
            text.push_str(&join(
                order
                    .iter()
                    .filter_map(|&i| fields.get(i).copied())
                    .collect(),
            ));
            text.push_str("\r\n");
        }
        std::fs::write(dir.join(format!("honhyo_{year}.csv")), cp932(&text)).unwrap();
    }
}

/// 原本から既存の前処理でschema 2のreleaseを作る。
fn make_release(source: &Path, out: &Path) -> Value {
    let prepared = prepare_release(
        source,
        HOLIDAYS.as_bytes().to_vec(),
        &AcceptAll,
        "2026-11-03T09:00:00+09:00",
    )
    .unwrap();
    write_release_files(out, &prepared.release).unwrap();
    prepared.release.metadata
}

fn expected_attrs() -> Vec<AccidentAttr> {
    (2019..=2025u16)
        .flat_map(cases)
        .filter_map(|c| c.expect)
        .collect()
}

struct Fixture {
    _source: tempfile::TempDir,
    _release: tempfile::TempDir,
    source: std::path::PathBuf,
    release: std::path::PathBuf,
    metadata: Value,
}

fn fixture() -> Fixture {
    let source = tempfile::tempdir().unwrap();
    let release = tempfile::tempdir().unwrap();
    write_sources(source.path());
    let metadata = make_release(source.path(), release.path());
    Fixture {
        source: source.path().to_owned(),
        release: release.path().to_owned(),
        _source: source,
        _release: release,
        metadata,
    }
}

#[test]
fn attributes_follow_the_retained_rows_in_accident_id_order() {
    let f = fixture();
    let out = build_accident_attrs(&f.source, &f.release, &AcceptAll).unwrap();
    let expected = expected_attrs();
    // 保持行の通し番号（事故ID）の順。除外した行は数えない
    assert_eq!(out.table.attrs, expected);
    assert_eq!(f.metadata["rows"], expected.len());
    assert_eq!(out.table.params["rows"], expected.len());
    // 17列の事故CSVを作り直すと、既存のprepare_releaseと同じバイト列になる
    assert!(out.gzip_identical);
    assert_eq!(out.dataset_id, f.metadata["dataset_id"].as_str().unwrap());
    assert_eq!(
        out.dataset_id,
        sha256_hex(&std::fs::read(f.release.join(ACCIDENTS_FILE)).unwrap())
    );
    let mut deps = BTreeMap::new();
    deps.insert(ACCIDENTS_FILE.to_owned(), out.dataset_id.clone());
    assert_eq!(out.deps, deps);
    // 値がformat書12章の範囲に収まる
    for attr in &out.table.attrs {
        attr.validate().unwrap();
    }
}

#[test]
fn missing_and_unknown_values_are_the_sentinels_of_the_format() {
    let f = fixture();
    let out = build_accident_attrs(&f.source, &f.release, &AcceptAll).unwrap();
    // 2019年の保持行（ID 0〜6）：0 通常、1 国道17号のバイパス、2 4999、3 上4桁が表にない、
    // 4 空の値だけの行、5 数字でない路線コード、6 車道幅員が2022年のコード
    let all_missing = out.table.attrs[4];
    assert_eq!(all_missing, AccidentAttr::MISSING);
    assert_eq!(all_missing.route_upper, 0xFFFF);
    assert_eq!(
        [
            all_missing.route_lower,
            all_missing.route_class,
            all_missing.road_shape,
            all_missing.carriageway_width,
            all_missing.sidewalk_division,
            all_missing.terrain
        ],
        [0xFF; 6]
    );
    assert!(!all_missing.is_motorway_code());
    // 4000〜4999は「高速・自専道のコードの事故」
    let expressway = out.table.attrs[2];
    assert_eq!(expressway.route_class_kind(), Some(RouteClass::Expressway));
    assert!(expressway.is_motorway_code());
    // 上4桁が表にない値は、route_classだけが未知（上4桁・下1桁は読んだ値のまま）
    let unknown_class = out.table.attrs[3];
    assert_eq!(unknown_class.route_upper, 9700);
    assert_eq!(unknown_class.route_lower, 3);
    assert_eq!(unknown_class.route_class, MISSING_U8);
    assert!(!unknown_class.is_motorway_code());
    // 数字でない路線コードは、上4桁・下1桁・route_classとも欠損・未知の値
    let not_digits = out.table.attrs[5];
    assert_eq!(
        (
            not_digits.route_upper,
            not_digits.route_lower,
            not_digits.route_class
        ),
        (MISSING_U16, MISSING_U8, MISSING_U8)
    );
    // 2022年のコード表にだけある車道幅員のコードは、ほかの年では未知
    assert_eq!(out.table.attrs[6].carriageway_width, MISSING_U8);
}

#[test]
fn audit_counts_distribution_missing_unknown_and_remapped_codes_per_year() {
    let f = fixture();
    let out = build_accident_attrs(&f.source, &f.release, &AcceptAll).unwrap();
    let years = &out.table.params["years"];
    let y2019 = &years["2019"];
    let y2022 = &years["2022"];
    let y2025 = &years["2025"];

    // 行数は既存の前処理の監査（metadataのfiles）と同じ
    for (i, year) in (2019..=2025u16).enumerate() {
        let file = &f.metadata["files"][i];
        let mine = &years[year.to_string()];
        assert_eq!(file["source_year"], year);
        assert_eq!(mine["input_rows"], file["input_rows"], "{year}");
        assert_eq!(mine["retained_rows"], file["retained_rows"], "{year}");
        assert_eq!(mine["excluded_rows"], file["excluded_rows"], "{year}");
        assert_eq!(mine["rows"], file["retained_rows"], "{year}");
        assert_eq!(mine["sha256"], file["sha256"], "{year}");
        assert_eq!(mine["encoding"], "cp932", "{year}");
        // 属性の列の位置は年ごとに違う（並びを回した）が、すべて見つかっている
        assert_eq!(mine["header_columns"], 23, "{year}");
        let positions: Vec<u64> = ["路線コード", "道路形状", "車道幅員", "歩車道区分", "地形"]
            .iter()
            .map(|name| mine["column_positions"][name].as_u64().unwrap())
            .collect();
        assert!(positions.iter().all(|p| *p < 23), "{year}");
    }
    assert_ne!(
        years["2019"]["column_positions"]["路線コード"],
        years["2020"]["column_positions"]["路線コード"]
    );
    assert_eq!(y2025["column_positions"]["路線コード"], 18);
    assert_eq!(y2025["column_positions"]["地形"], 22);

    // 2019年：車道幅員の「07」「12」（2022年以外では表にない）の2件が未知、空の1件が欠損
    assert_eq!(y2019["missing"]["carriageway_width"], 1);
    assert_eq!(y2019["unknown"]["carriageway_width"], 2);
    assert_eq!(y2019["unknown_values"]["carriageway_width"]["07"], 1);
    assert_eq!(y2019["unknown_values"]["carriageway_width"]["12"], 1);
    assert_eq!(y2019["width_remapped"], json!({}));
    // 路線コード：空が1件欠損、数字でない1件が未知、9700は区分だけ未知
    assert_eq!(y2019["missing"]["route"], 1);
    assert_eq!(y2019["unknown"]["route"], 1);
    assert_eq!(y2019["unknown"]["route_class"], 1);
    assert_eq!(y2019["unknown_values"]["route_class"]["9700"], 1);
    assert_eq!(y2019["route_lower_bypass"], 2); // 00171と97003
    assert_eq!(y2019["distribution"]["route_class"]["4"], 2); // 30000が2件
    assert_eq!(y2019["distribution"]["route_class"]["5"], 1);
    assert_eq!(y2019["distribution"]["route_class"]["0"], 1);
    assert_eq!(y2019["distribution"]["road_shape"]["14"], 1);

    // 2022年：12・13・16は2024年のコード14・17・18に写し、未知にしない
    assert_eq!(
        y2022["width_remapped"],
        json!({"12->14": 1, "13->17": 1, "16->18": 1})
    );
    assert_eq!(y2022["unknown"]["carriageway_width"], 1); // 「07」だけ
    assert_eq!(y2022["distribution"]["carriageway_width"]["14"], 1);
    assert_eq!(y2022["distribution"]["carriageway_width"]["17"], 1);
    assert_eq!(y2022["distribution"]["carriageway_width"]["18"], 1);
    assert_eq!(
        y2022["carriageway_width_aliases"],
        json!({"12": 14, "13": 17, "16": 18})
    );
    assert_eq!(y2019["carriageway_width_aliases"], json!({}));
    // 2025年：短い行は欠損
    assert_eq!(y2025["missing"]["terrain"], 2);
    assert_eq!(y2025["missing"]["route"], 2);

    // 合計は年ごとの合計
    let totals = &out.table.params["totals"];
    assert_eq!(totals["rows"], f.metadata["rows"]);
    let sum = |section: &str, key: &str| -> u64 {
        (2019..=2025u16)
            .map(|y| years[y.to_string()][section][key].as_u64().unwrap())
            .sum()
    };
    assert_eq!(totals["missing"]["route"], sum("missing", "route"));
    assert_eq!(
        totals["unknown"]["road_shape"],
        sum("unknown", "road_shape")
    );
    assert_eq!(
        totals["unknown"]["carriageway_width"],
        sum("unknown", "carriageway_width")
    );

    // コード表の根拠
    assert_eq!(y2019["codebook"]["check"], "pdf_exists_only");
    assert_eq!(y2022["codebook"]["check"], "xlsx");
    // 計画書4.2節の表との照合（2024年）：人工データでは割合は表と離れるが、構造と件数は出る
    let check = &out.audit["plan_4_2_check"];
    assert_eq!(check["source_year"], 2024);
    assert_eq!(check["rows"], years["2024"]["retained_rows"]);
    assert_eq!(check["route_class"].as_array().unwrap().len(), 5);
    assert_eq!(check["road_shape"].as_array().unwrap().len(), 4);
    assert_eq!(check["route_class"][1]["name"], "国道");
    assert_eq!(check["route_class"][1]["count"], 1);
    assert_eq!(check["route_class"][4]["name"], "高速・自専道");
    assert_eq!(check["route_class"][4]["count"], 1);
    // auditにはPARAMSの内容がすべて入っている
    for key in ["years", "totals", "rows", "codebooks", "legend", "columns"] {
        assert_eq!(out.audit[key], out.table.params[key], "{key}");
    }
}

#[test]
fn encode_decode_round_trip_and_layout() {
    let f = fixture();
    let out = build_accident_attrs(&f.source, &f.release, &AcceptAll).unwrap();
    let bytes = encode_accident_attrs(&out.table, &out.deps);
    // 同じ入力からは同じバイト列
    assert_eq!(bytes, encode_accident_attrs(&out.table, &out.deps));

    let decoded = decode_accident_attrs(&bytes).unwrap();
    assert_eq!(decoded.value, out.table);
    assert_eq!(decoded.deps, out.deps);

    // 形式書3章・12章のレイアウト：マジックナンバー、節の順、elem_size 8、payload_sha256
    assert_eq!(&bytes[0..8], b"TKRACATR");
    assert_eq!(ACCIDENT_ATTRS_MAGIC, *b"TKRACATR");
    let file = BinFile::parse(&bytes).unwrap();
    file.expect(*b"TKRACATR", 1).unwrap();
    file.verify_payload_sha256().unwrap();
    assert_eq!(file.section_count(), 3);
    let counts = file.section_counts();
    assert_eq!(counts["ATTR"], out.table.attrs.len() as u64);
    let section = file.require("ATTR").unwrap();
    assert_eq!(section.elem_size, 8);
    // 先頭のレコード（上4桁3000=0x0BB8のリトルエンディアン、下1桁0、route_class 4、道路形状1、車道幅員15、歩車道区分2、地形1）
    assert_eq!(section.bytes[..8], [0xB8, 0x0B, 0, 4, 1, 15, 2, 1]);
    // 空の値だけの行（ID 4）のレコードは全バイト0xFF
    assert_eq!(section.bytes[4 * 8..5 * 8], [0xFF; 8]);
    assert_eq!(file.deps().unwrap(), out.deps);
    assert_eq!(file.json("PARAMS").unwrap(), out.table.params);
}

fn bin_with(attrs: &[AccidentAttr], rows: Option<u64>, deps: &Deps, elem_size: u32) -> Vec<u8> {
    let mut writer = BinFileWriter::new(*b"TKRACATR", 1);
    writer.add_json("DEPS", &serde_json::to_value(deps).unwrap());
    writer.add_json("PARAMS", &json!({ "rows": rows }));
    let mut bytes = Vec::new();
    for attr in attrs {
        bytes.extend_from_slice(&attr.to_bytes());
    }
    // elem_sizeを変える試験では、バイト列の長さを合わせる
    let count = bytes.len() as u64 / u64::from(elem_size);
    assert_eq!(count * u64::from(elem_size), bytes.len() as u64);
    writer.add_section("ATTR", elem_size, count, bytes);
    writer.finish()
}

#[test]
fn decode_rejects_inconsistent_files() {
    let ok_attr = attr(3000, 0, 4, 1, 15, 2, 1);
    let deps: Deps = [(ACCIDENTS_FILE.to_owned(), "a".repeat(64))].into();
    let good = bin_with(&[ok_attr, ok_attr], Some(2), &deps, 8);
    assert_eq!(decode_accident_attrs(&good).unwrap().value.attrs.len(), 2);

    // 件数がPARAMSのrowsと違う
    assert!(decode_accident_attrs(&bin_with(&[ok_attr], Some(2), &deps, 8)).is_err());
    // rowsがない
    assert!(decode_accident_attrs(&bin_with(&[ok_attr], None, &deps, 8)).is_err());
    // elem_sizeが8でない
    assert!(decode_accident_attrs(&bin_with(&[ok_attr, ok_attr], Some(4), &deps, 4)).is_err());
    // DEPSにaccidents.csv.gzがない
    assert!(decode_accident_attrs(&bin_with(&[ok_attr], Some(1), &Deps::new(), 8)).is_err());
    // route_classが上4桁と食い違う／コード表にない値
    for bad in [
        attr(3000, 0, 5, 1, 15, 2, 1),
        attr(3000, 0, 4, 2, 15, 2, 1),
        attr(3000, 0, 4, 1, 12, 2, 1),
        attr(3000, 0, 4, 1, 15, 0, 1),
        attr(3000, 0, 4, 1, 15, 2, 4),
        attr(3000, 10, 4, 1, 15, 2, 1),
        attr(10000, 0, MISSING_U8, 1, 15, 2, 1),
        attr(MISSING_U16, 0, MISSING_U8, 1, 15, 2, 1),
    ] {
        let bytes = bin_with(&[bad], Some(1), &deps, 8);
        assert!(decode_accident_attrs(&bytes).is_err(), "{bad:?}");
    }
    // 別のファイル種別のマジックナンバー
    let mut writer = BinFileWriter::new(*b"TKRGRAPH", 1);
    writer.add_json("DEPS", &json!({}));
    assert!(decode_accident_attrs(&writer.finish()).is_err());
    // 壊れたバイト列
    assert!(decode_accident_attrs(b"short").is_err());
    let mut broken = good.clone();
    broken.truncate(broken.len() - 4);
    assert!(decode_accident_attrs(&broken).is_err());
}

#[test]
fn a_release_that_does_not_match_the_source_is_rejected() {
    let f = fixture();
    // 原本の保持行が変わると、事故IDの対応を保証できない
    // （1）保持行が1行減る
    let fewer = tempfile::tempdir().unwrap();
    write_sources_with(fewer.path(), |year, cases| {
        if year == 2024 {
            cases[0].row = cases[0].row.clone().set("天候", "9");
        }
    });
    let error = build_accident_attrs(fewer.path(), &f.release, &AcceptAll).unwrap_err();
    assert!(matches!(error, AttrsError::Mismatch(_)), "{error}");
    // （2）件数は同じで、行の内容（座標）が違う
    let moved = tempfile::tempdir().unwrap();
    write_sources_with(moved.path(), |year, cases| {
        if year == 2024 {
            cases[0].row = cases[0].row.clone().set("地点　緯度（北緯）", "354060000");
        }
    });
    let error = build_accident_attrs(moved.path(), &f.release, &AcceptAll).unwrap_err();
    assert!(matches!(error, AttrsError::Mismatch(_)), "{error}");

    // releaseのaccidents.csv.gzが改ざんされている
    let broken = tempfile::tempdir().unwrap();
    for name in ["accidents.csv.gz", "syukujitsu.csv", "metadata.json"] {
        std::fs::copy(f.release.join(name), broken.path().join(name)).unwrap();
    }
    let mut gz = std::fs::read(broken.path().join(ACCIDENTS_FILE)).unwrap();
    let last = gz.len() / 2;
    gz[last] ^= 0xFF;
    std::fs::write(broken.path().join(ACCIDENTS_FILE), gz).unwrap();
    let error = build_accident_attrs(&f.source, broken.path(), &AcceptAll).unwrap_err();
    assert!(matches!(error, AttrsError::Release(_)), "{error}");

    // schema 2のreleaseでない
    let wrong = tempfile::tempdir().unwrap();
    for name in ["accidents.csv.gz", "syukujitsu.csv"] {
        std::fs::copy(f.release.join(name), wrong.path().join(name)).unwrap();
    }
    let mut metadata = f.metadata.clone();
    metadata["schema_version"] = json!(3);
    std::fs::write(wrong.path().join("metadata.json"), metadata.to_string()).unwrap();
    let error = build_accident_attrs(&f.source, wrong.path(), &AcceptAll).unwrap_err();
    assert!(matches!(error, AttrsError::Release(_)), "{error}");
}

#[test]
fn a_missing_or_duplicate_attribute_column_fails_without_changing_prepare_release() {
    let f = fixture();
    let mutate = |from: &str, to: &str| {
        let dir = tempfile::tempdir().unwrap();
        write_sources(dir.path());
        let path = dir.path().join("honhyo_2023.csv");
        let text = encoding_rs::SHIFT_JIS
            .decode(&std::fs::read(&path).unwrap())
            .0
            .replacen(from, to, 1);
        std::fs::write(&path, cp932(&text)).unwrap();
        dir
    };
    // 属性の列がない（地形→地形名）／重複する（資料区分→地形）年は、属性の取り出しだけが失敗する
    for dir in [mutate("地　形", "地形名"), mutate("資料区分", "地形")] {
        let error = build_accident_attrs(dir.path(), &f.release, &AcceptAll).unwrap_err();
        assert!(
            matches!(error, AttrsError::Preprocess(PreprocessError::File { .. })),
            "{error}"
        );
        assert!(error.to_string().contains("honhyo_2023.csv"), "{error}");
        // 既存の前処理は属性の列を見ないので、同じreleaseができる
        let prepared = prepare_release(
            dir.path(),
            HOLIDAYS.as_bytes().to_vec(),
            &AcceptAll,
            "2026-11-03T09:00:00+09:00",
        )
        .unwrap();
        assert_eq!(prepared.release.dataset_id, f.metadata["dataset_id"]);
    }
}

// ---- コード表（実ファイル） ----

fn real_accident_dir() -> std::path::PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../accident");
    assert!(
        dir.join("codebook_2024.xlsx").is_file(),
        "{} に原本がありません（リポジトリの accident/ が必要）",
        dir.display()
    );
    dir
}

#[test]
fn real_codebooks_match_the_attribute_tables() {
    let dir = real_accident_dir();
    let (xlsx, pdf) = AttrCodebooks.verify(&dir).unwrap();
    assert_eq!(
        xlsx.iter().map(|c| c.source_year).collect::<Vec<_>>(),
        [2022, 2023, 2024, 2025]
    );
    assert_eq!(
        pdf.iter().map(|c| c.source_year).collect::<Vec<_>>(),
        [2019, 2020, 2021]
    );
    assert!(xlsx.iter().chain(&pdf).all(|c| c.sha256.len() == 64));

    // 2022年だけ車道幅員のコード表が違う（12,14／13,17／16,18）。年を取り違えると失敗する
    let sheets = |year: u16| -> AttrSheets {
        let bytes = std::fs::read(dir.join(format!("codebook_{year}.xlsx"))).unwrap();
        read_sheet_rows(
            &bytes,
            &["路線", "道路形状", "車道幅員", "歩車道区分", "地形"],
        )
        .unwrap()
    };
    verify_attr_sheets(2022, &sheets(2022)).unwrap();
    assert!(verify_attr_sheets(2024, &sheets(2022)).is_err());
    assert!(verify_attr_sheets(2022, &sheets(2024)).is_err());
    for year in [2023, 2024, 2025] {
        verify_attr_sheets(year, &sheets(year)).unwrap();
    }
}

#[test]
fn missing_codebook_files_fail() {
    let empty = tempfile::tempdir().unwrap();
    assert!(AttrCodebooks.verify(empty.path()).is_err());
}

#[test]
fn cli_writes_the_bin_and_audit_for_a_release() {
    let f = fixture();
    // コード表は実ファイルで照合されるので、原本の隣にコピーする
    let real = real_accident_dir();
    for entry in std::fs::read_dir(&real).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.starts_with("codebook_") {
            std::fs::copy(&path, f.source.join(&name)).unwrap();
        }
    }
    let out = tempfile::tempdir().unwrap();
    let bin = out.path().join("sub/accident_attrs.bin");
    let audit = out.path().join("audit.json");
    let routing_prep = || {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_routing_prep"));
        command.arg("accident-attrs");
        command
    };
    // 引数の不足・不明な引数は失敗し、何も書かない
    let failed = routing_prep().arg("--output").arg(&bin).output().unwrap();
    assert!(!failed.status.success());
    let failed = routing_prep()
        .args(["--bogus", "x", "--output"])
        .arg(&bin)
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(!bin.exists());

    let result = routing_prep()
        .arg("--accident-dir")
        .arg(&f.source)
        .arg("--base-release")
        .arg(&f.release)
        .arg("--output")
        .arg(&bin)
        .arg("--audit")
        .arg(&audit)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let summary: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(summary["rows"], f.metadata["rows"]);
    assert_eq!(summary["dataset_id"], f.metadata["dataset_id"]);
    assert_eq!(summary["gzip_identical"], true);

    let bytes = std::fs::read(&bin).unwrap();
    let decoded = decode_accident_attrs(&bytes).unwrap();
    assert_eq!(decoded.value.attrs, expected_attrs());
    assert_eq!(
        decoded.deps[ACCIDENTS_FILE],
        f.metadata["dataset_id"].as_str().unwrap()
    );
    assert_eq!(bin.file_name().unwrap(), ACCIDENT_ATTRS_FILE);
    let audit: Value = serde_json::from_slice(&std::fs::read(&audit).unwrap()).unwrap();
    assert_eq!(audit["rows"], f.metadata["rows"]);
    assert!(audit["plan_4_2_check"].is_object());
    // 実ファイルのコード表で照合した
    assert_eq!(audit["codebooks"]["xlsx"].as_array().unwrap().len(), 4);
    // 一時ファイルを残さない
    let leftovers: Vec<_> = std::fs::read_dir(bin.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, ["accident_attrs.bin"]);
}
