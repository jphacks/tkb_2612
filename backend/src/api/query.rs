//! GETのクエリ文字列の解析。既知キーの重複は400、未知キーは無視する。

use std::collections::HashMap;

/// 既知キーごとの値（出現順）。
#[derive(Debug, Default)]
pub struct QueryParams {
    values: HashMap<&'static str, Vec<String>>,
}

impl QueryParams {
    /// `known` に含まれるキーだけを集める。
    pub fn parse(query: Option<&str>, known: &[&'static str]) -> Self {
        let mut values: HashMap<&'static str, Vec<String>> = HashMap::new();
        for (key, value) in form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
            if let Some(name) = known.iter().find(|k| **k == key) {
                values.entry(name).or_default().push(value.into_owned());
            }
        }
        Self { values }
    }

    /// 0個なら `Ok(None)`、1個なら `Ok(Some)`、重複は `Err`。
    pub fn single(&self, key: &str) -> Result<Option<&str>, Duplicate> {
        match self.values.get(key).map(Vec::as_slice) {
            None | Some([]) => Ok(None),
            Some([value]) => Ok(Some(value)),
            Some(_) => Err(Duplicate),
        }
    }
}

/// 既知キーが複数回指定された。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Duplicate;
