//! ロード済みの事故データ（列配列と空間索引）。
//!
//! 事故IDはgzip CSVのデータ行の0始まり番号（behavior.md 3章）。1要求の中では
//! 同じスナップショットを使う。

use std::collections::HashMap;

/// 索引のセル。`(floor(lat*1000), floor(lon*1000))`。
pub type Cell = (i64, i64);

/// 0.001度グリッドのセル。
pub fn cell_of(lat: f64, lon: f64) -> Cell {
    ((lat * 1000.0).floor() as i64, (lon * 1000.0).floor() as i64)
}

/// gzip CSVの先頭10列（`day_type` まで）の列配列。添字が事故ID。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccidentColumns {
    pub lat: Vec<f64>,
    pub lon: Vec<f64>,
    /// `Mode::mask` のOR。
    pub modes: Vec<u8>,
    /// `Period::code`。
    pub period: Vec<u8>,
    /// `Weather::code`。
    pub weather: Vec<u8>,
    /// `DayType::code`。
    pub day_type: Vec<u8>,
    pub deaths: Vec<u16>,
    pub injuries: Vec<u16>,
    pub source_year: Vec<u16>,
    /// 発生日YYYYMMDD。
    pub date: Vec<u32>,
}

impl AccidentColumns {
    pub fn len(&self) -> usize {
        self.lat.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lat.is_empty()
    }
}

/// 列配列と、入力順に登録した0.001度グリッド。
#[derive(Debug, Clone)]
pub struct AccidentSnapshot {
    columns: AccidentColumns,
    grid: HashMap<Cell, Vec<u32>>,
}

impl AccidentSnapshot {
    /// 索引を作る。各セルの事故IDは昇順（入力順）。
    pub fn new(columns: AccidentColumns) -> Self {
        let mut grid: HashMap<Cell, Vec<u32>> = HashMap::new();
        for (id, (&lat, &lon)) in columns.lat.iter().zip(&columns.lon).enumerate() {
            grid.entry(cell_of(lat, lon)).or_default().push(id as u32);
        }
        Self { columns, grid }
    }

    pub fn columns(&self) -> &AccidentColumns {
        &self.columns
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// セル内の事故ID（昇順）。
    pub fn cell(&self, cell: Cell) -> &[u32] {
        self.grid.get(&cell).map_or(&[], Vec::as_slice)
    }

    /// 事故のあるセルの数。`/api/health` の `grid_cells`。
    pub fn grid_cells(&self) -> u64 {
        self.grid.len() as u64
    }
}
