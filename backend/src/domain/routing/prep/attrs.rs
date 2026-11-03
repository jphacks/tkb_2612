//! 中間生成物 `graph_attrs.bin`（形式書16章）の中身と、タグの値の解釈。
//!
//! 前処理だけで使う辺の属性。releaseには入れない。辺ごとの配列は `EdgeId` を添字にする。
//! 形式書16章の節に加えて、`NODEID`（ノードのOSMノードID、ノード番号順）を持つ。P2が
//! relationの via ノードをグラフのノードに対応付けるために使う。

use std::ops::Range;

use serde_json::Value;

/// 文字列表のオフセット（`REF`・`NAME` の値）。0は空の文字列（タグなし）。
pub type StrId = u32;

/// `graph_attrs.bin` の中身。
#[derive(Debug, Clone, PartialEq)]
pub struct GraphAttrs {
    /// `PARAMS`（JSON）。
    pub params: Value,
    /// `WAYID`：辺の元になったOSMのway ID。辺番号の順（way IDの昇順、way内は先頭から）。
    pub way_id: Vec<i64>,
    /// `NODEID`：ノードのOSMノードID。ノード番号の順（昇順）。
    pub node_id: Vec<i64>,
    /// `REF`：`ref` タグの文字列表のオフセット。
    pub ref_idx: Vec<StrId>,
    /// `NAME`：`name` タグの文字列表のオフセット。
    pub name_idx: Vec<StrId>,
    /// `STRTAB`：`\0` で終わる文字列を並べた表。先頭の1バイトは空の文字列。
    pub strtab: Vec<u8>,
    /// `LANES`：`lanes` タグ。0は不明。
    pub lanes: Vec<u8>,
    /// `WIDTHDM`：`width` タグ（デシメートル）。0は不明。
    pub width_dm: Vec<u16>,
    /// `LAYER`：`layer` タグ。
    pub layer: Vec<i8>,
}

impl GraphAttrs {
    /// 文字列表のオフセットから文字列を引く。範囲外・UTF-8でない場合は空の文字列。
    pub fn string(&self, id: StrId) -> &str {
        let start = id as usize;
        if start >= self.strtab.len() {
            return "";
        }
        let end = self.strtab[start..]
            .iter()
            .position(|&b| b == 0)
            .map_or(self.strtab.len(), |p| start + p);
        std::str::from_utf8(&self.strtab[start..end]).unwrap_or("")
    }

    /// OSMのway IDから、そのwayの辺の範囲（辺番号）。グラフにないwayは空の範囲。
    pub fn way_edges(&self, way: i64) -> Range<usize> {
        let start = self.way_id.partition_point(|&w| w < way);
        let end = self.way_id.partition_point(|&w| w <= way);
        start..end
    }

    /// OSMのノードIDから、グラフのノード番号。グラフのノードでなければ `None`。
    pub fn node_by_osm_id(&self, node: i64) -> Option<u32> {
        self.node_id.binary_search(&node).ok().map(|i| i as u32)
    }
}

/// 文字列表の組み立て。同じ文字列は同じオフセットになる。
#[derive(Debug, Clone)]
pub struct StringInterner {
    map: std::collections::HashMap<Box<str>, StrId>,
    buf: Vec<u8>,
}

impl Default for StringInterner {
    fn default() -> Self {
        Self::new()
    }
}

impl StringInterner {
    pub fn new() -> Self {
        // 先頭の1バイトが空の文字列（オフセット0）
        Self {
            map: std::collections::HashMap::new(),
            buf: vec![0],
        }
    }

    /// 文字列を加えてオフセットを返す。空の文字列は0。`\0` は空白に置き換える。
    pub fn intern(&mut self, s: &str) -> StrId {
        if s.is_empty() {
            return 0;
        }
        if let Some(&id) = self.map.get(s) {
            return id;
        }
        let id = u32::try_from(self.buf.len()).expect("文字列表がu32に収まる");
        let cleaned = s.replace('\0', " ");
        self.buf.extend_from_slice(cleaned.as_bytes());
        self.buf.push(0);
        self.map.insert(s.into(), id);
        id
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

/// `lanes` タグ。1〜255の整数だけを受け入れる。`2;3` や `2.5`、数字以外は0（不明）。
pub fn parse_lanes(value: &str) -> u8 {
    match value.trim().parse::<u32>() {
        Ok(n @ 1..=255) => n as u8,
        _ => 0,
    }
}

/// `width` タグ（m）をデシメートルにする。`3.5`、`3.5 m` を受け入れる。0以下、6553.5超、
/// 単位が `m` でないもの（`12'6"` など）は0（不明）。
pub fn parse_width_dm(value: &str) -> u16 {
    let value = value.trim();
    let end = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(end);
    if !matches!(unit.trim(), "" | "m") {
        return 0;
    }
    let Ok(meters) = number.parse::<f64>() else {
        return 0;
    };
    let dm = (meters * 10.0).round();
    if (1.0..=f64::from(u16::MAX)).contains(&dm) {
        dm as u16
    } else {
        0
    }
}

/// `layer` タグ。整数を `i8` に収める。解釈できなければ0。
pub fn parse_layer(value: &str) -> i8 {
    value
        .trim()
        .parse::<i64>()
        .map_or(0, |n| n.clamp(i64::from(i8::MIN), i64::from(i8::MAX)) as i8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lanes_width_layer_parsing() {
        assert_eq!(parse_lanes("2"), 2);
        assert_eq!(parse_lanes(" 4 "), 4);
        for v in ["0", "", "2;3", "2.5", "abc", "-1", "256"] {
            assert_eq!(parse_lanes(v), 0, "{v:?}");
        }
        assert_eq!(parse_width_dm("5.5"), 55);
        assert_eq!(parse_width_dm("5.5 m"), 55);
        assert_eq!(parse_width_dm("12"), 120);
        for v in ["", "0", "abc", "12'6\"", "5 ft", "-3", "99999", "5;6"] {
            assert_eq!(parse_width_dm(v), 0, "{v:?}");
        }
        assert_eq!(parse_layer("1"), 1);
        assert_eq!(parse_layer("-2"), -2);
        assert_eq!(parse_layer("1000"), 127);
        assert_eq!(parse_layer("-1000"), -128);
        assert_eq!(parse_layer("x"), 0);
        assert_eq!(parse_layer("1;2"), 0);
    }

    #[test]
    fn interner_reuses_offsets_and_zero_is_empty() {
        let mut t = StringInterner::new();
        assert_eq!(t.intern(""), 0);
        let a = t.intern("国道1号");
        let b = t.intern("main");
        assert_eq!(t.intern("国道1号"), a);
        assert_ne!(a, b);
        assert_eq!(a, 1);
        let attrs = GraphAttrs {
            params: Value::Null,
            way_id: vec![10, 10, 20, 30, 30, 30],
            node_id: vec![5, 7, 9],
            ref_idx: vec![],
            name_idx: vec![],
            strtab: t.finish(),
            lanes: vec![],
            width_dm: vec![],
            layer: vec![],
        };
        assert_eq!(attrs.string(0), "");
        assert_eq!(attrs.string(a), "国道1号");
        assert_eq!(attrs.string(b), "main");
        assert_eq!(attrs.string(9999), "");
        assert_eq!(attrs.way_edges(10), 0..2);
        assert_eq!(attrs.way_edges(20), 2..3);
        assert_eq!(attrs.way_edges(30), 3..6);
        assert_eq!(attrs.way_edges(15), 2..2);
        assert_eq!(attrs.node_by_osm_id(7), Some(1));
        assert_eq!(attrs.node_by_osm_id(8), None);
    }
}
