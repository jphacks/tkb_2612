import type { RouteCandidate } from "../lib/types";
import type { MatchResult } from "./matcher";
export function localPosition(x: number, y = 0) {
  return { lon: 139 + x / (6371008.8 * Math.cos(35 * Math.PI / 180)) * 180 / Math.PI, lat: 35 + y / 6371008.8 * 180 / Math.PI };
}
export function testRoute(xs = [0, 500], duration = 50): RouteCandidate {
  const coordinates = xs.map((x): [number, number] => { const p = localPosition(x); return [p.lon, p.lat]; });
  let length = 0;
  const distances = xs.map((x, i) => { if (i) length += Math.abs(x - xs[i - 1]); return length; });
  return {
    id: "route-1", geometry: { type: "LineString", coordinates }, distance: length, duration,
    navigation: { projection_lat_deg: 35, length_m: length, vertex_distances_m: distances, sections: [] }, high_risk_total: 0,
    accident_count: 0, fatal_accidents: 0, injury_accidents: 0, deaths: 0, injuries: 0, weather_match: null,
    weighted_risk: 0, risk_per_km: 0, points_shown: 0, accident_points: { type: "FeatureCollection", features: [] },
    high_risk_sections: { type: "FeatureCollection", features: [] }, exact_condition_count: 0, extra_minutes: 0, within_detour: true
  };
}

type TimeRange = { min: number; max: number } | null;
interface MatchingSummary {
  runs: number;
  firstConfirmation: TimeRange;
  stableConfirmation: TimeRange;
  reconfirmation: TimeRange;
  arrival: TimeRange;
  maxErrorM: number;
}
/** 正解は試験側だけで使用する。NAV_MATCH_REPORT=1で再現可能な集計を出す。 */
export class MatchingAudit {
  private readonly records = new Map<string, MatchingSummary>();
  record(key: string, results: readonly { t: number; value: MatchResult; trueP: number }[], reconfirmation?: number): void {
    const summary = this.records.get(key) ?? { runs: 0, firstConfirmation: null, stableConfirmation: null, reconfirmation: null, arrival: null, maxErrorM: 0 };
    const confirmed = results.filter(r => r.value.updated);
    let lastHold = -1;
    for (let i = results.length - 1; i >= 0; i--) if (!results[i].value.updated) { lastHold = i; break; }
    const stable = results.slice(lastHold + 1).find(r => r.value.updated);
    const include = (old: TimeRange, t: number | undefined): TimeRange => t === undefined ? old : { min: Math.min(old?.min ?? t, t), max: Math.max(old?.max ?? t, t) };
    summary.runs++;
    summary.firstConfirmation = include(summary.firstConfirmation, confirmed[0]?.t);
    summary.stableConfirmation = include(summary.stableConfirmation, stable?.t);
    summary.reconfirmation = include(summary.reconfirmation, reconfirmation);
    summary.arrival = include(summary.arrival, results.find(r => r.value.status === "arrived")?.t);
    for (const r of confirmed) summary.maxErrorM = Math.max(summary.maxErrorM, Math.abs(r.value.p! - r.trueP));
    this.records.set(key, summary);
  }
  summaries(): Record<string, MatchingSummary> { return Object.fromEntries(this.records); }
}
