//! 評価用の補助（M5）。PBFの読み取りは `data::osm` が行い、ここでは座標だけを扱う。
//!
//! `region` は行政界のリングの組み立て・内外判定・GeoJSONの読み書きを持つ。

pub mod region;
