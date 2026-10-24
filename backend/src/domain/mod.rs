//! 計算エンジン（担当：②）。
//!
//! 候補生成・72m回廊・事故集計・選定・ナビ区間と、時間帯/曜日区分の判定
//! （`contract::Calendar` の唯一の実装）を置く。
//! `contract` 以外の `api`・`providers`・`testing` やHTTP/非同期ランタイムに依存しない。
