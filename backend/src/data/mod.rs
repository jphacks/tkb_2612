//! CSV前処理・起動時ロード・祝日表の読込（担当：②）。
//!
//! ③は起動時に `ACCIDENT_DATA_DIR` を一度だけresolveした同じreleaseディレクトリを
//! [`load_calendar`] と [`load_engine`] に渡す。祝日表を読むのはこのモジュールだけで、
//! 前処理コマンドも同じ読込関数と同じ `Calendar` 実装を使う。
//! `contract` と `domain` に依存してよい（`domain` は `data` に依存しない）。
//! `api`・`providers`・`testing` やHTTP/非同期ランタイムには依存しない。

pub mod csv_syntax;
pub mod encoding;
pub mod holidays;
pub mod load;
pub mod preprocess;
pub mod release;

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use crate::contract::{Calendar, RouteEngine};
use crate::domain::AccidentEngine;

/// releaseの読込・検証の失敗。起動を止める。
#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    /// 形式・ハッシュ・件数・列挙値の不一致等。メッセージに位置や原本行を含めない。
    Invalid(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "releaseを読み込めません: {e}"),
            LoadError::Invalid(m) => write!(f, "releaseが不正です: {m}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<std::io::Error> for LoadError {
    fn from(value: std::io::Error) -> Self {
        LoadError::Io(value)
    }
}

/// releaseに同梱した祝日表から判定器を作る。事故データより先に読み、
/// 事故データの準備中でも `/api/conditions` が応答できるようにする。
/// metadataの祝日表ハッシュと照合する。
pub fn load_calendar(release_dir: &Path) -> Result<Arc<dyn Calendar>, LoadError> {
    Ok(Arc::new(load::load_holiday_calendar(release_dir)?))
}

/// 事故データを全件ロード・検証してエンジンを作る。
///
/// gzipと祝日表の実ファイルから `dataset_id`・`release_id` を再計算してmetadataと照合し、
/// 一致しなければ失敗する。
pub fn load_engine(release_dir: &Path) -> Result<Arc<dyn RouteEngine>, LoadError> {
    let (snapshot, dataset) = load::load_snapshot(release_dir)?;
    Ok(Arc::new(AccidentEngine::new(Arc::new(snapshot), dataset)))
}
