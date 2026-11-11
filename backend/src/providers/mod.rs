//! 外部提供元のアダプタ（担当：③）。
//!
//! 経路（`contract::Router` の実装）・住所検索・気象。`domain`・`data` に依存しない。
//! 全呼出しが提供元ごとの上流ゲート（[`gate`]）を通ってから送信する。
//! 安全危険情報（口コミ・投票）の保存（[`community`]）もここに置く。

pub mod clock;
pub mod community;
pub mod community_firestore;
pub mod gate;
pub mod gate_firestore;
pub mod geoapify;
pub mod http;
pub mod open_meteo;
pub mod osrm;
