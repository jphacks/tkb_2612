//! 8つの `.bin` に共通する容器形式の読み書き（形式書3章）。
//!
//! ヘッダー64バイト、節表32バイト×節の数、節の本体（各節は8バイト境界から始まり、
//! 節の間の詰め物は0）の順に並ぶ。数値はすべてリトルエンディアン。
//! 節の中身（構造体の並び）はこのモジュールでは解釈せず、`routing_files` が解釈する。

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::LoadError;

/// ヘッダーの長さ（形式書3.1節）。
pub const HEADER_LEN: usize = 64;
/// 節表の1行の長さ（形式書3.2節）。
pub const SECTION_ENTRY_LEN: usize = 32;
/// 節の開始位置の境界（バイト）。
pub const SECTION_ALIGN: usize = 8;
/// 節の名前の最大長（バイト）。
pub const SECTION_NAME_LEN: usize = 8;
/// 共通の節：入力ファイルのハッシュ（形式書3.3節）。
pub const DEPS_SECTION: &str = "DEPS";
/// 共通の節：作成時のパラメータ（形式書3.3節）。
pub const PARAMS_SECTION: &str = "PARAMS";

fn invalid(message: impl Into<String>) -> LoadError {
    LoadError::Invalid(message.into())
}

fn align_up(n: usize) -> usize {
    n.div_ceil(SECTION_ALIGN) * SECTION_ALIGN
}

/// 生の32バイトのSHA-256。
fn sha256_raw(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// リトルエンディアンで節に出し入れできる数値型。
pub trait LeValue: Copy {
    /// 1要素のバイト数。
    const SIZE: usize;
    fn write_le(self, out: &mut Vec<u8>);
    /// `bytes` は長さ `SIZE` ちょうど。
    fn read_le(bytes: &[u8]) -> Self;
}

macro_rules! le_value {
    ($($t:ty),*) => {$(
        impl LeValue for $t {
            const SIZE: usize = std::mem::size_of::<$t>();
            fn write_le(self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
            fn read_le(bytes: &[u8]) -> Self {
                Self::from_le_bytes(bytes.try_into().expect("長さはSIZE"))
            }
        }
    )*};
}

le_value!(u8, i8, u16, i16, u32, i32, u64, i64, f32, f64);

struct PendingSection {
    name: [u8; SECTION_NAME_LEN],
    elem_size: u32,
    count: u64,
    bytes: Vec<u8>,
}

/// 節を足していき、`finish` で1つのファイルのバイト列にする。
pub struct BinFileWriter {
    magic: [u8; 8],
    format_version: u32,
    sections: Vec<PendingSection>,
}

/// 節の名前の検査。ASCIIの印字可能文字で1〜8文字。
fn encode_name(name: &str) -> Option<[u8; SECTION_NAME_LEN]> {
    let bytes = name.as_bytes();
    if bytes.is_empty()
        || bytes.len() > SECTION_NAME_LEN
        || !bytes.iter().all(|b| (0x21..=0x7e).contains(b))
    {
        return None;
    }
    let mut out = [0u8; SECTION_NAME_LEN];
    out[..bytes.len()].copy_from_slice(bytes);
    Some(out)
}

impl BinFileWriter {
    pub fn new(magic: [u8; 8], format_version: u32) -> Self {
        Self {
            magic,
            format_version,
            sections: Vec::new(),
        }
    }

    /// 節を足す。節表の順は足した順になる。
    ///
    /// # Panics
    /// 書き手のプログラムの誤りとして、次の場合に止まる：名前が1〜8文字のASCII印字可能文字でない、
    /// 同じ名前の節がすでにある、`elem_size` が0、`bytes.len() != count × elem_size`。
    pub fn add_section(&mut self, name: &str, elem_size: u32, count: u64, bytes: Vec<u8>) {
        let encoded = encode_name(name).unwrap_or_else(|| panic!("節の名前が不正です: {name:?}"));
        assert!(
            self.sections.iter().all(|s| s.name != encoded),
            "節の名前が重複しています: {name}"
        );
        assert!(elem_size > 0, "elem_sizeが0です: {name}");
        assert_eq!(
            bytes.len() as u64,
            count * u64::from(elem_size),
            "節の長さがcount×elem_sizeと一致しません: {name}"
        );
        self.sections.push(PendingSection {
            name: encoded,
            elem_size,
            count,
            bytes,
        });
    }

    /// JSONの節（`elem_size` は1、`count` はバイト数）を足す。キーはserde_jsonの順（辞書順）で、
    /// 同じ値からは同じバイト列になる。
    pub fn add_json(&mut self, name: &str, value: &Value) {
        let bytes = serde_json::to_vec(value).expect("JSONに書ける値");
        self.add_section(name, 1, bytes.len() as u64, bytes);
    }

    /// 数値の配列の節を足す（`elem_size` は型の大きさ）。
    pub fn add_values<T: LeValue>(&mut self, name: &str, values: &[T]) {
        let mut bytes = Vec::with_capacity(values.len() * T::SIZE);
        for &v in values {
            v.write_le(&mut bytes);
        }
        self.add_section(name, T::SIZE as u32, values.len() as u64, bytes);
    }

    /// ファイルのバイト列を組み立てる。`payload_sha256` を計算してヘッダーに書く。
    pub fn finish(self) -> Vec<u8> {
        let header_len = HEADER_LEN + SECTION_ENTRY_LEN * self.sections.len();
        let mut body: Vec<u8> = Vec::new();
        let mut table: Vec<u8> = Vec::with_capacity(SECTION_ENTRY_LEN * self.sections.len());
        for section in &self.sections {
            // 節の開始位置は8の倍数。詰め物は0（空の節でも、開始位置までは埋める）
            let start = align_up(header_len + body.len());
            body.resize(start - header_len, 0);
            table.extend_from_slice(&section.name);
            table.extend_from_slice(&(start as u64).to_le_bytes());
            table.extend_from_slice(&section.count.to_le_bytes());
            table.extend_from_slice(&section.elem_size.to_le_bytes());
            table.extend_from_slice(&0u32.to_le_bytes());
            body.extend_from_slice(&section.bytes);
        }
        // payload_sha256 は header_len 以降（節の本体と詰め物）だけが対象。節表は含めない
        let digest = sha256_raw(&body);

        let mut out = Vec::with_capacity(header_len + body.len());
        out.extend_from_slice(&self.magic);
        out.extend_from_slice(&self.format_version.to_le_bytes());
        out.extend_from_slice(&(self.sections.len() as u32).to_le_bytes());
        out.extend_from_slice(&(header_len as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&digest);
        out.extend_from_slice(&[0u8; 8]);
        debug_assert_eq!(out.len(), HEADER_LEN);
        out.extend_from_slice(&table);
        debug_assert_eq!(out.len(), header_len);
        out.extend_from_slice(&body);
        out
    }
}

/// 節表の1行（検査済み）。
#[derive(Debug, Clone)]
pub(crate) struct SectionEntry {
    pub(crate) name: String,
    pub(crate) start: usize,
    pub(crate) count: u64,
    pub(crate) elem_size: u32,
}

impl SectionEntry {
    fn len(&self) -> usize {
        self.count as usize * self.elem_size as usize
    }

    pub(crate) fn expect_elem_size(&self, elem_size: u32) -> Result<(), LoadError> {
        if self.elem_size != elem_size {
            return Err(invalid(format!(
                "節{}のelem_sizeが{}ではない",
                self.name, elem_size
            )));
        }
        Ok(())
    }

    pub(crate) fn expect_count(&self, count: u64) -> Result<(), LoadError> {
        if self.count != count {
            return Err(invalid(format!(
                "節{}の件数{}が期待の{}と一致しない",
                self.name, self.count, count
            )));
        }
        Ok(())
    }
}

/// 検査済みの `.bin`。節の本体は元のバイト列を借りる（コピーしない）。
#[derive(Debug, Clone)]
pub struct BinFile<'a> {
    bytes: &'a [u8],
    magic: [u8; 8],
    format_version: u32,
    header_len: usize,
    payload_sha256: [u8; 32],
    sections: Vec<SectionEntry>,
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4バイト"))
}

fn read_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("8バイト"))
}

impl<'a> BinFile<'a> {
    /// ヘッダー・節表を検査して開く。
    ///
    /// 検査：ファイルの長さ、`header_len = 64 + 32×section_count`、予約領域が0、節の名前、
    /// 開始位置が8の倍数、範囲がファイル内、節が節表の順に並んで重ならない、詰め物が0、名前の重複。
    /// マジックナンバーと `format_version` の期待値は [`BinFile::expect`] で確かめる。
    pub fn parse(bytes: &'a [u8]) -> Result<BinFile<'a>, LoadError> {
        if bytes.len() < HEADER_LEN {
            return Err(invalid("binファイルがヘッダーの長さに満たない"));
        }
        let mut magic = [0u8; 8];
        magic.copy_from_slice(&bytes[0..8]);
        let format_version = read_u32(bytes, 8);
        let section_count = read_u32(bytes, 12) as usize;
        let header_len = read_u32(bytes, 16) as usize;
        if header_len != HEADER_LEN + SECTION_ENTRY_LEN * section_count {
            return Err(invalid("binファイルのheader_lenが節の数と一致しない"));
        }
        if header_len > bytes.len() {
            return Err(invalid("binファイルが節表の途中で終わっている"));
        }
        if read_u32(bytes, 20) != 0 || bytes[56..64].iter().any(|&b| b != 0) {
            return Err(invalid("binファイルのヘッダーの予約領域が0でない"));
        }
        let mut payload_sha256 = [0u8; 32];
        payload_sha256.copy_from_slice(&bytes[24..56]);

        let mut sections: Vec<SectionEntry> = Vec::with_capacity(section_count);
        // 直前の節の終わり。最初の節はヘッダーの直後から
        let mut prev_end = header_len;
        for i in 0..section_count {
            let at = HEADER_LEN + SECTION_ENTRY_LEN * i;
            let raw_name = &bytes[at..at + SECTION_NAME_LEN];
            let name_len = raw_name
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(SECTION_NAME_LEN);
            if name_len == 0
                || !raw_name[..name_len]
                    .iter()
                    .all(|b| (0x21..=0x7e).contains(b))
                || raw_name[name_len..].iter().any(|&b| b != 0)
            {
                return Err(invalid("binファイルの節の名前が不正"));
            }
            let name = String::from_utf8(raw_name[..name_len].to_vec()).expect("ASCII");
            if sections.iter().any(|s| s.name == name) {
                return Err(invalid(format!(
                    "binファイルの節の名前が重複している: {name}"
                )));
            }
            let start = read_u64(bytes, at + 8);
            let count = read_u64(bytes, at + 16);
            let elem_size = read_u32(bytes, at + 24);
            if read_u32(bytes, at + 28) != 0 {
                return Err(invalid(format!("節{name}の予約領域が0でない")));
            }
            if elem_size == 0 {
                return Err(invalid(format!("節{name}のelem_sizeが0")));
            }
            if start & (SECTION_ALIGN as u64 - 1) != 0 {
                return Err(invalid(format!("節{name}の開始位置が8バイト境界でない")));
            }
            let len = count
                .checked_mul(u64::from(elem_size))
                .ok_or_else(|| invalid(format!("節{name}の長さが大きすぎる")))?;
            let end = start
                .checked_add(len)
                .filter(|&end| end <= bytes.len() as u64)
                .ok_or_else(|| invalid(format!("節{name}がファイルの範囲外にある")))?;
            if start < prev_end as u64 {
                return Err(invalid(format!(
                    "節{name}が前の節と重なっている（または節表の順でない）"
                )));
            }
            // 範囲がファイル内に収まったので usize に変換できる
            let (start, end) = (start as usize, end as usize);
            if bytes[prev_end..start].iter().any(|&b| b != 0) {
                return Err(invalid(format!("節{name}の前の詰め物が0でない")));
            }
            sections.push(SectionEntry {
                name,
                start,
                count,
                elem_size,
            });
            prev_end = end;
        }
        if bytes[prev_end..].iter().any(|&b| b != 0) {
            return Err(invalid("binファイルの末尾に節の外のデータがある"));
        }
        Ok(BinFile {
            bytes,
            magic,
            format_version,
            header_len,
            payload_sha256,
            sections,
        })
    }

    /// マジックナンバーと `format_version` が期待どおりか確かめる。
    pub fn expect(&self, magic: [u8; 8], format_version: u32) -> Result<(), LoadError> {
        if self.magic != magic {
            return Err(invalid(format!(
                "マジックナンバーが{}ではない",
                String::from_utf8_lossy(&magic)
            )));
        }
        if self.format_version != format_version {
            return Err(invalid(format!(
                "{}のformat_versionが{}ではない",
                String::from_utf8_lossy(&magic),
                format_version
            )));
        }
        Ok(())
    }

    pub fn magic(&self) -> [u8; 8] {
        self.magic
    }

    pub fn format_version(&self) -> u32 {
        self.format_version
    }

    pub fn section_count(&self) -> usize {
        self.sections.len()
    }

    /// ヘッダーに書かれた `payload_sha256`（小文字hex）。
    pub fn payload_sha256(&self) -> String {
        hex(&self.payload_sha256)
    }

    /// 節名→`count`（`metadata.json` の `routing.files[].sections` の元）。
    pub fn section_counts(&self) -> BTreeMap<String, u64> {
        self.sections
            .iter()
            .map(|s| (s.name.clone(), s.count))
            .collect()
    }

    /// 名前で節を引く。
    pub fn section(&self, name: &str) -> Option<SectionRef<'_>> {
        let entry = self.sections.iter().find(|s| s.name == name)?;
        Some(SectionRef {
            name: &entry.name,
            count: entry.count,
            elem_size: entry.elem_size,
            bytes: &self.bytes[entry.start..entry.start + entry.len()],
        })
    }

    /// 必須の節を引く。ないときは `LoadError::Invalid`。
    pub fn require(&self, name: &str) -> Result<SectionRef<'_>, LoadError> {
        self.section(name)
            .ok_or_else(|| invalid(format!("必須の節{name}がない")))
    }

    /// JSONの節をJSONオブジェクトとして読む。
    pub fn json(&self, name: &str) -> Result<Value, LoadError> {
        let section = self.require(name)?;
        if section.elem_size != 1 {
            return Err(invalid(format!("JSONの節{name}のelem_sizeが1でない")));
        }
        let value: Value = serde_json::from_slice(section.bytes)
            .map_err(|_| invalid(format!("節{name}がJSONとして読めない")))?;
        if !value.is_object() {
            return Err(invalid(format!("節{name}がJSONオブジェクトでない")));
        }
        Ok(value)
    }

    /// `DEPS` 節：入力ファイル名→SHA-256（小文字hex）。
    pub fn deps(&self) -> Result<BTreeMap<String, String>, LoadError> {
        let value = self.json(DEPS_SECTION)?;
        let object = value.as_object().expect("jsonがobjectを確かめた");
        let mut deps = BTreeMap::new();
        for (file, hash) in object {
            let hash = hash
                .as_str()
                .ok_or_else(|| invalid(format!("DEPSの{file}の値が文字列でない")))?;
            deps.insert(file.clone(), hash.to_owned());
        }
        Ok(deps)
    }

    /// `header_len` からファイル末尾までのSHA-256が、ヘッダーの値と一致するか確かめる。
    /// 起動時には呼ばず、前処理の各段階と `package` が入力に対して呼ぶ（形式書4.3節）。
    pub fn verify_payload_sha256(&self) -> Result<(), LoadError> {
        if sha256_raw(&self.bytes[self.header_len..]) != self.payload_sha256 {
            return Err(invalid("payload_sha256が内容と一致しない"));
        }
        Ok(())
    }
}

/// 大きな `.bin` を、ファイル全体を `Vec<u8>` に読まずに節ごとに読むための読み取り器。
///
/// ヘッダーと節表だけを先に読み、節の本体は [`read_values`](Self::read_values)・
/// [`read_mapped`](Self::read_mapped) で必要な型の `Vec` に直接読み込む。形式書3章の容器の検査
/// （節表・詰め物・末尾）は [`BinFile::parse`] と同じものを行う。
pub(crate) struct BinFileReader {
    file: File,
    magic: [u8; 8],
    format_version: u32,
    sections: Vec<SectionEntry>,
}

fn invalid_reader(message: impl Into<String>) -> LoadError {
    LoadError::Invalid(message.into())
}

/// `file` の `[start, end)` がすべて0か確かめる。
fn check_zero_range(file: &mut File, start: u64, end: u64) -> Result<(), LoadError> {
    if start == end {
        return Ok(());
    }
    file.seek(SeekFrom::Start(start))?;
    let mut remaining = end - start;
    let mut buffer = [0_u8; 8192];
    while remaining > 0 {
        let n = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..n])?;
        if buffer[..n].iter().any(|&b| b != 0) {
            return Err(invalid_reader("binファイルの詰め物または末尾が0でない"));
        }
        remaining -= n as u64;
    }
    Ok(())
}

impl BinFileReader {
    /// ファイルを開き、ヘッダー・節表を検査する。検査の内容は [`BinFile::parse`] と同じ。
    pub(crate) fn open(path: &Path) -> Result<BinFileReader, LoadError> {
        let mut file = File::open(path)?;
        let file_len = file.metadata()?.len();
        if file_len < HEADER_LEN as u64 {
            return Err(invalid_reader("binファイルがヘッダーの長さに満たない"));
        }
        let mut header = [0_u8; HEADER_LEN];
        file.read_exact(&mut header)?;
        let mut magic = [0_u8; 8];
        magic.copy_from_slice(&header[0..8]);
        let format_version = read_u32(&header, 8);
        let section_count = read_u32(&header, 12) as usize;
        let header_len = read_u32(&header, 16) as usize;
        if header_len != HEADER_LEN + SECTION_ENTRY_LEN * section_count {
            return Err(invalid_reader(
                "binファイルのheader_lenが節の数と一致しない",
            ));
        }
        if header_len as u64 > file_len {
            return Err(invalid_reader("binファイルが節表の途中で終わっている"));
        }
        if read_u32(&header, 20) != 0 || header[56..64].iter().any(|&b| b != 0) {
            return Err(invalid_reader("binファイルのヘッダーの予約領域が0でない"));
        }

        let mut table = vec![0_u8; header_len - HEADER_LEN];
        file.read_exact(&mut table)?;
        let mut sections: Vec<SectionEntry> = Vec::with_capacity(section_count);
        let mut prev_end = header_len as u64;
        for i in 0..section_count {
            let at = SECTION_ENTRY_LEN * i;
            let raw_name = &table[at..at + SECTION_NAME_LEN];
            let name_len = raw_name
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(SECTION_NAME_LEN);
            if name_len == 0
                || !raw_name[..name_len]
                    .iter()
                    .all(|b| (0x21..=0x7e).contains(b))
                || raw_name[name_len..].iter().any(|&b| b != 0)
            {
                return Err(invalid_reader("binファイルの節の名前が不正"));
            }
            let name = String::from_utf8(raw_name[..name_len].to_vec()).expect("ASCII");
            if sections.iter().any(|s| s.name == name) {
                return Err(invalid_reader(format!(
                    "binファイルの節の名前が重複している: {name}"
                )));
            }
            let start = read_u64(&table, at + 8);
            let count = read_u64(&table, at + 16);
            let elem_size = read_u32(&table, at + 24);
            if read_u32(&table, at + 28) != 0 {
                return Err(invalid_reader(format!("節{name}の予約領域が0でない")));
            }
            if elem_size == 0 {
                return Err(invalid_reader(format!("節{name}のelem_sizeが0")));
            }
            if start & (SECTION_ALIGN as u64 - 1) != 0 {
                return Err(invalid_reader(format!(
                    "節{name}の開始位置が8バイト境界でない"
                )));
            }
            let len = count
                .checked_mul(u64::from(elem_size))
                .ok_or_else(|| invalid_reader(format!("節{name}の長さが大きすぎる")))?;
            let end = start
                .checked_add(len)
                .filter(|&end| end <= file_len)
                .ok_or_else(|| invalid_reader(format!("節{name}がファイルの範囲外にある")))?;
            if start < prev_end {
                return Err(invalid_reader(format!(
                    "節{name}が前の節と重なっている（または節表の順でない）"
                )));
            }
            check_zero_range(&mut file, prev_end, start)?;
            sections.push(SectionEntry {
                name,
                start: start as usize,
                count,
                elem_size,
            });
            prev_end = end;
        }
        check_zero_range(&mut file, prev_end, file_len)?;
        Ok(BinFileReader {
            file,
            magic,
            format_version,
            sections,
        })
    }

    /// マジックナンバーと `format_version` が期待どおりか確かめる。
    pub(crate) fn expect(&self, magic: [u8; 8], format_version: u32) -> Result<(), LoadError> {
        if self.magic != magic {
            return Err(invalid_reader(format!(
                "マジックナンバーが{}ではない",
                String::from_utf8_lossy(&magic)
            )));
        }
        if self.format_version != format_version {
            return Err(invalid_reader(format!(
                "{}のformat_versionが{}ではない",
                String::from_utf8_lossy(&magic),
                format_version
            )));
        }
        Ok(())
    }

    pub(crate) fn magic(&self) -> [u8; 8] {
        self.magic
    }

    pub(crate) fn format_version(&self) -> u32 {
        self.format_version
    }

    /// 節名→`count`。
    pub(crate) fn section_counts(&self) -> BTreeMap<String, u64> {
        self.sections
            .iter()
            .map(|s| (s.name.clone(), s.count))
            .collect()
    }

    /// 名前で節を引く（節表の情報だけを返す）。
    pub(crate) fn section(&self, name: &str) -> Option<SectionEntry> {
        self.sections.iter().find(|s| s.name == name).cloned()
    }

    /// 必須の節を引く。
    pub(crate) fn require(&self, name: &str) -> Result<SectionEntry, LoadError> {
        self.section(name)
            .ok_or_else(|| invalid_reader(format!("必須の節{name}がない")))
    }

    /// 節の本体をそのまま読む。
    pub(crate) fn read_section_bytes(
        &mut self,
        section: &SectionEntry,
    ) -> Result<Vec<u8>, LoadError> {
        let len = section.len();
        let mut bytes = vec![0_u8; len];
        self.file.seek(SeekFrom::Start(section.start as u64))?;
        self.file.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    /// 固定長の要素を `map` で型にしながら読む。作業用のバッファは1MB以下。
    pub(crate) fn read_mapped<T>(
        &mut self,
        section: &SectionEntry,
        elem_size: u32,
        mut map: impl FnMut(&[u8]) -> T,
    ) -> Result<Vec<T>, LoadError> {
        if section.elem_size != elem_size {
            return Err(invalid_reader(format!(
                "節{}のelem_sizeが{}ではない",
                section.name, elem_size
            )));
        }
        let size = elem_size as usize;
        let total = section.len();
        if total == 0 {
            return Ok(Vec::new());
        }
        let chunk_bytes = (1_usize << 20) / size * size;
        let mut buffer = vec![0_u8; chunk_bytes.max(size)];
        let mut out = Vec::with_capacity(section.count as usize);
        self.file.seek(SeekFrom::Start(section.start as u64))?;
        let mut remaining = total;
        while remaining > 0 {
            let n = remaining.min(buffer.len());
            self.file.read_exact(&mut buffer[..n])?;
            for record in buffer[..n].chunks_exact(size) {
                out.push(map(record));
            }
            remaining -= n;
        }
        Ok(out)
    }

    /// 数値の配列の節を読む。
    pub(crate) fn read_values<T: LeValue>(
        &mut self,
        section: &SectionEntry,
    ) -> Result<Vec<T>, LoadError> {
        self.read_mapped(section, T::SIZE as u32, T::read_le)
    }

    /// JSONの節をJSONオブジェクトとして読む。
    pub(crate) fn json(&mut self, name: &str) -> Result<Value, LoadError> {
        let section = self.require(name)?;
        if section.elem_size != 1 {
            return Err(invalid_reader(format!(
                "JSONの節{name}のelem_sizeが1でない"
            )));
        }
        let bytes = self.read_section_bytes(&section)?;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| invalid_reader(format!("節{name}がJSONとして読めない")))?;
        if !value.is_object() {
            return Err(invalid_reader(format!("節{name}がJSONオブジェクトでない")));
        }
        Ok(value)
    }

    /// `DEPS` 節：入力ファイル名→SHA-256（小文字hex）。
    pub(crate) fn deps(&mut self) -> Result<BTreeMap<String, String>, LoadError> {
        let value = self.json(DEPS_SECTION)?;
        let object = value.as_object().expect("jsonがobjectを確かめた");
        let mut deps = BTreeMap::new();
        for (file, hash) in object {
            let hash = hash
                .as_str()
                .ok_or_else(|| invalid_reader(format!("DEPSの{file}の値が文字列でない")))?;
            deps.insert(file.clone(), hash.to_owned());
        }
        Ok(deps)
    }
}

/// 節の本体への参照。
#[derive(Debug, Clone, Copy)]
pub struct SectionRef<'a> {
    pub name: &'a str,
    pub count: u64,
    pub elem_size: u32,
    /// 長さは `count × elem_size`。
    pub bytes: &'a [u8],
}

impl SectionRef<'_> {
    /// 要素の大きさが仕様どおりか確かめる。
    pub fn expect_elem_size(&self, elem_size: u32) -> Result<(), LoadError> {
        if self.elem_size != elem_size {
            return Err(invalid(format!(
                "節{}のelem_sizeが{}ではない",
                self.name, elem_size
            )));
        }
        Ok(())
    }

    /// 要素数が期待どおりか確かめる（他の節や `PARAMS` との矛盾を検出する）。
    pub fn expect_count(&self, count: u64) -> Result<(), LoadError> {
        if self.count != count {
            return Err(invalid(format!(
                "節{}の件数{}が期待の{}と一致しない",
                self.name, self.count, count
            )));
        }
        Ok(())
    }

    /// 数値の配列として読む。`elem_size` が型の大きさと違えば失敗する。
    pub fn values<T: LeValue>(&self) -> Result<Vec<T>, LoadError> {
        self.expect_elem_size(T::SIZE as u32)?;
        Ok(self.bytes.chunks_exact(T::SIZE).map(T::read_le).collect())
    }

    /// 固定長の構造体の並びとして、1要素ずつの生バイトを返す。
    pub fn records(&self, elem_size: u32) -> Result<std::slice::ChunksExact<'_, u8>, LoadError> {
        self.expect_elem_size(elem_size)?;
        Ok(self.bytes.chunks_exact(elem_size as usize))
    }

    pub fn u8s(&self) -> Result<Vec<u8>, LoadError> {
        self.values()
    }

    pub fn u16s(&self) -> Result<Vec<u16>, LoadError> {
        self.values()
    }

    pub fn u32s(&self) -> Result<Vec<u32>, LoadError> {
        self.values()
    }

    pub fn u64s(&self) -> Result<Vec<u64>, LoadError> {
        self.values()
    }

    pub fn i16s(&self) -> Result<Vec<i16>, LoadError> {
        self.values()
    }

    pub fn i32s(&self) -> Result<Vec<i32>, LoadError> {
        self.values()
    }

    pub fn f32s(&self) -> Result<Vec<f32>, LoadError> {
        self.values()
    }

    pub fn f64s(&self) -> Result<Vec<f64>, LoadError> {
        self.values()
    }
}
