//! Axumハンドラ・DTO・エラー変換（担当：③）。
//!
//! JSONの検証・正規化で `contract::RouteQuery` を作り、`contract::RouteOutcome` を
//! OpenAPIのDTOへ変換する。`domain`・`data` の内部型には依存せず、`contract` と
//! `data::load_calendar` / `data::load_engine` だけを使う。
