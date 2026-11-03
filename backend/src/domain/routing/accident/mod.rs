//! 事故の属性と道路への割り当て（形式書12章・13章）、起動時の索引。

use std::collections::HashMap;

use serde_json::Value;

use crate::domain::routing::graph::EdgeId;
use crate::domain::routing::topology::ClusterId;
use crate::domain::snapshot::{AccidentSnapshot, Cell, cell_of};

/// 欠損・未知（`u16` の列）。
pub const MISSING_U16: u16 = 0xFFFF;
/// 欠損・未知（`u8` の列）。
pub const MISSING_U8: u8 = 0xFF;

/// `ATTR` 節のレコード（形式書12章、8バイト）。事故ID（行番号）の順に並ぶ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccidentAttr {
    /// 路線コードの上4桁（0〜9999）。欠損・未知は `MISSING_U16`。
    pub route_upper: u16,
    /// 路線コードの下1桁（0：現道・包括路線、1〜9：バイパス）。欠損・未知は `MISSING_U8`。
    pub route_lower: u8,
    /// `RouteClass::code`。上4桁から決める。欠損・未知は `MISSING_U8`。
    pub route_class: u8,
    /// 道路形状のコード（2024年のコード表の値：0、1、7、11〜14、21〜23、31、37）。
    pub road_shape: u8,
    /// 車道幅員のコード（0、1〜6、11、14、15、17、18、19）。
    pub carriageway_width: u8,
    /// 歩車道区分のコード（1〜4）。
    pub sidewalk_division: u8,
    /// 地形のコード（1：市街地－人口集中、2：市街地－その他、3：非市街地）。
    pub terrain: u8,
}

/// 道路形状のコード（2024年のコード表の値）。
pub const ROAD_SHAPE_CODES: [u8; 12] = [0, 1, 7, 11, 12, 13, 14, 21, 22, 23, 31, 37];
/// 車道幅員のコード（2024年のコード表の値）。
pub const CARRIAGEWAY_WIDTH_CODES: [u8; 13] = [0, 1, 2, 3, 4, 5, 6, 11, 14, 15, 17, 18, 19];
/// 歩車道区分のコード。
pub const SIDEWALK_DIVISION_CODES: [u8; 4] = [1, 2, 3, 4];
/// 地形のコード（1：市街地－人口集中、2：市街地－その他、3：非市街地）。
pub const TERRAIN_CODES: [u8; 3] = [1, 2, 3];

impl AccidentAttr {
    /// `ATTR` 節の `elem_size`。
    pub const SIZE: usize = 8;

    /// すべての属性が欠損・未知のレコード。
    pub const MISSING: AccidentAttr = AccidentAttr {
        route_upper: MISSING_U16,
        route_lower: MISSING_U8,
        route_class: MISSING_U8,
        road_shape: MISSING_U8,
        carriageway_width: MISSING_U8,
        sidewalk_division: MISSING_U8,
        terrain: MISSING_U8,
    };

    /// `ATTR` 節の1要素（形式書12章、リトルエンディアン）。
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let upper = self.route_upper.to_le_bytes();
        [
            upper[0],
            upper[1],
            self.route_lower,
            self.route_class,
            self.road_shape,
            self.carriageway_width,
            self.sidewalk_division,
            self.terrain,
        ]
    }

    pub fn from_bytes(bytes: [u8; Self::SIZE]) -> Self {
        Self {
            route_upper: u16::from_le_bytes([bytes[0], bytes[1]]),
            route_lower: bytes[2],
            route_class: bytes[3],
            road_shape: bytes[4],
            carriageway_width: bytes[5],
            sidewalk_division: bytes[6],
            terrain: bytes[7],
        }
    }

    /// 路線の区分。欠損・未知は `None`。
    pub fn route_class_kind(&self) -> Option<RouteClass> {
        RouteClass::from_code(self.route_class)
    }

    /// 「高速・自専道のコードの事故」（`route_class` が5〜7）か。欠損・未知は偽。
    pub fn is_motorway_code(&self) -> bool {
        self.route_class_kind()
            .is_some_and(RouteClass::is_motorway_code)
    }

    /// 形式書12章の値の範囲と、列どうしの整合を確かめる。読込側（`decode_accident_attrs`）が使う。
    ///
    /// - 路線コードの上4桁は0〜9999、下1桁は0〜9。上4桁と下1桁は、両方あるか両方とも欠損・未知。
    /// - `route_class` は、上4桁から `RouteClass::from_route_upper` で決めた値と一致する。
    ///   上4桁が表にない（または欠損・未知の）ときは `MISSING_U8`。
    /// - 道路形状・車道幅員・歩車道区分・地形は、2024年のコード表の値か `MISSING_U8`。
    pub fn validate(&self) -> Result<(), String> {
        let upper_missing = self.route_upper == MISSING_U16;
        if !upper_missing && self.route_upper > 9999 {
            return Err(format!("路線コードの上4桁{}が範囲外", self.route_upper));
        }
        if upper_missing != (self.route_lower == MISSING_U8) {
            return Err("路線コードの上4桁と下1桁の欠損が一致しない".to_owned());
        }
        if !upper_missing && self.route_lower > 9 {
            return Err(format!("路線コードの下1桁{}が範囲外", self.route_lower));
        }
        let expected_class = if upper_missing {
            MISSING_U8
        } else {
            RouteClass::from_route_upper(self.route_upper).map_or(MISSING_U8, RouteClass::code)
        };
        if self.route_class != expected_class {
            return Err(format!(
                "route_class{}が上4桁{}から決まる値{}と一致しない",
                self.route_class, self.route_upper, expected_class
            ));
        }
        for (name, value, codes) in [
            ("道路形状", self.road_shape, &ROAD_SHAPE_CODES[..]),
            (
                "車道幅員",
                self.carriageway_width,
                &CARRIAGEWAY_WIDTH_CODES[..],
            ),
            (
                "歩車道区分",
                self.sidewalk_division,
                &SIDEWALK_DIVISION_CODES[..],
            ),
            ("地形", self.terrain, &TERRAIN_CODES[..]),
        ] {
            if value != MISSING_U8 && !codes.contains(&value) {
                return Err(format!("{name}のコード{value}が表にない"));
            }
        }
        Ok(())
    }
}

/// `accident_attrs.bin` の中身。
#[derive(Debug, Clone, PartialEq)]
pub struct AccidentAttrTable {
    /// `PARAMS`（JSON）。
    pub params: Value,
    /// `ATTR`：長さA。
    pub attrs: Vec<AccidentAttr>,
}

/// 路線の区分（形式書12章の `route_class` の表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RouteClass {
    /// 0001〜0999：一般国道（上4桁が国道番号）。
    NationalRoute = 0,
    /// 1000〜1499：主要地方道－都道府県道。
    MajorPrefecturalRoad = 1,
    /// 1500〜1999：主要地方道－市道。
    MajorMunicipalRoad = 2,
    /// 2000〜2999：一般都道府県道。
    PrefecturalRoad = 3,
    /// 3000〜3999：一般市町村道。
    MunicipalRoad = 4,
    /// 4000〜4999：高速自動車国道。
    Expressway = 5,
    /// 5000〜5499：自動車専用道－指定。
    DesignatedMotorway = 6,
    /// 5500〜5999：自動車専用道－その他。
    OtherMotorway = 7,
    /// 6000〜6999：道路運送法上の道路。
    RoadTransportActRoad = 8,
    /// 7000〜7999：農（免）道。
    FarmRoad = 9,
    /// 8000〜8499：林道。
    ForestRoad = 10,
    /// 8500〜8999：港湾道。
    PortRoad = 11,
    /// 9000〜9499：私道。
    PrivateRoad = 12,
    /// 9500：その他。
    Other = 13,
    /// 9900：一般の交通の用に供するその他の道路。
    GeneralTrafficOther = 14,
}

impl RouteClass {
    pub const ALL: [RouteClass; 15] = [
        RouteClass::NationalRoute,
        RouteClass::MajorPrefecturalRoad,
        RouteClass::MajorMunicipalRoad,
        RouteClass::PrefecturalRoad,
        RouteClass::MunicipalRoad,
        RouteClass::Expressway,
        RouteClass::DesignatedMotorway,
        RouteClass::OtherMotorway,
        RouteClass::RoadTransportActRoad,
        RouteClass::FarmRoad,
        RouteClass::ForestRoad,
        RouteClass::PortRoad,
        RouteClass::PrivateRoad,
        RouteClass::Other,
        RouteClass::GeneralTrafficOther,
    ];

    pub const fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.get(usize::from(code)).copied()
    }

    /// 区分に入る路線コードの上4桁の範囲 `(最小, 最大)`（形式書12章の表。両端を含む）。
    pub const fn upper_range(self) -> (u16, u16) {
        match self {
            RouteClass::NationalRoute => (1, 999),
            RouteClass::MajorPrefecturalRoad => (1000, 1499),
            RouteClass::MajorMunicipalRoad => (1500, 1999),
            RouteClass::PrefecturalRoad => (2000, 2999),
            RouteClass::MunicipalRoad => (3000, 3999),
            RouteClass::Expressway => (4000, 4999),
            RouteClass::DesignatedMotorway => (5000, 5499),
            RouteClass::OtherMotorway => (5500, 5999),
            RouteClass::RoadTransportActRoad => (6000, 6999),
            RouteClass::FarmRoad => (7000, 7999),
            RouteClass::ForestRoad => (8000, 8499),
            RouteClass::PortRoad => (8500, 8999),
            RouteClass::PrivateRoad => (9000, 9499),
            RouteClass::Other => (9500, 9500),
            RouteClass::GeneralTrafficOther => (9900, 9900),
        }
    }

    /// 路線コードの上4桁から決める（形式書12章の表）。表にない値（0、9501〜9899、9901以上）は `None`。
    pub fn from_route_upper(upper: u16) -> Option<Self> {
        match upper {
            1..=999 => Some(RouteClass::NationalRoute),
            1000..=1499 => Some(RouteClass::MajorPrefecturalRoad),
            1500..=1999 => Some(RouteClass::MajorMunicipalRoad),
            2000..=2999 => Some(RouteClass::PrefecturalRoad),
            3000..=3999 => Some(RouteClass::MunicipalRoad),
            4000..=4999 => Some(RouteClass::Expressway),
            5000..=5499 => Some(RouteClass::DesignatedMotorway),
            5500..=5999 => Some(RouteClass::OtherMotorway),
            6000..=6999 => Some(RouteClass::RoadTransportActRoad),
            7000..=7999 => Some(RouteClass::FarmRoad),
            8000..=8499 => Some(RouteClass::ForestRoad),
            8500..=8999 => Some(RouteClass::PortRoad),
            9000..=9499 => Some(RouteClass::PrivateRoad),
            9500 => Some(RouteClass::Other),
            9900 => Some(RouteClass::GeneralTrafficOther),
            _ => None,
        }
    }

    /// 「高速・自専道のコードの事故」（`route_class` が5〜7、形式書12章）。
    pub const fn is_motorway_code(self) -> bool {
        matches!(
            self,
            RouteClass::Expressway | RouteClass::DesignatedMotorway | RouteClass::OtherMotorway
        )
    }
}

/// `TargetKind`（`TARGET` 節の `kind`、形式書13章）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum TargetKind {
    /// 辺（`target` は辺番号、`pos_dm` は始点からの位置）。
    Edge = 0,
    /// 交差点クラスタ（`target` はクラスタ番号、`pos_dm` は `u32::MAX`）。
    Cluster = 1,
}

impl TargetKind {
    pub const fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(TargetKind::Edge),
            1 => Some(TargetKind::Cluster),
            _ => None,
        }
    }
}

/// `AssignTarget::flags` のビット0：双子の辺への写し（形式書13章）。
pub const TARGET_MIRROR: u8 = 0x01;
/// 辺の候補を探す距離の上限（m、形式書13章・計画書4.3節）。
pub const EDGE_RADIUS_M: f64 = 35.0;
/// 交差点クラスタの候補を探す距離の上限（m）。
pub const CLUSTER_RADIUS_M: f64 = 30.0;
/// 1件の事故に割り当てる重みの下限（`MIRROR` でない候補、形式書13章）。
pub const MIN_WEIGHT: f32 = 0.1;
/// 1件の事故に割り当てる候補の上限（`MIRROR` を除く）。
pub const MAX_TARGETS: usize = 3;
/// 重みの合計の許容誤差（形式書13章：浮動小数の誤差1e-6まで）。
pub const WEIGHT_SUM_TOLERANCE: f64 = 1e-6;
/// 距離の分散の初期値 σ0（m、計画書4.3節。M5で調整する）。
pub const SIGMA0_M: f64 = 3.5;

/// `TARGET` 節のレコード（形式書13章、16バイト）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AssignTarget {
    /// 辺番号またはクラスタ番号（`kind` による）。
    pub target: u32,
    /// 辺の始点からの位置（デシメートル）。クラスタは `u32::MAX`。
    pub pos_dm: u32,
    /// 重み。`MIRROR` でない候補は各 `weight ≥ 0.1`、合計は1以下（誤差1e-6まで）。
    pub weight: f32,
    /// `TargetKind::code`。
    pub kind: u8,
    /// `TARGET_MIRROR` など。
    pub flags: u8,
    /// 予約（0）。
    pub reserved: u16,
}

impl AssignTarget {
    /// `TARGET` 節の `elem_size`。
    pub const SIZE: usize = 16;

    pub fn is_mirror(&self) -> bool {
        self.flags & TARGET_MIRROR != 0
    }
}

/// 帰属不明の理由（`UNASSGN` 節の `reason`、形式書13章）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum UnassignedReason {
    /// 1：候補なし（35m以内に車道の辺がなく、交差点の事故で30m以内にクラスタもない）。
    NoCandidate = 1,
    /// 2：高速・自専道のコードの不一致（候補はあるが、条件で全部外れた）。
    MotorwayCodeMismatch = 2,
    /// 3：重みの不足（正規化後、0.1以上の候補がない）。
    InsufficientWeight = 3,
    /// 4：一般交通の場所（道路形状0）。
    GeneralTrafficPlace = 4,
    /// 255：その他。
    Other = 255,
}

impl UnassignedReason {
    pub const fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(UnassignedReason::NoCandidate),
            2 => Some(UnassignedReason::MotorwayCodeMismatch),
            3 => Some(UnassignedReason::InsufficientWeight),
            4 => Some(UnassignedReason::GeneralTrafficPlace),
            255 => Some(UnassignedReason::Other),
            _ => None,
        }
    }
}

/// `UNASSGN` 節のレコード（形式書13章、8バイト）。事故IDの昇順。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnassignedRecord {
    pub accident: u32,
    pub reason: UnassignedReason,
}

impl UnassignedRecord {
    /// `UNASSGN` 節の `elem_size`（`{accident: u32, reason: u8, pad: [u8; 3]}`）。
    pub const SIZE: usize = 8;
}

/// `assignment.bin` の中身（事故ごとの形）。起動時に `EdgeAccidentIndex` へ組み替えて捨てる。
#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    /// `PARAMS`（JSON）。
    pub params: Value,
    /// `ACCOFS`：事故iの割り当ては `targets[offsets[i]..offsets[i+1]]`。長さA+1。
    pub offsets: Vec<u32>,
    /// `TARGET`：候補は `(kind, target, pos_dm)` の昇順。
    pub targets: Vec<AssignTarget>,
    /// `UNASSGN`：割り当てのない事故。割り当てのある事故は入らない。
    pub unassigned: Vec<UnassignedRecord>,
}

impl Assignment {
    /// 事故iの割り当て。
    pub fn targets_of(&self, accident: usize) -> &[AssignTarget] {
        &self.targets[self.offsets[accident] as usize..self.offsets[accident + 1] as usize]
    }

    /// 事故の数A。
    pub fn accident_count(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    /// 形式書13章の読込時の検査のうち、グラフとの照合が要らないもの。
    ///
    /// - `ACCOFS` は0から始まり非減少で、末尾が `TARGET` の件数。
    /// - 各 `TARGET`：`kind` が0か1、`flags` は `MIRROR` だけ、`reserved` は0、`weight` は有限で
    ///   `MIN_WEIGHT` 以上1以下。クラスタは `pos_dm == u32::MAX` で `MIRROR` でない。
    /// - 1件の事故の候補は `(kind, target, pos_dm)` の狭義昇順。`MIRROR` でない候補は1個以上
    ///   `MAX_TARGETS` 個以下で、重みの合計は1以下（誤差 `WEIGHT_SUM_TOLERANCE`）。
    /// - `UNASSGN` は事故IDの狭義昇順で、事故の数未満。割り当てのある事故は入らず、割り当てのない事故は
    ///   必ず1回入る（排他で、合わせて全事故）。
    pub fn validate_structure(&self) -> Result<(), String> {
        let accidents = self.accident_count();
        if self.offsets.is_empty() {
            return Err("ACCOFSが空です".to_owned());
        }
        if self.offsets[0] != 0 {
            return Err(format!("ACCOFS[0]が{}です（0のはず）", self.offsets[0]));
        }
        if let Some(i) = self.offsets.windows(2).position(|w| w[0] > w[1]) {
            return Err(format!("ACCOFSが非減少ではありません（{i}番目）"));
        }
        if self.offsets[accidents] as usize != self.targets.len() {
            return Err(format!(
                "ACCOFSの末尾{}がTARGETの件数{}と一致しません",
                self.offsets[accidents],
                self.targets.len()
            ));
        }
        let mut next_unassigned = self.unassigned.iter().peekable();
        let mut previous_unassigned: Option<u32> = None;
        for record in &self.unassigned {
            if record.accident as usize >= accidents {
                return Err(format!(
                    "UNASSGNの事故{}が事故の数{accidents}以上です",
                    record.accident
                ));
            }
            if previous_unassigned.is_some_and(|p| p >= record.accident) {
                return Err(format!(
                    "UNASSGNが事故IDの狭義昇順ではありません（事故{}）",
                    record.accident
                ));
            }
            previous_unassigned = Some(record.accident);
        }
        for accident in 0..accidents {
            let targets = self.targets_of(accident);
            let listed = next_unassigned
                .next_if(|r| r.accident as usize == accident)
                .is_some();
            if targets.is_empty() {
                if !listed {
                    return Err(format!(
                        "事故{accident}は割り当てがなく、UNASSGNにもありません"
                    ));
                }
                continue;
            }
            if listed {
                return Err(format!(
                    "事故{accident}は割り当てがあるのにUNASSGNにもあります"
                ));
            }
            Self::validate_accident_targets(accident, targets)?;
        }
        Ok(())
    }

    fn validate_accident_targets(accident: usize, targets: &[AssignTarget]) -> Result<(), String> {
        let mut direct = 0_usize;
        let mut sum = 0.0_f64;
        let mut previous: Option<(u8, u32, u32)> = None;
        for t in targets {
            let Some(kind) = TargetKind::from_code(t.kind) else {
                return Err(format!("事故{accident}のkind {}が不正です", t.kind));
            };
            if t.flags & !TARGET_MIRROR != 0 || t.reserved != 0 {
                return Err(format!("事故{accident}のflagsまたはreservedが不正です"));
            }
            if !t.weight.is_finite() || t.weight < MIN_WEIGHT || t.weight > 1.0 {
                return Err(format!(
                    "事故{accident}の重み{}が{MIN_WEIGHT}以上1以下ではありません",
                    t.weight
                ));
            }
            if kind == TargetKind::Cluster && (t.pos_dm != u32::MAX || t.is_mirror()) {
                return Err(format!(
                    "事故{accident}のクラスタの割り当てでpos_dmがu32::MAXでない、またはMIRRORが立っています"
                ));
            }
            let key = (t.kind, t.target, t.pos_dm);
            if previous.is_some_and(|p| p >= key) {
                return Err(format!(
                    "事故{accident}の候補が(kind, target, pos_dm)の狭義昇順ではありません"
                ));
            }
            previous = Some(key);
            if !t.is_mirror() {
                direct += 1;
                sum += f64::from(t.weight);
            }
        }
        if direct == 0 || direct > MAX_TARGETS {
            return Err(format!(
                "事故{accident}のMIRRORでない候補が{direct}個です（1〜{MAX_TARGETS}個のはず）"
            ));
        }
        if sum > 1.0 + WEIGHT_SUM_TOLERANCE {
            return Err(format!("事故{accident}の重みの合計{sum}が1を超えています"));
        }
        Ok(())
    }

    /// `validate_structure` に加えて、辺番号・クラスタ番号の範囲を確かめる。
    pub fn validate(&self, edge_count: usize, cluster_count: usize) -> Result<(), String> {
        self.validate_structure()?;
        for (i, t) in self.targets.iter().enumerate() {
            let limit = match TargetKind::from_code(t.kind) {
                Some(TargetKind::Edge) => edge_count,
                _ => cluster_count,
            };
            if t.target as usize >= limit {
                return Err(format!(
                    "TARGET[{i}]の番号{}が範囲外です（上限{limit}）",
                    t.target
                ));
            }
        }
        Ok(())
    }
}

/// 辺への割り当て1件（辺ごとの索引の要素）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeAccident {
    pub accident: u32,
    /// 辺の始点からの位置（デシメートル）。
    pub pos_dm: u32,
    pub weight: f32,
    /// 双子の辺への写し（`TARGET_MIRROR`）か。
    pub mirror: bool,
}

/// クラスタへの割り当て1件（クラスタごとの索引の要素）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClusterAccident {
    pub accident: u32,
    pub weight: f32,
}

/// 辺ごと・クラスタごとの事故の索引（形式書13章の「起動時の持ち方」）。
/// 事故ごとの形は捨てる。要素は辺・クラスタ番号の昇順に並べた CSR 形式。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EdgeAccidentIndex {
    /// 長さE+1。辺eの要素は `edge_entries[edge_ofs[e]..edge_ofs[e+1]]`（位置 `pos_dm` の昇順）。
    pub edge_ofs: Vec<u32>,
    pub edge_entries: Vec<EdgeAccident>,
    /// 長さC+1。クラスタcの要素は `cluster_entries[cluster_ofs[c]..cluster_ofs[c+1]]`。
    pub cluster_ofs: Vec<u32>,
    pub cluster_entries: Vec<ClusterAccident>,
}

impl EdgeAccidentIndex {
    /// `Assignment` から辺ごと・クラスタごとの索引を作る。事故ごとの形は引き継がない。
    ///
    /// 辺ごとの要素は位置 `pos_dm` の昇順（同じ位置は事故IDの昇順）、クラスタごとの要素は事故IDの昇順。
    /// 同じ入力から同じ索引になる。
    ///
    /// # Panics
    /// 割り当ての辺番号が `edge_count` 以上、クラスタ番号が `cluster_count` 以上、または `kind` が不正なとき。
    /// 先に [`Assignment::validate`] で確かめる。
    pub fn build(
        assignment: &Assignment,
        edge_count: usize,
        cluster_count: usize,
    ) -> EdgeAccidentIndex {
        let mut edge_ofs = vec![0_u32; edge_count + 1];
        let mut cluster_ofs = vec![0_u32; cluster_count + 1];
        for t in &assignment.targets {
            match TargetKind::from_code(t.kind).expect("kindは0か1") {
                TargetKind::Edge => edge_ofs[t.target as usize + 1] += 1,
                TargetKind::Cluster => cluster_ofs[t.target as usize + 1] += 1,
            }
        }
        for i in 0..edge_count {
            edge_ofs[i + 1] += edge_ofs[i];
        }
        for i in 0..cluster_count {
            cluster_ofs[i + 1] += cluster_ofs[i];
        }
        let placeholder_edge = EdgeAccident {
            accident: 0,
            pos_dm: 0,
            weight: 0.0,
            mirror: false,
        };
        let placeholder_cluster = ClusterAccident {
            accident: 0,
            weight: 0.0,
        };
        let mut edge_entries = vec![placeholder_edge; edge_ofs[edge_count] as usize];
        let mut cluster_entries = vec![placeholder_cluster; cluster_ofs[cluster_count] as usize];
        let mut edge_cursor = edge_ofs.clone();
        let mut cluster_cursor = cluster_ofs.clone();
        for accident in 0..assignment.accident_count() {
            for t in assignment.targets_of(accident) {
                match TargetKind::from_code(t.kind).expect("kindは0か1") {
                    TargetKind::Edge => {
                        let slot = &mut edge_cursor[t.target as usize];
                        edge_entries[*slot as usize] = EdgeAccident {
                            accident: accident as u32,
                            pos_dm: t.pos_dm,
                            weight: t.weight,
                            mirror: t.is_mirror(),
                        };
                        *slot += 1;
                    }
                    TargetKind::Cluster => {
                        let slot = &mut cluster_cursor[t.target as usize];
                        cluster_entries[*slot as usize] = ClusterAccident {
                            accident: accident as u32,
                            weight: t.weight,
                        };
                        *slot += 1;
                    }
                }
            }
        }
        // 事故IDの昇順に詰めたので、辺ごとの要素は位置の昇順に並べ直す（同じ位置は事故IDの昇順のまま）
        for e in 0..edge_count {
            let (start, end) = (edge_ofs[e] as usize, edge_ofs[e + 1] as usize);
            if end - start > 1 {
                edge_entries[start..end].sort_by_key(|entry| (entry.pos_dm, entry.accident));
            }
        }
        EdgeAccidentIndex {
            edge_ofs,
            edge_entries,
            cluster_ofs,
            cluster_entries,
        }
    }

    /// 辺への割り当て（位置の昇順）。
    pub fn edge_entries(&self, e: EdgeId) -> &[EdgeAccident] {
        let (start, end) = (
            self.edge_ofs[e as usize] as usize,
            self.edge_ofs[e as usize + 1] as usize,
        );
        &self.edge_entries[start..end]
    }

    /// クラスタへの割り当て。
    pub fn cluster_entries(&self, c: ClusterId) -> &[ClusterAccident] {
        let (start, end) = (
            self.cluster_ofs[c as usize] as usize,
            self.cluster_ofs[c as usize + 1] as usize,
        );
        &self.cluster_entries[start..end]
    }
}

/// 帰属不明の事故の格子索引（形式書13章。72m判定用）。
#[derive(Debug, Clone, Default)]
pub struct UnassignedGrid {
    /// 既存の `AccidentSnapshot` と同じ0.001度のセル。各セルの事故IDは昇順。
    pub cells: HashMap<Cell, Vec<u32>>,
}

impl UnassignedGrid {
    /// 帰属不明の事故だけを、スナップショットの座標でセルに入れる。各セルの事故IDは昇順
    /// （`UNASSGN` が事故IDの昇順なので、入れた順のまま昇順になる）。
    ///
    /// # Panics
    /// `UNASSGN` の事故IDがスナップショットの事故の数以上のとき。
    pub fn build(assignment: &Assignment, snapshot: &AccidentSnapshot) -> UnassignedGrid {
        let columns = snapshot.columns();
        let mut cells: HashMap<Cell, Vec<u32>> = HashMap::new();
        for record in &assignment.unassigned {
            let id = record.accident as usize;
            cells
                .entry(cell_of(columns.lat[id], columns.lon[id]))
                .or_default()
                .push(record.accident);
        }
        UnassignedGrid { cells }
    }

    /// セル内の事故ID（昇順）。
    pub fn cell(&self, cell: Cell) -> &[u32] {
        self.cells.get(&cell).map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_class_boundaries() {
        let cases = [
            (0, None),
            (1, Some(0)),
            (999, Some(0)),
            (1000, Some(1)),
            (1499, Some(1)),
            (1500, Some(2)),
            (1999, Some(2)),
            (2000, Some(3)),
            (3999, Some(4)),
            (4000, Some(5)),
            (5499, Some(6)),
            (5500, Some(7)),
            (5999, Some(7)),
            (6000, Some(8)),
            (7000, Some(9)),
            (8000, Some(10)),
            (8500, Some(11)),
            (9000, Some(12)),
            (9499, Some(12)),
            (9500, Some(13)),
            (9501, None),
            (9899, None),
            (9900, Some(14)),
            (9901, None),
            (9999, None),
            (MISSING_U16, None),
        ];
        for (upper, expected) in cases {
            assert_eq!(
                RouteClass::from_route_upper(upper).map(RouteClass::code),
                expected,
                "上4桁 {upper}"
            );
        }
        for class in RouteClass::ALL {
            assert_eq!(RouteClass::from_code(class.code()), Some(class));
        }
        assert_eq!(RouteClass::from_code(15), None);
        assert_eq!(
            RouteClass::ALL
                .map(RouteClass::is_motorway_code)
                .iter()
                .filter(|&&b| b)
                .count(),
            3
        );
        assert!(
            RouteClass::Expressway.is_motorway_code()
                && !RouteClass::MunicipalRoad.is_motorway_code()
        );
    }

    #[test]
    fn route_class_ranges_agree_with_from_route_upper() {
        let mut next = 1u16;
        for class in RouteClass::ALL {
            let (lo, hi) = class.upper_range();
            assert!(lo <= hi);
            assert_eq!(RouteClass::from_route_upper(lo), Some(class));
            assert_eq!(RouteClass::from_route_upper(hi), Some(class));
            // 区分は昇順に並び、重ならない
            assert!(lo >= next, "{class:?}");
            next = hi + 1;
        }
        // 範囲に入る値は、その区分に決まる
        for upper in 0..=9999u16 {
            let by_range = RouteClass::ALL.into_iter().find(|c| {
                let (lo, hi) = c.upper_range();
                (lo..=hi).contains(&upper)
            });
            assert_eq!(RouteClass::from_route_upper(upper), by_range, "{upper}");
        }
    }

    #[test]
    fn attr_bytes_round_trip_and_layout() {
        let attr = AccidentAttr {
            route_upper: 0x1234,
            route_lower: 3,
            route_class: 1,
            road_shape: 14,
            carriageway_width: 15,
            sidewalk_division: 2,
            terrain: 1,
        };
        // 形式書12章：上4桁はリトルエンディアンの u16、続けて下1桁・route_class・道路形状・
        // 車道幅員・歩車道区分・地形
        assert_eq!(attr.to_bytes(), [0x34, 0x12, 3, 1, 14, 15, 2, 1]);
        assert_eq!(AccidentAttr::from_bytes(attr.to_bytes()), attr);
        assert_eq!(AccidentAttr::MISSING.to_bytes(), [0xFF; 8]);
        assert_eq!(AccidentAttr::from_bytes([0xFF; 8]), AccidentAttr::MISSING);
        assert_eq!(AccidentAttr::SIZE, 8);
    }

    #[test]
    fn attr_validation() {
        let ok = AccidentAttr {
            route_upper: 5000,
            route_lower: 0,
            route_class: RouteClass::DesignatedMotorway.code(),
            road_shape: 14,
            carriageway_width: 6,
            sidewalk_division: 4,
            terrain: 3,
        };
        assert_eq!(ok.validate(), Ok(()));
        assert!(ok.is_motorway_code());
        assert_eq!(ok.route_class_kind(), Some(RouteClass::DesignatedMotorway));
        assert_eq!(AccidentAttr::MISSING.validate(), Ok(()));
        assert!(!AccidentAttr::MISSING.is_motorway_code());
        // 上4桁が表にない（9501〜9899）ときは route_class が欠損・未知
        let unknown_class = AccidentAttr {
            route_upper: 9700,
            route_class: MISSING_U8,
            ..ok
        };
        assert_eq!(unknown_class.validate(), Ok(()));
        let bad = [
            AccidentAttr {
                route_upper: 10000,
                ..ok
            },
            AccidentAttr {
                route_lower: 10,
                ..ok
            },
            AccidentAttr {
                route_lower: MISSING_U8,
                ..ok
            },
            AccidentAttr {
                route_class: RouteClass::Expressway.code(),
                ..ok
            },
            AccidentAttr {
                route_class: MISSING_U8,
                ..ok
            },
            AccidentAttr {
                road_shape: 2,
                ..ok
            },
            AccidentAttr {
                carriageway_width: 12,
                ..ok
            },
            AccidentAttr {
                sidewalk_division: 0,
                ..ok
            },
            AccidentAttr { terrain: 4, ..ok },
        ];
        for attr in bad {
            assert!(attr.validate().is_err(), "{attr:?}");
        }
    }

    #[test]
    fn reason_and_kind_codes() {
        for reason in [
            UnassignedReason::NoCandidate,
            UnassignedReason::MotorwayCodeMismatch,
            UnassignedReason::InsufficientWeight,
            UnassignedReason::GeneralTrafficPlace,
            UnassignedReason::Other,
        ] {
            assert_eq!(UnassignedReason::from_code(reason.code()), Some(reason));
        }
        assert_eq!(UnassignedReason::from_code(0), None);
        assert_eq!(UnassignedReason::from_code(5), None);
        assert_eq!(TargetKind::from_code(1), Some(TargetKind::Cluster));
        assert_eq!(TargetKind::from_code(2), None);
    }

    // --- Assignment の検査と、起動時の索引 ---

    fn edge_target(target: u32, pos_dm: u32, weight: f32, mirror: bool) -> AssignTarget {
        AssignTarget {
            target,
            pos_dm,
            weight,
            kind: TargetKind::Edge.code(),
            flags: if mirror { TARGET_MIRROR } else { 0 },
            reserved: 0,
        }
    }

    fn cluster_target(target: u32, weight: f32) -> AssignTarget {
        AssignTarget {
            target,
            pos_dm: u32::MAX,
            weight,
            kind: TargetKind::Cluster.code(),
            flags: 0,
            reserved: 0,
        }
    }

    /// 事故0：辺3（位置50）と辺5（位置20）に0.6・0.4。事故1：帰属不明（理由1）。
    /// 事故2：辺3（位置10）に1.0と、双子の辺4への写し。事故3：クラスタ2に0.9。
    fn sample_assignment() -> Assignment {
        Assignment {
            params: serde_json::json!({}),
            offsets: vec![0, 2, 2, 4, 5],
            targets: vec![
                edge_target(3, 50, 0.6, false),
                edge_target(5, 20, 0.4, false),
                edge_target(3, 10, 1.0, false),
                edge_target(4, 30, 1.0, true),
                cluster_target(2, 0.9),
            ],
            unassigned: vec![UnassignedRecord {
                accident: 1,
                reason: UnassignedReason::NoCandidate,
            }],
        }
    }

    #[test]
    fn a_valid_assignment_passes_the_structure_and_range_checks() {
        let a = sample_assignment();
        assert_eq!(a.validate_structure(), Ok(()));
        assert_eq!(a.validate(6, 3), Ok(()));
        assert!(a.validate(5, 3).is_err(), "辺5がE=5以上");
        assert!(a.validate(6, 2).is_err(), "クラスタ2がC=2以上");
        assert_eq!(a.accident_count(), 4);
        assert_eq!(a.targets_of(2).len(), 2);
    }

    #[test]
    fn broken_assignments_are_rejected() {
        type Mutation = Box<dyn Fn(&mut Assignment)>;
        let mutations: Vec<(&str, Mutation)> = vec![
            ("ACCOFSの先頭が0でない", Box::new(|a| a.offsets[0] = 1)),
            ("ACCOFSが減る", Box::new(|a| a.offsets[2] = 1)),
            (
                "ACCOFSの末尾がTARGETの件数と違う",
                Box::new(|a| a.offsets[4] = 4),
            ),
            ("重みが0.1未満", Box::new(|a| a.targets[1].weight = 0.09)),
            ("重みが1を超える", Box::new(|a| a.targets[2].weight = 1.5)),
            ("重みがNaN", Box::new(|a| a.targets[1].weight = f32::NAN)),
            (
                "合計が1を超える",
                Box::new(|a| {
                    a.targets[0].weight = 0.7;
                    a.targets[1].weight = 0.7;
                }),
            ),
            ("kindが不正", Box::new(|a| a.targets[0].kind = 2)),
            ("flagsが不正", Box::new(|a| a.targets[0].flags = 2)),
            ("reservedが0でない", Box::new(|a| a.targets[0].reserved = 1)),
            (
                "クラスタのpos_dmが不正",
                Box::new(|a| a.targets[4].pos_dm = 0),
            ),
            ("並びが昇順でない", Box::new(|a| a.targets.swap(0, 1))),
            ("同じ候補が重複", Box::new(|a| a.targets[1] = a.targets[0])),
            (
                "MIRRORだけ",
                Box::new(|a| a.targets[4] = edge_target(9, 1, 0.9, true)),
            ),
            (
                "割り当てがあるのにUNASSGNにもある",
                Box::new(|a| {
                    a.unassigned.push(UnassignedRecord {
                        accident: 2,
                        reason: UnassignedReason::Other,
                    })
                }),
            ),
            (
                "割り当てがないのにUNASSGNにない",
                Box::new(|a| a.unassigned.clear()),
            ),
            (
                "UNASSGNが昇順でない",
                Box::new(|a| {
                    a.unassigned.insert(
                        0,
                        UnassignedRecord {
                            accident: 3,
                            reason: UnassignedReason::Other,
                        },
                    )
                }),
            ),
            (
                "UNASSGNの事故が範囲外",
                Box::new(|a| {
                    a.unassigned.push(UnassignedRecord {
                        accident: 4,
                        reason: UnassignedReason::Other,
                    })
                }),
            ),
        ];
        for (what, mutate) in mutations {
            let mut a = sample_assignment();
            mutate(&mut a);
            assert!(a.validate_structure().is_err(), "{what}");
        }
        // 候補が4個
        let mut a = Assignment {
            params: serde_json::json!({}),
            offsets: vec![0, 4],
            targets: (0..4).map(|e| edge_target(e, 0, 0.2, false)).collect(),
            unassigned: Vec::new(),
        };
        assert!(a.validate_structure().is_err());
        a.targets.pop();
        a.offsets[1] = 3;
        assert_eq!(a.validate_structure(), Ok(()));
    }

    #[test]
    fn edge_accident_index_groups_by_edge_in_position_order() {
        let a = sample_assignment();
        let index = EdgeAccidentIndex::build(&a, 6, 3);
        assert_eq!(index.edge_ofs.len(), 7);
        assert_eq!(index.cluster_ofs.len(), 4);
        // 辺3：事故2（位置10）、事故0（位置50）。位置の昇順
        let e3 = index.edge_entries(3);
        assert_eq!(
            e3.iter()
                .map(|x| (x.accident, x.pos_dm))
                .collect::<Vec<_>>(),
            vec![(2, 10), (0, 50)]
        );
        assert!(e3.iter().all(|x| !x.mirror));
        // 辺4：写し
        let e4 = index.edge_entries(4);
        assert_eq!(e4.len(), 1);
        assert!(e4[0].mirror && e4[0].accident == 2 && e4[0].weight == 1.0);
        assert_eq!(index.edge_entries(5)[0].weight, 0.4);
        assert!(index.edge_entries(0).is_empty());
        let c2 = index.cluster_entries(2);
        assert_eq!((c2.len(), c2[0].accident, c2[0].weight), (1, 3, 0.9));
        assert!(index.cluster_entries(0).is_empty());
        // 割り当ての数が保たれる
        assert_eq!(
            index.edge_entries.len() + index.cluster_entries.len(),
            a.targets.len()
        );
        // 同じ入力から同じ索引
        assert_eq!(index, EdgeAccidentIndex::build(&a, 6, 3));
    }

    #[test]
    fn same_position_entries_are_ordered_by_accident() {
        let a = Assignment {
            params: serde_json::json!({}),
            offsets: vec![0, 1, 2, 3],
            targets: vec![
                edge_target(0, 70, 1.0, false),
                edge_target(0, 70, 1.0, false),
                edge_target(0, 5, 1.0, false),
            ],
            unassigned: Vec::new(),
        };
        let index = EdgeAccidentIndex::build(&a, 1, 0);
        assert_eq!(
            index
                .edge_entries(0)
                .iter()
                .map(|x| x.accident)
                .collect::<Vec<_>>(),
            vec![2, 0, 1]
        );
        assert_eq!(index.cluster_ofs, vec![0]);
    }

    #[test]
    #[should_panic]
    fn index_build_panics_on_an_out_of_range_edge() {
        EdgeAccidentIndex::build(&sample_assignment(), 5, 3);
    }

    #[test]
    fn unassigned_grid_puts_only_unassigned_accidents_in_their_cells() {
        use crate::domain::snapshot::{AccidentColumns, AccidentSnapshot};
        let columns = AccidentColumns {
            lat: vec![35.0005, 35.0007, 35.0015, 35.0006, 36.0],
            lon: vec![139.0005, 139.0006, 139.0005, 139.0005, 140.0],
            modes: vec![4; 5],
            period: vec![1; 5],
            weather: vec![2; 5],
            day_type: vec![0; 5],
            deaths: vec![0; 5],
            injuries: vec![1; 5],
            source_year: vec![2024; 5],
            date: vec![20240101; 5],
        };
        let snapshot = AccidentSnapshot::new(columns);
        let a = Assignment {
            params: serde_json::json!({}),
            offsets: vec![0, 0, 0, 1, 1, 1],
            targets: vec![edge_target(0, 0, 1.0, false)],
            unassigned: [0, 1, 3, 4]
                .into_iter()
                .map(|accident| UnassignedRecord {
                    accident,
                    reason: UnassignedReason::NoCandidate,
                })
                .collect(),
        };
        let grid = UnassignedGrid::build(&a, &snapshot);
        // 事故2は割り当てがあるので入らない。事故0・1・3は同じセル、事故4は別のセル
        assert_eq!(grid.cell((35000, 139000)), &[0, 1, 3]);
        assert_eq!(grid.cell((36000, 140000)), &[4]);
        assert!(grid.cell((35001, 139000)).is_empty());
        assert_eq!(grid.cells.len(), 2);
    }
}
