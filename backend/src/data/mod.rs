//! CSV前処理・起動時ロード・祝日表の読込（担当：②）。
//!
//! ③は起動時に `data/current` を一度だけresolveした同じreleaseディレクトリを
//! [`load_calendar`] と [`load_engine`] に渡す。祝日表を読むのはこのモジュールだけで、
//! 前処理コマンドも同じ読込関数と同じ `Calendar` 実装を使う。
//! `contract` 以外の `api`・`providers`・`testing` やHTTP/非同期ランタイムに依存しない。

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use crate::contract::{Calendar, RouteEngine};

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
///
/// ②の実装までの仮実装で、常に失敗する。
pub fn load_calendar(release_dir: &Path) -> Result<Arc<dyn Calendar>, LoadError> {
    let _ = release_dir;
    Err(LoadError::Invalid("祝日表の読込は未実装です".to_owned()))
}

/// 事故データを全件ロード・検証してエンジンを作る。
///
/// ②の実装までの仮実装で、常に失敗する。
pub fn load_engine(release_dir: &Path) -> Result<Arc<dyn RouteEngine>, LoadError> {
    let _ = release_dir;
    Err(LoadError::Invalid(
        "事故データのロードは未実装です".to_owned(),
    ))
}
