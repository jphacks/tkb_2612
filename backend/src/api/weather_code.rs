//! WMO天気コード→天候の変換表（behavior.md 9章、backend-spec.md 4章）。
//!
//! 統括の決定で③が `api` に置く（`api` は `domain` に依存できないため）。提供元を替えても
//! この表を共通に使い、アダプタはWMOコードで返す。表の内容は仕様のとおりで変えない。
//! architecture.md 7章の「（domain）」という配置の表記との差は未決事項。

use crate::contract::Weather;

/// 対応表にないコードは `None`（天候 `unavailable`）。
pub fn weather_from_wmo(code: i64) -> Option<Weather> {
    match code {
        0..=2 => Some(Weather::Clear),
        3 => Some(Weather::Cloudy),
        45 | 48 => Some(Weather::Fog),
        51 | 53 | 55 | 56 | 57 | 61 | 63 | 65 | 66 | 67 | 80 | 81 | 82 | 95 | 96 | 99 => {
            Some(Weather::Rain)
        }
        71 | 73 | 75 | 77 | 85 | 86 => Some(Weather::Snow),
        _ => None,
    }
}
