//! 実サービス・相手側の実装なしで検証するためのスタブとfixture読込（共同所有）。
//!
//! - ②：[`ScriptedRouter`] と [`fixtures`] の固定経路で、OSRMなしに候補生成・集計を検証する。
//! - ③：[`StubEngine`] と [`FixedCalendar`] で、計算本体の完成前にハンドラ・DTO・中断を検証する。
//!
//! 本番の処理からは使わない。

pub mod calendar;
pub mod engine;
pub mod fixtures;
pub mod router;

pub use calendar::FixedCalendar;
pub use engine::{StubEngine, StubEvaluation};
pub use router::{RouterResponse, ScriptedRouter, StallingRouter, valid_routes};
