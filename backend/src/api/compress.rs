//! routeの応答のgzip（behavior.md 7章）。
//!
//! gzipを許可する要求には圧縮を必須とし、本文の送信を始める前に全文を圧縮する
//! （圧縮も35秒の期限内に完了させるため。ストリーミング圧縮にしない）。

use std::io::Write;

use axum::http::HeaderMap;
use axum::http::header::ACCEPT_ENCODING;
use flate2::Compression;
use flate2::write::GzEncoder;

use crate::contract::{CancelToken, Interrupt};

/// 一度に圧縮する大きさ。この間隔で期限・取消しを確認する。
const CHUNK_BYTES: usize = 256 * 1024;

/// Accept-Encodingがgzipを許可するか（HTTPの一般規則。`gzip;q=0` は許可しない）。
///
/// `gzip`（`x-gzip` を含む）の指定があればそのq値で、無ければ `*` のq値で決める。
pub fn accepts_gzip(headers: &HeaderMap) -> bool {
    let mut gzip: Option<bool> = None;
    let mut wildcard: Option<bool> = None;
    for value in headers.get_all(ACCEPT_ENCODING) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for item in value.split(',') {
            let mut parts = item.split(';');
            let coding = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
            let allowed = parts
                .find_map(|p| {
                    let (name, q) = p.split_once('=')?;
                    name.trim()
                        .eq_ignore_ascii_case("q")
                        .then(|| q.trim().parse::<f64>().is_ok_and(|q| q > 0.0))
                })
                .unwrap_or(true);
            match coding.as_str() {
                "gzip" | "x-gzip" => gzip = Some(gzip.unwrap_or(false) || allowed),
                "*" => wildcard = Some(wildcard.unwrap_or(false) || allowed),
                _ => {}
            }
        }
    }
    gzip.or(wildcard).unwrap_or(false)
}

/// 全文をgzipにする。期限・取消しを途中で確認する。
pub fn gzip(body: &[u8], cancel: &CancelToken) -> Result<Vec<u8>, Interrupt> {
    let mut encoder = GzEncoder::new(Vec::with_capacity(body.len() / 4), Compression::default());
    for chunk in body.chunks(CHUNK_BYTES) {
        cancel.check()?;
        encoder.write_all(chunk).expect("メモリへの書込み");
    }
    cancel.check()?;
    Ok(encoder.finish().expect("メモリへの書込み"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn accepts(values: &[&str]) -> bool {
        let mut headers = HeaderMap::new();
        for v in values {
            headers.append(ACCEPT_ENCODING, HeaderValue::from_str(v).unwrap());
        }
        accepts_gzip(&headers)
    }

    #[test]
    fn q_values_follow_http_rules() {
        assert!(accepts(&["gzip, deflate, br"]));
        assert!(accepts(&["GZIP;q=0.5"]));
        assert!(accepts(&["*"]));
        assert!(!accepts(&[]));
        assert!(!accepts(&["identity"]));
        assert!(!accepts(&["gzip;q=0"]));
        assert!(!accepts(&["gzip;q=0.000, *"]));
        assert!(!accepts(&["br", "deflate"]));
        assert!(accepts(&["br", "x-gzip"]));
    }
}
