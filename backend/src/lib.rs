//! 事故考慮ルート検索バックエンド。
//!
//! モジュールの担当と依存方向はREADME.mdを参照。
//! `contract` は②と③の共有契約で、どちらの実装にも依存しない。

pub mod api;
pub mod contract;
pub mod data;
pub mod domain;
pub mod providers;
pub mod testing;
