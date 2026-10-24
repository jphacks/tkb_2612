//! 人工の原本CSV（`honhyo_YEAR.csv`）とコード表の照合器。

use std::path::Path;

use accident_route_backend::data::preprocess::codebook::{
    CodebookError, CodebookFile, CodebookVerifier,
};

/// 2025年版の列名（全角空白を含む。前処理は全空白を除いて比べる）。必須列と無関係な列を混ぜる。
pub const HEADER: [&str; 18] = [
    "資料区分",
    "死者数",
    "負傷者数",
    "発生日時　　年",
    "発生日時　　月",
    "発生日時　　日",
    "発生日時　　時",
    "発生日時　　分",
    "昼夜",
    "天候",
    "事故類型",
    "当事者種別（当事者A）",
    "当事者種別（当事者B）",
    "人身損傷程度（当事者A）",
    "人身損傷程度（当事者B）",
    "地点　緯度（北緯）",
    "地点　経度（東経）",
    "祝日(発生年月日)",
];

/// 原本1行（HEADERの順）。
#[derive(Debug, Clone)]
pub struct SourceRow(pub [String; 18]);

impl SourceRow {
    /// 有効な行（東京付近、昼、晴、自動車×歩行者、負傷1人）。
    pub fn valid(year: u16, month: u32, day: u32, hour: u32) -> Self {
        Self(
            [
                "1",
                "0",
                "1",
                &year.to_string(),
                &format!("{month:02}"),
                &format!("{day:02}"),
                &format!("{hour:02}"),
                "30",
                "12",
                "1",
                "21",
                "03",
                "61",
                "4",
                "2",
                "354050000",
                "1394550000",
                "0",
            ]
            .map(str::to_owned),
        )
    }

    pub fn set(mut self, column: &str, value: &str) -> Self {
        let i = HEADER
            .iter()
            .position(|h| *h == column)
            .expect("HEADERの列名");
        self.0[i] = value.to_owned();
        self
    }
}

/// ヘッダと行からCSV文字列を作る。`order` で列の並びを変えられる。
pub fn csv_text(header: &[&str], rows: &[SourceRow], order: Option<&[usize]>) -> String {
    let order: Vec<usize> = order.map_or_else(|| (0..header.len()).collect(), <[usize]>::to_vec);
    let mut text = order
        .iter()
        .map(|&i| header[i])
        .collect::<Vec<_>>()
        .join(",");
    text.push_str("\r\n");
    for row in rows {
        text.push_str(
            &order
                .iter()
                .map(|&i| row.0[i].as_str())
                .collect::<Vec<_>>()
                .join(","),
        );
        text.push_str("\r\n");
    }
    text
}

pub fn cp932(text: &str) -> Vec<u8> {
    let (bytes, _, unmappable) = encoding_rs::SHIFT_JIS.encode(text);
    assert!(!unmappable);
    bytes.into_owned()
}

/// 7年度の原本をcp932で `dir` に書く。`rows(year)` が各年度の行。
pub fn write_source_dir(dir: &Path, rows: impl Fn(u16) -> Vec<SourceRow>) {
    std::fs::create_dir_all(dir).unwrap();
    for year in 2019..=2025u16 {
        // 年度ごとに列の並びを変える（位置を固定しないことの確認）
        let mut order: Vec<usize> = (0..HEADER.len()).collect();
        order.rotate_left(usize::from(year - 2019));
        let text = csv_text(&HEADER, &rows(year), Some(&order));
        std::fs::write(dir.join(format!("honhyo_{year}.csv")), cp932(&text)).unwrap();
    }
}

/// 照合済みとして扱う照合器（XLSXの照合は別の試験で行う）。
pub struct AcceptAll;

impl CodebookVerifier for AcceptAll {
    fn verify(
        &self,
        _accident_dir: &Path,
    ) -> Result<(Vec<CodebookFile>, Vec<CodebookFile>), CodebookError> {
        Ok((Vec::new(), Vec::new()))
    }
}

/// 2019〜2026年の一部の祝日（人工）。
pub const HOLIDAYS: &str = "\
国民の祝日・休日月日,国民の祝日・休日名称
2019/1/1,元日
2025/1/1,元日
2025/11/23,勤労感謝の日
2025/11/24,休日
";
