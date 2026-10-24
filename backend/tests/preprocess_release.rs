//! 前処理からreleaseの書き出しまで（behavior.md 2章・3章、#8 ステップ6）。

mod common;

use std::io::Read;

use accident_route_backend::data;
use accident_route_backend::data::preprocess::prepare_release;
use accident_route_backend::data::preprocess::rows::OUTPUT_COLUMNS;
use accident_route_backend::data::release::{
    ACCIDENTS_FILE, HOLIDAYS_FILE, METADATA_FILE, release_id, sha256_hex, write_release_files,
};

use common::source::{AcceptAll, HOLIDAYS, SourceRow, write_source_dir};

fn source_rows(year: u16) -> Vec<SourceRow> {
    let mut rows = vec![
        SourceRow::valid(year, 1, 6, 12),
        // 二輪×対象外：modes=0の行も保持する
        SourceRow::valid(year, 1, 7, 12)
            .set("当事者種別（当事者A）", "31")
            .set("当事者種別（当事者B）", "0"),
        // 除外される行
        SourceRow::valid(year, 1, 8, 12).set("天候", "9"),
    ];
    if year == 2019 {
        // 2018年の事故（発生年≠source_year）。2018年は祝日表の範囲外
        rows.push(SourceRow::valid(2018, 12, 3, 9));
    }
    rows
}

#[test]
fn release_has_17_columns_ids_and_audit() {
    let source = tempfile::tempdir().unwrap();
    write_source_dir(source.path(), source_rows);
    let original: Vec<(String, Vec<u8>)> = std::fs::read_dir(source.path())
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect();

    let prepared = prepare_release(
        source.path(),
        HOLIDAYS.as_bytes().to_vec(),
        &AcceptAll,
        "2026-10-25T09:00:00+09:00",
    )
    .unwrap();
    let out = tempfile::tempdir().unwrap();
    write_release_files(out.path(), &prepared.release).unwrap();

    // 原本は変更しない
    for (name, bytes) in original {
        assert_eq!(
            std::fs::read(source.path().join(&name)).unwrap(),
            bytes,
            "{name}"
        );
    }

    // 17列のヘッダ（完全一致）と保持行
    let gzip = std::fs::read(out.path().join(ACCIDENTS_FILE)).unwrap();
    let mut csv = String::new();
    flate2::read::GzDecoder::new(gzip.as_slice())
        .read_to_string(&mut csv)
        .unwrap();
    let mut lines = csv.lines();
    assert_eq!(lines.next().unwrap(), OUTPUT_COLUMNS.join(","));
    let rows: Vec<Vec<&str>> = lines.map(|l| l.split(',').collect()).collect();
    assert_eq!(rows.len(), 7 * 2 + 1);
    assert!(rows.iter().all(|r| r.len() == 17));
    // 2019年の行が先、入力行順（年度順→行順）
    assert_eq!(rows[0][8], "2019");
    assert_eq!(rows[2][9], "20181203");
    assert_eq!(rows[2][8], "2019");
    assert_eq!(rows.last().unwrap()[8], "2025");
    assert_eq!(rows[0][0], "35.6805556"); // 小数7桁
    assert_eq!(rows[1][2], "0"); // modes=0を保持

    // 識別子の再計算がmetadataと一致する
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.path().join(METADATA_FILE)).unwrap()).unwrap();
    let dataset_id = sha256_hex(&gzip);
    let holiday_sha = sha256_hex(&std::fs::read(out.path().join(HOLIDAYS_FILE)).unwrap());
    assert_eq!(metadata["dataset_id"], dataset_id.as_str());
    assert_eq!(
        metadata["release_id"],
        release_id(&dataset_id, &holiday_sha).as_str()
    );
    assert_eq!(metadata["schema_version"], 2);
    assert_eq!(metadata["rows"], 15);
    assert_eq!(metadata["gzip_bytes"], gzip.len());
    assert_eq!(metadata["generated_at"], "2026-10-25T09:00:00+09:00");
    assert_eq!(
        metadata["data_years"],
        serde_json::json!([2019, 2020, 2021, 2022, 2023, 2024, 2025])
    );
    assert_eq!(metadata["holiday_table"]["sha256"], holiday_sha.as_str());
    assert_eq!(metadata["holiday_table"]["first_date"], "2019-01-01");
    assert_eq!(metadata["holiday_table"]["last_date"], "2025-11-24");
    assert_eq!(metadata["holiday_table"]["rows"], 4);
    // 2018年の1行だけが収録範囲外
    assert_eq!(metadata["holiday_uncovered_rows"], 1);
    assert_eq!(metadata["totals"]["input_rows"], 22);
    assert_eq!(metadata["totals"]["retained_rows"], 15);
    assert_eq!(metadata["totals"]["excluded_rows"], 7);
    let files = metadata["files"].as_array().unwrap();
    assert_eq!(files.len(), 7);
    assert_eq!(files[0]["source_year"], 2019);
    assert_eq!(files[0]["encoding"], "cp932");
    assert_eq!(files[0]["no_target_mode"], 1);
    assert_eq!(files[0]["excluded_by_reason"]["unknown_weather_code"], 1);
    assert!(metadata["elapsed_seconds"].as_f64().unwrap() >= 0.0);

    // 書いたreleaseはそのままロードできる
    let engine = data::load_engine(out.path()).unwrap();
    let info = engine.dataset();
    assert_eq!(info.dataset_id, dataset_id);
    assert_eq!(info.accidents, 15);
    assert_eq!(info.data_years, [2019, 2020, 2021, 2022, 2023, 2024, 2025]);
    data::load_calendar(out.path()).unwrap();
}

#[test]
fn same_input_gives_same_dataset_id() {
    let source = tempfile::tempdir().unwrap();
    write_source_dir(source.path(), source_rows);
    let run = |generated_at: &str| {
        prepare_release(
            source.path(),
            HOLIDAYS.as_bytes().to_vec(),
            &AcceptAll,
            generated_at,
        )
        .unwrap()
        .release
    };
    let (a, b) = (
        run("2026-10-25T09:00:00+09:00"),
        run("2026-10-26T09:00:00+09:00"),
    );
    // generated_atはIDに入らない
    assert_eq!(a.dataset_id, b.dataset_id);
    assert_eq!(a.release_id, b.release_id);
}
