//! 前処理の純粋なロジック（担当：②、形式書6〜11章・16章）。
//!
//! PBFや `.bin` の読み書きは `data`（`data::osm`・`data::routing_files`）が行い、ここには
//! メモリ上の表を入力にとる関数だけを置く。`routing_prep` のサブコマンドとテストが同じ関数を使う。
//!
//! | モジュール | 内容 | 段階 |
//! |---|---|---|
//! | `graph_build` | wayの表とノードの表から、辺の分割・CSR・形状点を作る | P1 |
//! | `attrs` | 中間生成物 `graph_attrs.bin` の中身（形式書16章） | P1 |
//! | `restrict` | turn restrictionのrelationを有向辺の列にする | P2 |
//! | `topo` | 吸着可能な辺、交差点クラスタ、双子、歩道の親 | P3 |
//! | `odindex` | 吸着用の格子 | P7 |
//! | `assign`・`assign_tables`・`assign_audit`・`assign_review` | 事故の割り当て、尤度表、集計と検査、確認用の抽出 | P5 |
//! | `geom`・`grid`・`view` | 座標・格子・グラフの読み取りの補助 | — |

pub mod assign;
pub mod assign_audit;
pub mod assign_review;
pub mod assign_tables;
pub mod attrs;
pub mod geom;
pub mod graph_build;
pub mod grid;
pub mod odindex;
pub mod restrict;
pub mod risk_fit;
pub mod time_defaults;
pub mod topo;
pub mod view;
