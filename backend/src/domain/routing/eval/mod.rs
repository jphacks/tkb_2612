//! 評価用の計算（M5.4）。方式に依存しない物差し `y-metrics` と、現行方式の経路を
//! 自前のグラフの辺の列に写す `match-routes` の本体を持つ。
//!
//! ファイルの読み書きとJSONの組み立ては `src/bin/routing_eval.rs` が行い、ここでは
//! `Graph`・`Topology`・`AccidentSnapshot`・`EdgeAccidentIndex` だけを使う。

pub mod geometry;
pub mod matching;
pub mod metrics;
