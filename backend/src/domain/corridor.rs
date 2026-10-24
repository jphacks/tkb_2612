//! 局所平面投影・75m分割・72m回廊（behavior.md 6章）。

use crate::contract::geo::METERS_PER_DEGREE;
use crate::contract::{CancelToken, Interrupt, LonLat};

use super::snapshot::{AccidentSnapshot, Cell, cell_of};

/// 小線分の最大長（m）。元の隣接頂点間を `ceil(length/75)` 本に分ける。
pub const SUBSEGMENT_LENGTH_M: f64 = 75.0;
/// 回廊の半径（m）。`contract::engine::RADIUS_M` と同じ。
pub const CORRIDOR_RADIUS_M: f64 = crate::contract::engine::RADIUS_M;
/// `d² <= 72²` の事故だけを採用する。
pub const CORRIDOR_RADIUS_SQ_M2: f64 = CORRIDOR_RADIUS_M * CORRIDOR_RADIUS_M;
/// 長いループの中で `cancel.check()` を呼ぶ間隔。
pub const CANCEL_CHECK_INTERVAL: usize = 1024;

/// 候補ごとの局所正距円筒投影。原点は最初の頂点、φは全頂点の緯度の算術平均。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projection {
    pub phi_deg: f64,
    pub kx: f64,
    pub ky: f64,
    pub origin: LonLat,
}

impl Projection {
    /// `coordinates` は1頂点以上。
    pub fn for_path(coordinates: &[LonLat]) -> Self {
        let phi_deg = coordinates.iter().map(|p| p.lat).sum::<f64>() / coordinates.len() as f64;
        Self {
            phi_deg,
            // 仕様の式 cos(φ·π/180) のとおりに計算する（`to_radians` とは1ulp違うことがある）
            kx: METERS_PER_DEGREE * (phi_deg * std::f64::consts::PI / 180.0).cos(),
            ky: METERS_PER_DEGREE,
            origin: coordinates[0],
        }
    }

    /// 平面座標（m）。
    pub fn xy(&self, p: LonLat) -> (f64, f64) {
        (
            (p.lon - self.origin.lon) * self.kx,
            (p.lat - self.origin.lat) * self.ky,
        )
    }

    /// 2頂点間の平面距離（m）。
    pub fn distance(&self, a: LonLat, b: LonLat) -> f64 {
        ((b.lon - a.lon) * self.kx).hypot((b.lat - a.lat) * self.ky)
    }
}

/// 75m分割後の小線分。添字がsid（経路の先頭から0始まり）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Subsegment {
    /// 元geometryの辺の番号j（頂点jとj+1を結ぶ）。
    pub edge: u32,
    /// 辺内の番号k（0始まり）。
    pub k: u32,
    /// 辺の分割数n。
    pub n: u32,
    /// 補間済みの両端。
    pub start: LonLat,
    pub end: LonLat,
    /// 平面座標の始点Aと方向D。
    a: (f64, f64),
    d: (f64, f64),
}

impl Subsegment {
    /// 点Pまでの距離の2乗（m²）。長さ0の小線分ではt=0。
    pub fn distance_sq(&self, p: (f64, f64)) -> f64 {
        let (px, py) = (p.0 - self.a.0, p.1 - self.a.1);
        let dd = self.d.0 * self.d.0 + self.d.1 * self.d.1;
        let t = if dd > 0.0 {
            ((px * self.d.0 + py * self.d.1) / dd).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (ex, ey) = (px - t * self.d.0, py - t * self.d.1);
        ex * ex + ey * ey
    }
}

/// 1候補の投影と小線分。回廊・高リスク区間・ナビ区間で一度だけ作って共有する。
#[derive(Debug, Clone, PartialEq)]
pub struct Subdivision {
    pub projection: Projection,
    /// 元の各辺の平面距離（m）。長さは頂点数−1。
    pub edge_lengths: Vec<f64>,
    pub segments: Vec<Subsegment>,
}

/// 元の辺の上の比率 `k/n` の点。両端は元の頂点をそのまま使う。
fn interpolate(a: LonLat, b: LonLat, k: u32, n: u32) -> LonLat {
    if k == 0 {
        a
    } else if k == n {
        b
    } else {
        let (k, n) = (f64::from(k), f64::from(n));
        LonLat::new(
            a.lon + (b.lon - a.lon) * k / n,
            a.lat + (b.lat - a.lat) * k / n,
        )
    }
}

impl Subdivision {
    /// `coordinates` は2頂点以上（候補の検証済み）。期限なしで作る。
    pub fn new(coordinates: &[LonLat]) -> Self {
        Self::build(coordinates, &CancelToken::unbounded()).expect("期限なしでは中断しない")
    }

    /// 元の辺1024本ごとに `cancel.check()` を確かめながら作る。
    pub fn build(coordinates: &[LonLat], cancel: &CancelToken) -> Result<Self, Interrupt> {
        let projection = Projection::for_path(coordinates);
        let mut edge_lengths = Vec::with_capacity(coordinates.len().saturating_sub(1));
        let mut segments = Vec::new();
        for (j, pair) in coordinates.windows(2).enumerate() {
            if j % CANCEL_CHECK_INTERVAL == 0 {
                cancel.check()?;
            }
            let (a, b) = (pair[0], pair[1]);
            let length = projection.distance(a, b);
            edge_lengths.push(length);
            // 長さ0の辺も1本残す
            let n = ((length / SUBSEGMENT_LENGTH_M).ceil() as u32).max(1);
            for k in 0..n {
                let start = interpolate(a, b, k, n);
                let end = interpolate(a, b, k + 1, n);
                let pa = projection.xy(start);
                let pb = projection.xy(end);
                segments.push(Subsegment {
                    edge: j as u32,
                    k,
                    n,
                    start,
                    end,
                    a: pa,
                    d: (pb.0 - pa.0, pb.1 - pa.1),
                });
            }
        }
        Ok(Self {
            projection,
            edge_lengths,
            segments,
        })
    }
}

/// 回廊内と判定した事故。1候補1事故1回。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CorridorHit {
    /// 事故ID（gzip CSVの行番号）。
    pub accident: u32,
    /// 最寄りの小線分（同距離は小さいsid）。
    pub sid: u32,
    /// 最寄り小線分までの距離の2乗（m²）。
    pub distance_sq: f64,
}

impl CorridorHit {
    pub fn distance_m(&self) -> f64 {
        self.distance_sq.sqrt()
    }
}

/// 72m以内で移動手段が一致する事故を、事故ID昇順で返す。
///
/// 小線分ごとにbboxを72m広げたセル範囲（gy昇順→gx昇順）へsidを登録し、
/// そのセルにある事故を候補にする。各事故はそのセルに登録されたsidだけと比べる。
/// 72m以内の小線分は必ずその事故のセルに登録されるので、採否と最寄りsidは
/// 全小線分と比べた場合と同じになる。結果は索引の走査順に依存しない。
pub fn corridor_hits(
    subdivision: &Subdivision,
    snapshot: &AccidentSnapshot,
    mode_mask: u8,
    cancel: &CancelToken,
) -> Result<Vec<CorridorHit>, Interrupt> {
    let projection = &subdivision.projection;
    let lat_margin = CORRIDOR_RADIUS_M / projection.ky;
    let lon_margin = CORRIDOR_RADIUS_M / projection.kx;
    // セルごとのsid（登録順＝sid昇順）。HashMapは参照にだけ使い、反復しない。
    let mut cell_sids: std::collections::HashMap<Cell, Vec<u32>> = std::collections::HashMap::new();
    // 候補の事故IDのビット集合。整列せずに事故ID昇順で走査でき、重複も除ける
    let mut candidates = vec![0u64; snapshot.len().div_ceil(64)];
    // 処理量（登録・距離計算の回数）に応じて中断を確かめる
    let mut work = 0usize;
    let mut tick = |amount: usize| -> Result<(), Interrupt> {
        let before = work / CANCEL_CHECK_INTERVAL;
        work += amount;
        if work / CANCEL_CHECK_INTERVAL != before {
            cancel.check()?;
        }
        Ok(())
    };
    for (sid, segment) in subdivision.segments.iter().enumerate() {
        tick(1)?;
        let lat_lo = segment.start.lat.min(segment.end.lat) - lat_margin;
        let lat_hi = segment.start.lat.max(segment.end.lat) + lat_margin;
        let lon_lo = segment.start.lon.min(segment.end.lon) - lon_margin;
        let lon_hi = segment.start.lon.max(segment.end.lon) + lon_margin;
        let (gy_lo, gx_lo) = cell_of(lat_lo, lon_lo);
        let (gy_hi, gx_hi) = cell_of(lat_hi, lon_hi);
        for gy in gy_lo..=gy_hi {
            for gx in gx_lo..=gx_hi {
                let sids = cell_sids.entry((gy, gx)).or_default();
                if sids.is_empty() {
                    // 元CSVの重複行を除かないので、セル内の件数に小さい上限はない
                    for chunk in snapshot.cell((gy, gx)).chunks(CANCEL_CHECK_INTERVAL) {
                        tick(chunk.len())?;
                        for &id in chunk {
                            candidates[id as usize / 64] |= 1 << (id % 64);
                        }
                    }
                }
                sids.push(sid as u32);
            }
        }
    }
    // 各事故を1回だけ判定する（セルの走査順によらず事故ID昇順）
    let columns = snapshot.columns();
    let mut hits = Vec::new();
    for (word_index, &word) in candidates.iter().enumerate() {
        tick(1)?;
        let mut bits = word;
        while bits != 0 {
            let id = (word_index * 64) as u32 + bits.trailing_zeros();
            bits &= bits - 1;
            let index = id as usize;
            if columns.modes[index] & mode_mask == 0 {
                continue;
            }
            let (lat, lon) = (columns.lat[index], columns.lon[index]);
            let p = projection.xy(LonLat::new(lon, lat));
            let sids = &cell_sids[&cell_of(lat, lon)];
            let mut best: Option<(f64, u32)> = None;
            for chunk in sids.chunks(CANCEL_CHECK_INTERVAL) {
                tick(chunk.len())?;
                for &sid in chunk {
                    let d2 = subdivision.segments[sid as usize].distance_sq(p);
                    // sidは昇順なので、同距離では先に見た小さいsidが残る
                    if best.is_none_or(|(b, _)| d2 < b) {
                        best = Some((d2, sid));
                    }
                }
            }
            if let Some((distance_sq, sid)) = best
                && distance_sq <= CORRIDOR_RADIUS_SQ_M2
            {
                hits.push(CorridorHit {
                    accident: id,
                    sid,
                    distance_sq,
                });
            }
        }
    }
    Ok(hits)
}

/// 索引を使わず全小線分と比べる参照実装。索引の走査順に依存しないことの検証用。
pub fn corridor_hits_exhaustive(
    subdivision: &Subdivision,
    snapshot: &AccidentSnapshot,
    mode_mask: u8,
) -> Vec<CorridorHit> {
    let columns = snapshot.columns();
    let projection = &subdivision.projection;
    (0..snapshot.len())
        .filter(|&i| columns.modes[i] & mode_mask != 0)
        .filter_map(|i| {
            let p = projection.xy(LonLat::new(columns.lon[i], columns.lat[i]));
            let mut best: Option<(f64, u32)> = None;
            for (sid, segment) in subdivision.segments.iter().enumerate() {
                let d2 = segment.distance_sq(p);
                if best.is_none_or(|(b, _)| d2 < b) {
                    best = Some((d2, sid as u32));
                }
            }
            let (distance_sq, sid) = best?;
            (distance_sq <= CORRIDOR_RADIUS_SQ_M2).then_some(CorridorHit {
                accident: i as u32,
                sid,
                distance_sq,
            })
        })
        .collect()
}
