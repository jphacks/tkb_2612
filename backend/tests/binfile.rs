//! 容器形式 `data::binfile`（形式書3章）の読み書きと検査。

use accident_route_backend::data::LoadError;
use accident_route_backend::data::binfile::{
    BinFile, BinFileWriter, DEPS_SECTION, HEADER_LEN, PARAMS_SECTION, SECTION_ENTRY_LEN,
};
use serde_json::json;

const MAGIC: [u8; 8] = *b"TKRTEST_";

fn invalid_message(result: Result<BinFile<'_>, LoadError>) -> String {
    match result {
        Err(LoadError::Invalid(message)) => message,
        Err(other) => panic!("Invalid以外の失敗: {other}"),
        Ok(_) => panic!("壊れたファイルを受け付けた"),
    }
}

/// DEPS・PARAMS・数値の節を持つ見本。
fn sample() -> Vec<u8> {
    let mut w = BinFileWriter::new(MAGIC, 7);
    w.add_json(
        DEPS_SECTION,
        &json!({"graph.bin": "a".repeat(64), "accidents.csv.gz": "b".repeat(64)}),
    );
    w.add_json(PARAMS_SECTION, &json!({"rows": 3, "name": "見本"}));
    w.add_values::<u32>("NODEOFS", &[0, 2, 5, 9]);
    w.add_values::<i32>("LL", &[-1, 2, i32::MIN, i32::MAX]);
    w.add_values::<i16>("DELTA", &[-3, 4, i16::MAX]);
    w.add_values::<u64>("BITS", &[u64::MAX, 1]);
    w.add_values::<f32>("W", &[0.5, -1.25]);
    w.add_values::<f64>("R", &[std::f64::consts::PI]);
    w.add_values::<u8>("FLG", &[1, 2, 3]);
    w.add_section("EMPTY", 4, 0, Vec::new());
    w.finish()
}

/// 節表だけを指定して作る生のファイル。本体は全体の長さまで0で埋める。
/// 各要素は (名前, 開始位置, 件数, elem_size)。
fn raw(entries: &[(&str, u64, u64, u32)], total_len: usize) -> Vec<u8> {
    let header_len = HEADER_LEN + SECTION_ENTRY_LEN * entries.len();
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend_from_slice(&(header_len as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 4 + 32 + 8]);
    for (name, start, count, elem_size) in entries {
        let mut padded = [0u8; 8];
        padded[..name.len()].copy_from_slice(name.as_bytes());
        out.extend_from_slice(&padded);
        out.extend_from_slice(&start.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&elem_size.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
    }
    assert_eq!(out.len(), header_len);
    out.resize(total_len.max(header_len), 0);
    out
}

#[test]
fn round_trip() {
    let bytes = sample();
    let file = BinFile::parse(&bytes).unwrap();
    file.expect(MAGIC, 7).unwrap();
    assert_eq!(file.magic(), MAGIC);
    assert_eq!(file.format_version(), 7);
    assert_eq!(file.section_count(), 10);

    assert_eq!(
        file.section("NODEOFS").unwrap().u32s().unwrap(),
        [0, 2, 5, 9]
    );
    assert_eq!(
        file.section("LL").unwrap().i32s().unwrap(),
        [-1, 2, i32::MIN, i32::MAX]
    );
    assert_eq!(
        file.section("DELTA").unwrap().i16s().unwrap(),
        [-3, 4, i16::MAX]
    );
    assert_eq!(file.section("BITS").unwrap().u64s().unwrap(), [u64::MAX, 1]);
    assert_eq!(file.section("W").unwrap().f32s().unwrap(), [0.5, -1.25]);
    assert_eq!(
        file.section("R").unwrap().f64s().unwrap(),
        [std::f64::consts::PI]
    );
    assert_eq!(file.section("FLG").unwrap().u8s().unwrap(), [1, 2, 3]);
    let empty = file.section("EMPTY").unwrap();
    assert_eq!((empty.count, empty.elem_size, empty.bytes.len()), (0, 4, 0));
    assert!(file.section("NOPE").is_none());
    assert!(file.require("NOPE").is_err());

    let nodeofs = file.section("NODEOFS").unwrap();
    assert_eq!(
        (nodeofs.name, nodeofs.count, nodeofs.elem_size),
        ("NODEOFS", 4, 4)
    );
    assert_eq!(nodeofs.bytes.len(), 16);
    assert_eq!(file.section_counts()["BITS"], 2);

    assert_eq!(
        file.json(PARAMS_SECTION).unwrap(),
        json!({"rows": 3, "name": "見本"})
    );
    file.verify_payload_sha256().unwrap();
}

#[test]
fn layout_follows_the_format_document() {
    let bytes = sample();
    let section_count = 10;
    assert_eq!(&bytes[0..8], &MAGIC);
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 7);
    assert_eq!(
        u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
        section_count
    );
    assert_eq!(
        u32::from_le_bytes(bytes[16..20].try_into().unwrap()) as usize,
        64 + 32 * section_count as usize
    );
    assert!(bytes[20..24].iter().all(|&b| b == 0));
    assert!(bytes[56..64].iter().all(|&b| b == 0));
    // 節表：名前は8バイトに0で埋める。開始位置は8の倍数
    let first = &bytes[64..96];
    assert_eq!(&first[0..8], b"DEPS\0\0\0\0");
    for i in 0..section_count as usize {
        let at = 64 + 32 * i;
        let start = u64::from_le_bytes(bytes[at + 8..at + 16].try_into().unwrap());
        assert_eq!(start % 8, 0, "{i}番目の節の開始位置");
    }
    // 先頭の節はヘッダー（節表を含む）の直後
    let start0 = u64::from_le_bytes(first[8..16].try_into().unwrap());
    assert_eq!(start0, 64 + 32 * u64::from(section_count));
}

#[test]
fn output_is_deterministic() {
    assert_eq!(sample(), sample());
}

#[test]
fn deps_are_read() {
    let bytes = sample();
    let deps = BinFile::parse(&bytes).unwrap().deps().unwrap();
    assert_eq!(deps.len(), 2);
    assert_eq!(deps["graph.bin"], "a".repeat(64));
    assert_eq!(deps["accidents.csv.gz"], "b".repeat(64));

    // 入力がなければ {}
    let mut w = BinFileWriter::new(MAGIC, 1);
    w.add_json(DEPS_SECTION, &json!({}));
    let bytes = w.finish();
    assert!(BinFile::parse(&bytes).unwrap().deps().unwrap().is_empty());

    // DEPS の値が文字列でない、DEPS がない、JSONオブジェクトでない
    let mut w = BinFileWriter::new(MAGIC, 1);
    w.add_json(DEPS_SECTION, &json!({"graph.bin": 1}));
    let bytes = w.finish();
    assert!(BinFile::parse(&bytes).unwrap().deps().is_err());
    let mut w = BinFileWriter::new(MAGIC, 1);
    w.add_json(PARAMS_SECTION, &json!({}));
    let bytes = w.finish();
    assert!(BinFile::parse(&bytes).unwrap().deps().is_err());
    let mut w = BinFileWriter::new(MAGIC, 1);
    w.add_section(DEPS_SECTION, 1, 2, b"[]".to_vec());
    let bytes = w.finish();
    assert!(BinFile::parse(&bytes).unwrap().deps().is_err());
    let mut w = BinFileWriter::new(MAGIC, 1);
    w.add_section(DEPS_SECTION, 1, 3, b"{x}".to_vec());
    let bytes = w.finish();
    assert!(BinFile::parse(&bytes).unwrap().deps().is_err());
}

#[test]
fn typed_reads_check_elem_size() {
    let bytes = sample();
    let file = BinFile::parse(&bytes).unwrap();
    let nodeofs = file.section("NODEOFS").unwrap();
    assert!(nodeofs.u64s().is_err());
    assert!(nodeofs.u8s().is_err());
    assert!(nodeofs.expect_elem_size(4).is_ok());
    assert!(nodeofs.expect_elem_size(8).is_err());
    assert!(nodeofs.expect_count(4).is_ok());
    assert!(nodeofs.expect_count(5).is_err());
    assert_eq!(nodeofs.records(4).unwrap().count(), 4);
    assert!(nodeofs.records(8).is_err());
    // JSONの節は数値の配列としては読めるが（elem_size=1）、オブジェクトでない節をjsonで読むと失敗する
    assert!(file.json("NODEOFS").is_err());
}

#[test]
fn expect_checks_magic_and_version() {
    let bytes = sample();
    let file = BinFile::parse(&bytes).unwrap();
    assert!(file.expect(MAGIC, 7).is_ok());
    assert!(file.expect(MAGIC, 8).is_err());
    assert!(file.expect(*b"TKRGRAPH", 7).is_err());
}

#[test]
fn broken_headers_are_rejected() {
    let good = sample();

    // ヘッダーに満たない・空
    assert!(invalid_message(BinFile::parse(&[])).contains("ヘッダー"));
    assert!(invalid_message(BinFile::parse(&good[..63])).contains("ヘッダー"));

    // header_len が節の数と合わない
    let mut bytes = good.clone();
    bytes[16..20].copy_from_slice(&64u32.to_le_bytes());
    assert!(invalid_message(BinFile::parse(&bytes)).contains("header_len"));

    // 節表の途中で終わる
    assert!(invalid_message(BinFile::parse(&good[..100])).contains("節表"));

    // section_count が巨大（header_len も合わせても、ファイルに収まらない）
    let mut bytes = good.clone();
    let count = 0x00ff_ffffu32;
    bytes[12..16].copy_from_slice(&count.to_le_bytes());
    bytes[16..20].copy_from_slice(&(64 + 32 * count).to_le_bytes());
    assert!(invalid_message(BinFile::parse(&bytes)).contains("節表"));

    // 予約領域が0でない
    for at in [20usize, 56, 63] {
        let mut bytes = good.clone();
        bytes[at] = 1;
        assert!(
            invalid_message(BinFile::parse(&bytes)).contains("予約"),
            "offset {at}"
        );
    }

    // 節表の予約が0でない
    let mut bytes = good.clone();
    bytes[64 + 28] = 1;
    assert!(invalid_message(BinFile::parse(&bytes)).contains("予約"));

    // 節の名前：空、非ASCII、NULの後に文字
    for name in [[0u8; 8], *b"\xe3\x81\x82\0\0\0\0\0", *b"AB\0CD\0\0\0"] {
        let mut bytes = good.clone();
        bytes[64..72].copy_from_slice(&name);
        assert!(
            invalid_message(BinFile::parse(&bytes)).contains("名前"),
            "{name:?}"
        );
    }

    // elem_size が0
    let mut bytes = good.clone();
    bytes[64 + 24..64 + 28].copy_from_slice(&0u32.to_le_bytes());
    assert!(invalid_message(BinFile::parse(&bytes)).contains("elem_size"));
}

#[test]
fn section_ranges_are_checked() {
    let header = 64 + 32; // 節が1つ
    // 範囲内の節は通る
    BinFile::parse(&raw(&[("A", header as u64, 4, 4)], header + 16)).unwrap();
    // ファイルの外にはみ出す
    let message = invalid_message(BinFile::parse(&raw(
        &[("A", header as u64, 5, 4)],
        header + 16,
    )));
    assert!(message.contains("範囲外"), "{message}");
    // 開始位置が遠くてファイルの外
    let message = invalid_message(BinFile::parse(&raw(&[("A", 4096, 1, 1)], header + 16)));
    assert!(message.contains("範囲外"), "{message}");
    // count×elem_size があふれる
    let message = invalid_message(BinFile::parse(&raw(
        &[("A", header as u64, u64::MAX, 8)],
        header + 16,
    )));
    assert!(message.contains("大きすぎる"), "{message}");
    // 開始位置+長さがあふれる
    let message = invalid_message(BinFile::parse(&raw(
        &[("A", u64::MAX - 7, 16, 1)],
        header + 16,
    )));
    assert!(message.contains("範囲外"), "{message}");
    // 開始位置が8の倍数でない
    let message = invalid_message(BinFile::parse(&raw(
        &[("A", header as u64 + 4, 1, 1)],
        header + 16,
    )));
    assert!(message.contains("8バイト境界"), "{message}");
    // ヘッダー（節表）の中に食い込む
    let message = invalid_message(BinFile::parse(&raw(&[("A", 64, 1, 1)], header + 16)));
    assert!(message.contains("重なって"), "{message}");
}

#[test]
fn overlapping_or_unordered_sections_are_rejected() {
    let header = 64 + 32 * 2;
    let h = header as u64;
    // 重ならない（隣り合う）
    BinFile::parse(&raw(&[("A", h, 4, 4), ("B", h + 16, 4, 4)], header + 32)).unwrap();
    // 後ろの節が前の節の途中から始まる
    let message = invalid_message(BinFile::parse(&raw(
        &[("A", h, 4, 4), ("B", h + 8, 4, 4)],
        header + 32,
    )));
    assert!(message.contains("重なって"), "{message}");
    // 節表の順と並びが逆
    let message = invalid_message(BinFile::parse(&raw(
        &[("A", h + 16, 4, 4), ("B", h, 4, 4)],
        header + 32,
    )));
    assert!(message.contains("重なって"), "{message}");
    // 同じ位置の2つの節
    let message = invalid_message(BinFile::parse(&raw(
        &[("A", h, 4, 4), ("B", h, 4, 4)],
        header + 32,
    )));
    assert!(message.contains("重なって"), "{message}");
    // 空の節は同じ位置に並んでよい
    BinFile::parse(&raw(&[("A", h, 0, 4), ("B", h, 0, 4)], header)).unwrap();
}

#[test]
fn duplicate_section_names_are_rejected() {
    let header = 64 + 32 * 2;
    let h = header as u64;
    let message = invalid_message(BinFile::parse(&raw(
        &[("DEPS", h, 1, 1), ("DEPS", h + 8, 1, 1)],
        header + 16,
    )));
    assert!(message.contains("重複"), "{message}");
}

#[test]
fn writer_rejects_duplicate_names_and_bad_sections() {
    let panics = |f: fn(&mut BinFileWriter)| {
        std::panic::catch_unwind(move || {
            let mut w = BinFileWriter::new(MAGIC, 1);
            f(&mut w);
        })
        .is_err()
    };
    assert!(panics(|w| {
        w.add_values::<u8>("A", &[1]);
        w.add_values::<u8>("A", &[2]);
    }));
    assert!(panics(|w| w.add_values::<u8>("", &[1])));
    assert!(panics(|w| w.add_values::<u8>("TOOLONGNAME", &[1])));
    assert!(panics(|w| w.add_values::<u8>("日本語", &[1])));
    assert!(panics(|w| w.add_section("A", 4, 2, vec![0; 7])));
    assert!(panics(|w| w.add_section("A", 0, 0, vec![])));
    assert!(!panics(|w| w.add_values::<u8>("EIGHTCHR", &[1])));
}

#[test]
fn nonzero_padding_and_trailing_bytes_are_rejected() {
    // 3バイトの節の後ろに詰め物が入る
    let mut w = BinFileWriter::new(MAGIC, 1);
    w.add_values::<u8>("A", &[1, 2, 3]);
    w.add_values::<u8>("B", &[4]);
    let good = w.finish();
    let file = BinFile::parse(&good).unwrap();
    assert_eq!(file.section("B").unwrap().u8s().unwrap(), [4]);
    let header = 64 + 32 * 2;
    // 詰め物（A の後ろ）を壊す
    let mut bytes = good.clone();
    bytes[header + 3] = 9;
    let message = invalid_message(BinFile::parse(&bytes));
    assert!(message.contains("詰め物"), "{message}");
    // 末尾に節の外のデータを足す
    let mut bytes = good.clone();
    bytes.extend_from_slice(&[0, 0, 7]);
    let message = invalid_message(BinFile::parse(&bytes));
    assert!(message.contains("末尾"), "{message}");
    // 末尾の0はそのまま通る（payload_sha256 は内容と一致しなくなるので別に検出する）
    let mut bytes = good;
    bytes.extend_from_slice(&[0, 0, 0]);
    let file = BinFile::parse(&bytes).unwrap();
    assert!(file.verify_payload_sha256().is_err());
}

#[test]
fn payload_tampering_is_detected() {
    let good = sample();
    BinFile::parse(&good)
        .unwrap()
        .verify_payload_sha256()
        .unwrap();
    let header_len = 64 + 32 * 10;

    // 節の本体の1バイトを変える：構造の検査は通るが、payload_sha256 が一致しない
    let mut bytes = good.clone();
    let last_data = bytes.len() - 1;
    for at in [header_len, header_len + 5, last_data] {
        let mut tampered = bytes.clone();
        tampered[at] ^= 0x01;
        // 節の外の詰め物を壊すと parse が先に失敗するので、parse が通る場合だけ照合を確かめる
        if let Ok(file) = BinFile::parse(&tampered) {
            assert!(file.verify_payload_sha256().is_err(), "offset {at}");
        }
    }
    // 数値の節の中身を確実に変える
    let nodeofs_at = {
        let file = BinFile::parse(&bytes).unwrap();
        file.section("NODEOFS").unwrap().bytes.as_ptr() as usize - bytes.as_ptr() as usize
    };
    bytes[nodeofs_at] ^= 0x40;
    let file = BinFile::parse(&bytes).unwrap();
    assert!(file.verify_payload_sha256().is_err());

    // ヘッダーのハッシュを書き換えても検出する
    let mut bytes = good.clone();
    bytes[24] ^= 0xff;
    let file = BinFile::parse(&bytes).unwrap();
    assert!(file.verify_payload_sha256().is_err());
}

#[test]
fn payload_hash_is_hex_of_sha256_after_the_section_table() {
    use sha2::{Digest, Sha256};
    let bytes = sample();
    let header_len = 64 + 32 * 10;
    let expected: String = Sha256::digest(&bytes[header_len..])
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(BinFile::parse(&bytes).unwrap().payload_sha256(), expected);
}

#[test]
fn empty_file_with_no_sections() {
    let bytes = BinFileWriter::new(MAGIC, 1).finish();
    assert_eq!(bytes.len(), 64);
    let file = BinFile::parse(&bytes).unwrap();
    assert_eq!(file.section_count(), 0);
    file.verify_payload_sha256().unwrap();
}
