//! 計算エンジン（担当：②）。
//!
//! 候補生成・72m回廊・事故集計・選定・ナビ区間と、時間帯/曜日区分の判定
//! （`contract::Calendar` の唯一の実装）を置く。
//! `contract` 以外の `api`・`providers`・`testing` やHTTP/非同期ランタイムに依存しない。
//! `data` にも依存しない（`data` から `domain` への依存だけを許す）。

pub mod calendar;
pub mod candidates;
pub mod corridor;
pub mod engine;
pub mod navigation;
pub mod rounding;
pub mod scoring;
pub mod snapshot;

pub use calendar::HolidayCalendar;
pub use engine::AccidentEngine;
pub use rounding::py_round;
pub use snapshot::{AccidentColumns, AccidentSnapshot};
