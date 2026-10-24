//! CSVの文字コード判定（behavior.md 2章）。原本CSVと祝日表が同じ規則を使う。

use std::fmt;

/// 判定に使う先頭の行数。
const SNIFF_LINES: usize = 5;
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

/// 採用した文字コード。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    /// BOMの有無を問わないUTF-8。
    Utf8Sig,
    Cp932,
}

impl TextEncoding {
    /// metadataに書く名前。
    pub const fn as_str(self) -> &'static str {
        match self {
            TextEncoding::Utf8Sig => "utf-8-sig",
            TextEncoding::Cp932 => "cp932",
        }
    }
}

/// 復号の失敗。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// 先頭5行がUTF-8でもcp932でも復号できない。
    Unrecognized,
    /// 先頭5行より後で復号できない（行の除外にせず、ファイル処理の失敗にする）。
    Invalid(TextEncoding),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Unrecognized => write!(f, "文字コードを判定できません"),
            DecodeError::Invalid(e) => write!(f, "{}として復号できない箇所があります", e.as_str()),
        }
    }
}

impl std::error::Error for DecodeError {}

fn decode_cp932(bytes: &[u8]) -> Option<String> {
    encoding_rs::SHIFT_JIS
        .decode_without_bom_handling_and_without_replacement(bytes)
        .map(|s| s.into_owned())
}

fn decode_utf8_sig(bytes: &[u8]) -> Option<String> {
    let body = bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes);
    std::str::from_utf8(body).ok().map(str::to_owned)
}

/// 先頭5行（完全な行単位のバイト列）から文字コードを決める。
/// `utf-8-sig` で復号できればそれを、失敗したらcp932で復号できることを確かめて採用する。
pub fn detect_encoding(bytes: &[u8]) -> Result<TextEncoding, DecodeError> {
    let mut end = 0;
    for _ in 0..SNIFF_LINES {
        match bytes[end..].iter().position(|&b| b == b'\n') {
            Some(i) => end += i + 1,
            None => {
                end = bytes.len();
                break;
            }
        }
    }
    let head = &bytes[..end];
    if decode_utf8_sig(head).is_some() {
        Ok(TextEncoding::Utf8Sig)
    } else if decode_cp932(head).is_some() {
        Ok(TextEncoding::Cp932)
    } else {
        Err(DecodeError::Unrecognized)
    }
}

/// 文字コードを判定して全体を復号する。
pub fn decode_text(bytes: &[u8]) -> Result<(TextEncoding, String), DecodeError> {
    let encoding = detect_encoding(bytes)?;
    let text = match encoding {
        TextEncoding::Utf8Sig => decode_utf8_sig(bytes),
        TextEncoding::Cp932 => decode_cp932(bytes),
    };
    text.map(|t| (encoding, t))
        .ok_or(DecodeError::Invalid(encoding))
}
