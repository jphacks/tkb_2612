//! releaseのロードと起動時検証（behavior.md 3章、11章 追加試験3、#8 ステップ7）。

use std::io::{Read, Write};
use std::path::Path;

use accident_route_backend::data::release::{
    ACCIDENTS_FILE, HOLIDAYS_FILE, METADATA_FILE, release_id, sha256_hex,
};
use accident_route_backend::data::{self, LoadError};
use accident_route_backend::testing::TestRelease;
use accident_route_backend::testing::release::TEST_HOLIDAY_CSV;

/// 人工releaseを一時ディレクトリへ複製する（改変の試験用）。
fn copy_release() -> (TestRelease, tempfile::TempDir) {
    let release = TestRelease::for_example("twoSelected");
    let dir = tempfile::tempdir().unwrap();
    for name in [ACCIDENTS_FILE, HOLIDAYS_FILE, METADATA_FILE] {
        std::fs::copy(release.path().join(name), dir.path().join(name)).unwrap();
    }
    (release, dir)
}

fn metadata(dir: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(dir.join(METADATA_FILE)).unwrap()).unwrap()
}

fn write_metadata(dir: &Path, value: &serde_json::Value) {
    std::fs::write(dir.join(METADATA_FILE), serde_json::to_vec(value).unwrap()).unwrap();
}

fn gunzip(bytes: &[u8]) -> String {
    let mut text = String::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_string(&mut text)
        .unwrap();
    text
}

fn gzip(text: &str) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(3));
    e.write_all(text.as_bytes()).unwrap();
    e.finish().unwrap()
}

/// 事故CSVを差し替え、metadataのIDと件数を合わせる（形式の検証だけを失敗させるため）。
fn replace_accidents(dir: &Path, text: &str, rows: u64) {
    let bytes = gzip(text);
    std::fs::write(dir.join(ACCIDENTS_FILE), &bytes).unwrap();
    let mut m = metadata(dir);
    let dataset_id = sha256_hex(&bytes);
    let holidays = sha256_hex(&std::fs::read(dir.join(HOLIDAYS_FILE)).unwrap());
    m["dataset_id"] = dataset_id.clone().into();
    m["release_id"] = release_id(&dataset_id, &holidays).into();
    m["rows"] = rows.into();
    write_metadata(dir, &m);
}

fn assert_invalid(dir: &Path, case: &str) {
    match data::load_engine(dir) {
        Err(LoadError::Invalid(_)) => {}
        Err(other) => panic!("{case}: Invalid以外の失敗: {other}"),
        Ok(_) => panic!("{case}: 改変したreleaseを受け付けた"),
    }
}

#[test]
fn test_generated_schema_load_roundtrip() {
    let release = TestRelease::for_example("twoSelected");
    let engine = data::load_engine(release.path()).unwrap();
    let info = engine.dataset();
    assert_eq!(info.accidents, 3);
    assert_eq!(info.data_years, [2025]);
    assert_eq!(info.dataset_id, release.ids.dataset_id);
    assert_eq!(info.release_id, release.ids.release_id);
    assert_eq!(info.grid_cells, Some(3));
    assert!(info.load_seconds >= 0.0);

    // 発生年≠source_year・緯度・mask・date・day_typeを保持する
    let text = "lat,lon,modes,period,weather,day_type,deaths,injuries,source_year,date,hour,minute,daynight,accident_type,injury_a,injury_b,severity\n\
                35.6805556,139.7647222,6,4,5,1,1,2,2019,20181231,2,5,22,21,1,2,死亡\n";
    let columns = data::load::parse_accidents(&gzip(text)).unwrap();
    assert_eq!(columns.lat, [35.6805556]);
    assert_eq!(columns.lon, [139.7647222]);
    assert_eq!(columns.modes, [6]);
    assert_eq!(columns.period, [4]);
    assert_eq!(columns.weather, [5]);
    assert_eq!(columns.day_type, [1]);
    assert_eq!(columns.source_year, [2019]);
    assert_eq!(columns.date, [20181231]);
}

#[test]
fn tampered_files_stop_loading() {
    // gzipの1バイトを変える
    let (_r, dir) = copy_release();
    let mut bytes = std::fs::read(dir.path().join(ACCIDENTS_FILE)).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    std::fs::write(dir.path().join(ACCIDENTS_FILE), bytes).unwrap();
    assert_invalid(dir.path(), "gzipの改変");

    // 祝日表の差し替え（metadataはそのまま）
    let (_r, dir) = copy_release();
    std::fs::write(
        dir.path().join(HOLIDAYS_FILE),
        format!("{TEST_HOLIDAY_CSV}2025/12/1,テスト\n"),
    )
    .unwrap();
    assert_invalid(dir.path(), "祝日表の差し替え");
    assert!(
        data::load_calendar(dir.path()).is_err(),
        "load_calendarも照合する"
    );

    // metadataのdataset_id・release_idの改変
    for key in ["dataset_id", "release_id"] {
        let (_r, dir) = copy_release();
        let mut m = metadata(dir.path());
        m[key] = "0".repeat(64).into();
        write_metadata(dir.path(), &m);
        assert_invalid(dir.path(), key);
    }

    // schema_versionと件数
    let (_r, dir) = copy_release();
    let mut m = metadata(dir.path());
    m["schema_version"] = 1.into();
    write_metadata(dir.path(), &m);
    assert_invalid(dir.path(), "schema_version");
    let (_r, dir) = copy_release();
    let mut m = metadata(dir.path());
    m["rows"] = 4.into();
    write_metadata(dir.path(), &m);
    assert_invalid(dir.path(), "件数");
}

#[test]
fn malformed_accident_csv_stops_loading() {
    let header = "lat,lon,modes,period,weather,day_type,deaths,injuries,source_year,date,hour,minute,daynight,accident_type,injury_a,injury_b,severity";
    let row = |period: &str, day_type: &str| {
        format!(
            "35.0000000,139.0000000,7,{period},1,{day_type},0,1,2025,20250106,12,0,12,21,2,0,負傷"
        )
    };
    for (case, text, rows) in [
        ("period=5", format!("{header}\n{}\n", row("5", "0")), 1),
        ("day_type=2", format!("{header}\n{}\n", row("1", "2")), 1),
        ("列数不一致", format!("{header}\n{},x\n", row("1", "0")), 1),
        (
            "16列のヘッダ",
            format!("{}\n", header.rsplit_once(',').unwrap().0),
            0,
        ),
        (
            "列名の違い",
            format!(
                "{}\n{}\n",
                header.replace("day_type", "daytype"),
                row("1", "0")
            ),
            1,
        ),
        ("空データ", format!("{header}\n"), 0),
        ("数値でない値", format!("{header}\n{}\n", row("x", "0")), 1),
        (
            "範囲外の緯度",
            format!("{header}\n{}\n", row("1", "0").replacen("35.0", "15.0", 1)),
            1,
        ),
    ] {
        let (_r, dir) = copy_release();
        replace_accidents(dir.path(), &text, rows);
        assert_invalid(dir.path(), case);
    }
}

#[test]
fn holiday_only_change_changes_release_id_but_not_dataset_id() {
    let (_r, dir) = copy_release();
    let before = data::load_engine(dir.path()).unwrap().dataset().clone();
    let holidays = format!("{TEST_HOLIDAY_CSV}2025/12/1,テスト\n");
    std::fs::write(dir.path().join(HOLIDAYS_FILE), &holidays).unwrap();
    let holiday_sha = sha256_hex(holidays.as_bytes());
    let mut m = metadata(dir.path());
    m["holiday_table"]["sha256"] = holiday_sha.clone().into();
    m["release_id"] = release_id(&before.dataset_id, &holiday_sha).into();
    write_metadata(dir.path(), &m);
    let after = data::load_engine(dir.path()).unwrap().dataset().clone();
    assert_eq!(after.dataset_id, before.dataset_id);
    assert_ne!(after.release_id, before.release_id);
    // 事故CSVの内容は同じ
    assert_eq!(
        gunzip(&std::fs::read(dir.path().join(ACCIDENTS_FILE)).unwrap())
            .lines()
            .count(),
        4
    );
}

#[test]
fn missing_release_is_an_io_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        data::load_engine(dir.path()),
        Err(LoadError::Io(_))
    ));
    assert!(matches!(
        data::load_calendar(dir.path()),
        Err(LoadError::Io(_))
    ));
}
