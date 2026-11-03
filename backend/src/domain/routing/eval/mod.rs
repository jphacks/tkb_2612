//! 評価用の補助と計算（M5）。PBFの読み取りは `data::osm` が、ファイルの読み書きとJSONの組み立ては
//! `src/bin/routing_eval.rs` が行う。
//!
//! - `region`：行政界のリングの組み立て・内外判定・GeoJSONの読み書き（M5.1・M5.2）。
//! - `metrics`：方式に依存しない物差し `y-metrics`（Y1・Y2・Y3。M5.4）。
//! - `matching`：現行方式の経路を自前のグラフの辺の列に写す `match-routes`（M5.4）。
//! - `geometry`：`metrics`・`matching` が使う幾何の補助。

pub mod geometry;
pub mod matching;
pub mod metrics;
pub mod region;
