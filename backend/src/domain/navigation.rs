//! 表示用の高リスク区間とナビ用の全対象区間（behavior.md 7章）。
//!
//! どちらも小線分ごとの深刻度（距離減衰・天候倍率なし）だけを入力にとる。
//! `navigation-cases.json` の入力（geometryとsid別の重み）をそのまま使える。

use crate::contract::engine::MAX_HIGH_RISK_SECTIONS;
use crate::contract::{CancelToken, HighRiskSegment, Interrupt, NavigationData, NavigationSection};

use super::corridor::{CANCEL_CHECK_INTERVAL, Subdivision};

/// 高リスク区間・ナビ対象とする小線分の重みの下限。
pub const HIGH_RISK_MIN_WEIGHT: u64 = 3;

/// 表示用の高リスク区間（`(-weight, sid)` 昇順の先頭100件）と、
/// 100件で切る前の重み3以上の小線分数。期限なしで作る。
pub fn display_sections(subdivision: &Subdivision, weights: &[u64]) -> (Vec<HighRiskSegment>, u64) {
    display_sections_checked(subdivision, weights, &CancelToken::unbounded())
        .expect("期限なしでは中断しない")
}

/// [`display_sections`] を、整列の前後で `cancel.check()` を確かめながら作る。
/// 整列の対象は最大で小線分数（16,466本）。
pub fn display_sections_checked(
    subdivision: &Subdivision,
    weights: &[u64],
    cancel: &CancelToken,
) -> Result<(Vec<HighRiskSegment>, u64), Interrupt> {
    cancel.check()?;
    let mut targets: Vec<(u64, usize)> = weights
        .iter()
        .enumerate()
        .filter(|&(_, &w)| w >= HIGH_RISK_MIN_WEIGHT)
        .map(|(sid, &w)| (w, sid))
        .collect();
    let total = targets.len() as u64;
    targets.sort_by_key(|&(w, sid)| (std::cmp::Reverse(w), sid));
    cancel.check()?;
    let sections = targets
        .into_iter()
        .take(MAX_HIGH_RISK_SECTIONS)
        .map(|(severity_weight, sid)| {
            let segment = &subdivision.segments[sid];
            HighRiskSegment {
                segment_index: sid as u32,
                source_vertex_start_index: segment.edge,
                start: segment.start,
                end: segment.end,
                severity_weight,
            }
        })
        .collect();
    Ok((sections, total))
}

/// 元geometryの各頂点までの累積距離（m）。先頭0、末尾が `length_m`。
pub fn vertex_distances(subdivision: &Subdivision) -> Vec<f64> {
    let mut distances = Vec::with_capacity(subdivision.edge_lengths.len() + 1);
    let mut total = 0.0;
    distances.push(total);
    for &length in &subdivision.edge_lengths {
        total += length;
        distances.push(total);
    }
    distances
}

/// 小線分の始点と終点の累積距離。端では元頂点の累積値を直接使う。
fn segment_range_m(subdivision: &Subdivision, vertex_distances: &[f64], sid: usize) -> (f64, f64) {
    let segment = &subdivision.segments[sid];
    let j = segment.edge as usize;
    let length = subdivision.edge_lengths[j];
    let (k, n) = (f64::from(segment.k), f64::from(segment.n));
    let start = if segment.k == 0 {
        vertex_distances[j]
    } else {
        vertex_distances[j] + length * k / n
    };
    let end = if segment.k + 1 == segment.n {
        vertex_distances[j + 1]
    } else {
        vertex_distances[j] + length * (k + 1.0) / n
    };
    (start, end)
}

/// 選定経路のナビ用データ。重み3以上のsidが連続する最大の列を1区間に結合する。
/// 100件で切る前の全対象を使い、切り捨てない。期限なしで作る。
pub fn navigation_data(subdivision: &Subdivision, weights: &[u64]) -> NavigationData {
    navigation_data_checked(subdivision, weights, &CancelToken::unbounded())
        .expect("期限なしでは中断しない")
}

/// [`navigation_data`] を、小線分1024本ごとに `cancel.check()` を確かめながら作る。
pub fn navigation_data_checked(
    subdivision: &Subdivision,
    weights: &[u64],
    cancel: &CancelToken,
) -> Result<NavigationData, Interrupt> {
    cancel.check()?;
    let vertex_distances_m = vertex_distances(subdivision);
    let length_m = *vertex_distances_m.last().expect("頂点は1個以上");
    let mut sections: Vec<NavigationSection> = Vec::new();
    let mut open: Option<(usize, usize, u64)> = None;
    let close = |(start_sid, end_sid, weight): (usize, usize, u64),
                 sections: &mut Vec<NavigationSection>| {
        let (start_m, _) = segment_range_m(subdivision, &vertex_distances_m, start_sid);
        let (_, end_m) = segment_range_m(subdivision, &vertex_distances_m, end_sid);
        sections.push(NavigationSection {
            start_segment_index: start_sid as u32,
            end_segment_index: end_sid as u32,
            start_m,
            end_m,
            severity_weight: weight,
        });
    };
    for (sid, &weight) in weights.iter().enumerate() {
        if sid % CANCEL_CHECK_INTERVAL == 0 {
            cancel.check()?;
        }
        if weight >= HIGH_RISK_MIN_WEIGHT {
            open = Some(match open {
                Some((start, _, total)) => (start, sid, total + weight),
                None => (sid, sid, weight),
            });
        } else if let Some(section) = open.take() {
            close(section, &mut sections);
        }
    }
    if let Some(section) = open {
        close(section, &mut sections);
    }
    Ok(NavigationData {
        projection_lat_deg: subdivision.projection.phi_deg,
        length_m,
        vertex_distances_m,
        sections,
    })
}
