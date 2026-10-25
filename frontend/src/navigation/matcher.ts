import type { NavigationData, RouteCandidate } from "../lib/types";
import type { Clock } from "./clock";
import type { PositionFix } from "./source";
import { createRouteGeometry, projectPoint, routeProjections, type RouteProjection } from "./geometry";

export type MatchStatus = "waiting" | "tracking" | "poor-accuracy" | "matching-hold" | "arrived" | "paused" | "stopped";
export interface MatchResult {
  status: MatchStatus;
  /** 今回の確定進捗。更新停止中はnull。到着時も0/Lに置換しない。 */
  p: number | null;
  lastConfirmedP: number | null;
  lastConfirmedTimestamp: number | null;
  lastAccepted: PositionFix | null;
  distanceToRouteM: number | null;
  accuracy: number | null;
  timestamp: number | null;
  accepted: PositionFix | null;
  updated: boolean;
}
// 正のaccuracyが極小でも、共分散の除算を有限に保つ（標準偏差の下限0.1m）。
const positionVariance = (accuracy: number): number => Math.max(0.01, accuracy * accuracy / 4);
interface Confirmation { since: number; count: number; }
interface Motion {
  n: number; time: number; value: number; tt: number; tv: number; yy: number;
}
// 中心化した十分統計を更新し、長時間でも桁落ちと保持量の増加を避ける。
function observeMotion(previous: Motion | null, t: number, y: number): Motion {
  const m = previous ? { ...previous } : { n: 0, time: 0, value: 0, tt: 0, tv: 0, yy: 0 };
  if (m.n >= 32) { m.n *= 31 / 32; m.tt *= 31 / 32; m.tv *= 31 / 32; m.yy *= 31 / 32; }
  const dt = t - m.time, dy = y - m.value;
  m.n++; m.time += dt / m.n; m.value += dy / m.n;
  m.tt += dt * (t - m.time); m.tv += dt * (y - m.value); m.yy += dy * (y - m.value);
  return m;
}
/** 各有向辺の最良仮説。過去の測位ではなく位置・速度の十分統計を保持する。 */
interface Hypothesis {
  segment: number; direction: 1 | -1; p: number; v: number;
  pp: number; pv: number; vv: number; score: number; motion: Motion | null;
}

export class NavigationMatcher {
  private readonly geometry;
  private selected: { segment: number; direction: 1 | -1 } | null = null;
  private previousTimestamp: number | null = null;
  private hypotheses: Hypothesis[] = [];
  private originAnchor: number | null = null;
  private originSeen = false;
  private initialSearch = true;
  private confirmation: Confirmation | null = null;
  private arrival: Confirmation | null = null;
  private trusted = false;
  private lastTimestamp = -Infinity;
  private lastGood = -Infinity;
  private lastUsable = -Infinity;
  private searchSince = -Infinity;
  private previousGood: PositionFix | null = null;
  private waitingSince: number;
  private result: MatchResult = { status: "waiting", p: null, lastConfirmedP: null, lastConfirmedTimestamp: null, lastAccepted: null, distanceToRouteM: null, accuracy: null, timestamp: null, accepted: null, updated: false };
  constructor(geometry: RouteCandidate["geometry"], navigation: NavigationData, private readonly clock: Clock) {
    this.waitingSince = clock.now();
    const original = createRouteGeometry(geometry.coordinates, navigation);
    // 同方向の一直線を一つの照合辺にまとめる。距離は元のAPI軸をそのまま使う。
    const indices = [0];
    for (let i = 1; i < original.points.length; i++) {
      const b = original.points[indices.at(-1)!], c = original.points[i];
      if (Math.hypot(c.x - b.x, c.y - b.y) < 1e-9) continue;
      while (indices.length > 1) {
        const a = original.points[indices[indices.length - 2]];
        const ux = b.x - a.x, uy = b.y - a.y, vx = c.x - b.x, vy = c.y - b.y;
        if (ux * vx + uy * vy <= 0 || Math.abs(ux * vy - uy * vx) > 1e-9 * Math.hypot(ux, uy) * Math.hypot(vx, vy)) break;
        indices.pop();
      }
      indices.push(i);
    }
    this.geometry = { ...original, points: indices.map(i => original.points[i]), distances: indices.map(i => original.distances[i]), computedDistances: indices.map(i => original.computedDistances[i]) };
    if (navigation.length_m <= 0) throw new Error("長さ0の経路では照合できません");
  }
  private firstSegmentAt(p: number): number {
    let lo = 0, hi = this.geometry.distances.length - 1;
    while (lo < hi) {
      const mid = Math.floor((lo + hi) / 2);
      if (this.geometry.distances[mid + 1] < p) lo = mid + 1; else hi = mid;
    }
    return lo;
  }
  private matchHypotheses(projections: RouteProjection[], fix: PositionFix, dt: number): RouteProjection | null {
    const g = this.geometry, point = projectPoint(g, fix), accuracy = fix.accuracy;
    const near = projections.filter(q => q.distanceM <= accuracy + 1e-6 && g.distances[q.segmentIndex + 1] > g.distances[q.segmentIndex]);
    const nearest = projections.reduce((a, b) => b.distanceM < a.distanceM ? b : a);
    const available = near.length ? near : [nearest];
    // この縮約は全候補が同じ局所進捗を示す場合だけ。辺の変更自体では曖昧さを解除しない。
    const angles = available.map(q => {
      const a = g.points[q.segmentIndex], b = g.points[q.segmentIndex + 1];
      return Math.atan2(b.y - a.y, b.x - a.x);
    });
    const relative = angles.map(angle => Math.atan2(Math.sin(angle - angles[0]), Math.cos(angle - angles[0])));
    const contiguous = available.every((q, i) => !i || q.segmentIndex === available[i - 1].segmentIndex + 1);
    const local = Math.max(...relative) - Math.min(...relative) <= Math.PI / 2 + 1e-6
      && (contiguous || Math.max(...available.map(q => q.p)) - Math.min(...available.map(q => q.p)) <= 2 * accuracy);
    if (local) {
      const q = available.reduce((a, b) => b.distanceM < a.distanceM ? b : a);
      const old = this.hypotheses.length ? this.hypotheses.reduce((a, b) => a.score < b.score ? a : b) : null;
      if (old && Math.abs(projections[old.segment].p - q.p) > 2 * accuracy + 1e-6) this.resetConfirmation();
      const motion = observeMotion(old?.segment === q.segmentIndex ? old.motion : null, fix.timestamp / 1000, q.p);
      const v = motion.n >= 3 && motion.tt > 1e-6 ? motion.tv / motion.tt : old?.v ?? 0;
      const direction = v < 0 ? -1 : 1;
      this.hypotheses = [{ segment: q.segmentIndex, direction, p: q.p, v, pp: positionVariance(accuracy), pv: 0, vv: 1,
        score: 0, motion }];
      this.selected = { segment: q.segmentIndex, direction };
      return q;
    }
    // 交差の別枝は経路上の連続移動と区別できないので、全枝から再確認する。
    const crossing = available.some(a => available.some(b => {
      if (Math.abs(a.segmentIndex - b.segmentIndex) <= 1 || Math.abs(a.p - b.p) <= 2 * accuracy) return false;
      const u = g.points[a.segmentIndex], v = g.points[a.segmentIndex + 1];
      const w = g.points[b.segmentIndex], z = g.points[b.segmentIndex + 1];
      return Math.abs((v.x - u.x) * (z.y - w.y) - (v.y - u.y) * (z.x - w.x)) > 1e-8;
    }));
    if (crossing) { this.hypotheses = []; this.resetConfirmation(); this.initialSearch = false; }
    if (!this.hypotheses.length) {
      this.hypotheses = available.flatMap(q => ([1, -1] as const).map(direction => ({
        segment: q.segmentIndex, direction, p: q.p, v: 0,
        pp: positionVariance(accuracy), pv: 0, vv: accuracy * accuracy, score: 0, motion: null
      })));
    } else {
      const best = new Map<string, Hypothesis>();
      for (const h of this.hypotheses) {
        const sigma2 = positionVariance(accuracy);
        // 通常継続と、方向・測位間の反転時点を追加する逆行仮説を競合させる。
        for (const reverse of [false, true]) {
          if (reverse && Math.abs(h.v) < accuracy / 5) continue;
          const direction = reverse ? (h.direction === 1 ? -1 : 1) : h.direction;
          const v = reverse ? -h.v : h.v, pv = reverse ? -h.pv : h.pv;
          const centre = h.p + v * dt;
          // 加速度1m/s²の連続速度モデル。位置の誤差幅はaccuracyに従う。
          const pp = h.pp + 2 * dt * pv + dt * dt * h.vv + dt ** 4 / 4;
          const cross = pv + dt * h.vv + dt ** 3 / 2, vv = h.vv + dt * dt;
          const margin = 2 * accuracy;
          for (let segment = this.firstSegmentAt(centre - margin); segment < projections.length && g.distances[segment] <= centre + margin; segment++) {
            const start = g.distances[segment], end = g.distances[segment + 1];
            if (end <= start || centre < start - 2 * accuracy || centre > end + 2 * accuracy) continue;
            const q = projections[segment];
            if (q.distanceM > Math.max(accuracy, nearest.distanceM) + 1e-6) continue;
            const a = g.points[segment], b = g.points[segment + 1];
            const dx = b.x - a.x, dy = b.y - a.y, length2 = dx * dx + dy * dy;
            let raw = start + ((point.x - a.x) * dx + (point.y - a.y) * dy) / length2 * (end - start);
            if (segment === 0 && raw < start || segment === projections.length - 1 && raw > end) raw = q.p;
            let motion = reverse ? null : h.motion;
            const boundary = direction === 1 ? end : start;
            if (Math.abs(raw - boundary) > 2 * accuracy && Math.abs(h.v) > accuracy / 5) motion = observeMotion(motion, fix.timestamp / 1000, raw);
            // 逆行の時点は測位間の任意時刻。反転位置を前回測位に固定しない。
            const reach = Math.abs(h.v) * dt;
            const predicted = reverse ? Math.max(h.p - reach, Math.min(h.p + reach, raw)) : centre;
            const innovation = raw - predicted, variance = pp + sigma2;
            const p = Math.max(start, Math.min(end, predicted + pp / variance * innovation));
            const velocity = direction * Math.max(0, direction * (v + cross / variance * innovation));
            // 頂点付近を除いた移動の回帰を枝変更後も維持し、位置差を吸収してしまう仮説を検出する。
            // accuracy由来の1標本分を分散へ足し、少数標本の小さな残差だけで過信しない。
            let continuity = 0;
            if (!reverse && h.motion && h.motion.n >= 6 && h.motion.tt > 1e-6 && q.p > accuracy && q.p < g.lengthM - accuracy) {
              const m = h.motion, elapsed = fix.timestamp / 1000 - m.time;
              const prediction = m.value + m.tv / m.tt * elapsed;
              const residualVariance = Math.max(0.01, (m.yy - m.tv * m.tv / m.tt + positionVariance(accuracy)) / (m.n - 1));
              const variance = residualVariance * (1 + 1 / m.n + elapsed * elapsed / m.tt);
              continuity = Math.max(0, (raw - prediction) ** 2 / variance - 1) / 2;
            }
            const score = h.score + continuity + (reverse ? 9 : 0) + innovation * innovation / (2 * variance)
              + q.distanceM * q.distanceM / (2 * sigma2) + Math.log(variance / sigma2) / 2;
            const candidate: Hypothesis = { segment, direction, p, v: velocity,
              pp: pp * sigma2 / variance, pv: cross * sigma2 / variance, vv: Math.max(0, vv - cross * cross / variance), score, motion };
            const key = `${segment}:${direction}`;
            if (!best.has(key) || score < best.get(key)!.score) best.set(key, candidate);
          }
        }
      }
      if (!best.size) { this.hypotheses = []; this.resetConfirmation(); return null; }
      // 同じ有向辺へ合流した履歴は最良の1組へ縮約し、経路規模で上限を持たせる。
      const min = Math.min(...Array.from(best.values(), h => h.score));
      this.hypotheses = Array.from(best.values()).filter(h => h.score - min <= 24).map(h => ({ ...h, score: h.score - min }));
    }
    if (Math.hypot(point.x, point.y) <= accuracy + 1e-6) this.originSeen = true;
    // 新規開始の始点からの移動だけを使い、途中からの全経路探索には適用しない。
    if (this.initialSearch && this.originSeen) {
      this.originAnchor ??= Math.min(...this.hypotheses.map(h => h.p));
      const first = this.hypotheses.reduce((a, b) => a.p < b.p ? a : b);
      if (first.p - this.originAnchor > 2 * accuracy) {
        this.hypotheses = this.hypotheses.filter(h => Math.abs(h.p - first.p) <= 2 * accuracy);
        this.initialSearch = false;
      }
    }
    const winner = this.hypotheses.reduce((a, b) => a.score < b.score ? a : b);
    const competing = this.hypotheses.some(h => Math.abs(h.p - winner.p) > 2 * accuracy && h.score - winner.score < 4.5);
    if (crossing || competing) { this.selected = null; return null; }
    const a = g.points[winner.segment], b = g.points[winner.segment + 1];
    if (this.selected && (this.selected.segment !== winner.segment || this.selected.direction !== winner.direction)) {
      const old = this.selected.segment, u = g.points[old], v = g.points[old + 1];
      if (this.selected.direction !== winner.direction || (v.x - u.x) * (b.x - a.x) + (v.y - u.y) * (b.y - a.y) < -1e-6 * Math.hypot(v.x - u.x, v.y - u.y) * Math.hypot(b.x - a.x, b.y - a.y)
        || Math.abs(projections[old].p - projections[winner.segment].p) > 2 * accuracy + Math.abs(winner.v) * dt + 1e-6) this.resetConfirmation();
    }
    this.selected = { segment: winner.segment, direction: winner.direction };
    const next = winner.segment + winner.direction;
    const boundary = g.distances[winner.direction === 1 ? winner.segment + 1 : winner.segment];
    if (next >= 0 && next < projections.length) {
      const u = g.points[next], v = g.points[next + 1];
      if ((b.x - a.x) * (v.x - u.x) + (b.y - a.y) * (v.y - u.y) < -1e-6 * Math.hypot(b.x - a.x, b.y - a.y) * Math.hypot(v.x - u.x, v.y - u.y) && Math.abs(winner.p - boundary) <= 2 * accuracy) return null;
    }
    // フィルタの予測位置ではなく、選んだ枝への今回の射影を公開する。
    return projections[winner.segment];
  }
  private resetConfirmation(): void { this.confirmation = null; this.arrival = null; this.trusted = false; }
  private resetSearch(): void { this.selected = null; this.hypotheses = []; this.previousTimestamp = null; this.originAnchor = null; this.originSeen = false; this.initialSearch = false; this.resetConfirmation(); this.previousGood = null; }
  private suspend(status: MatchStatus, accepted: PositionFix | null = null, distance: number | null = null): MatchResult {
    this.resetConfirmation();
    this.result = { ...this.result, status, p: null, distanceToRouteM: distance, accuracy: accepted?.accuracy ?? this.result.accuracy, timestamp: accepted?.timestamp ?? this.result.timestamp, lastAccepted: accepted ?? this.result.lastAccepted, accepted, updated: false };
    return this.result;
  }
  pause(): MatchResult { this.resetSearch(); return this.suspend("paused"); }
  resume(): MatchResult { this.waitingSince = this.clock.now(); this.resetSearch(); return this.suspend("waiting"); }
  stop(): void {
    this.resetSearch(); this.lastTimestamp = -Infinity; this.lastGood = -Infinity; this.lastUsable = -Infinity; this.searchSince = -Infinity;
    this.result = { status: "stopped", p: null, lastConfirmedP: null, lastConfirmedTimestamp: null, lastAccepted: null, distanceToRouteM: null, accuracy: null, timestamp: null, accepted: null, updated: false };
  }
  poll(): MatchResult {
    if (["paused", "stopped", "arrived"].includes(this.result.status)) return { ...this.result, accepted: null, updated: false };
    if (this.clock.now() - (this.result.status === "waiting" ? this.waitingSince : this.lastGood) > 5000) return this.suspend("poor-accuracy");
    return { ...this.result, p: null, accepted: null, updated: false };
  }
  update(fix: PositionFix): MatchResult {
    if (["paused", "stopped", "arrived"].includes(this.result.status)) return { ...this.result, accepted: null, updated: false };
    const now = this.clock.now();
    const valid = [fix.lon, fix.lat, fix.accuracy, fix.timestamp, now].every(Number.isFinite) && fix.lon >= 122 && fix.lon <= 154 && fix.lat >= 20 && fix.lat <= 46 && fix.accuracy > 0 && fix.timestamp > this.lastTimestamp && fix.timestamp <= now && now - fix.timestamp <= 5000;
    if (!valid) return this.suspend("poor-accuracy");
    this.lastTimestamp = fix.timestamp;
    const projections = routeProjections(this.geometry, fix);
    const nearest = projections.reduce((a, b) => b.distanceM < a.distanceM ? b : a);
    if (fix.accuracy > 30) { this.resetSearch(); return this.suspend("poor-accuracy", fix, nearest.distanceM); }
    const gap = fix.timestamp - this.lastGood;
    if (!Number.isFinite(this.lastGood)) this.searchSince = fix.timestamp;
    if (Number.isFinite(this.lastGood) && (gap > 30000 || fix.timestamp - Math.max(this.lastUsable, this.searchSince) > 30000)) { this.resetSearch(); this.searchSince = fix.timestamp; }
    else if (gap > 5000) {
      this.resetConfirmation();
      // 30秒以内は枝だけを保持し、欠測中の速度・回帰を外挿しない。
      this.hypotheses = this.hypotheses.map(h => ({ ...h, v: 0, pp: positionVariance(fix.accuracy), pv: 0, vv: fix.accuracy ** 2, motion: null }));
    }
    this.lastGood = fix.timestamp;
    // 短い間隔の大きな座標ジャンプは全経路から確認し直す。
    if (this.previousGood) {
      const a = projectPoint(this.geometry, this.previousGood), b = projectPoint(this.geometry, fix);
      if (Math.hypot(b.x - a.x, b.y - a.y) > 70 * (fix.timestamp - this.previousGood.timestamp) / 1000 + this.previousGood.accuracy + fix.accuracy) this.resetSearch();
    }
    this.previousGood = fix;
    const corridor = Math.max(20, fix.accuracy);
    if (nearest.distanceM > corridor + 1e-6) { this.resetSearch(); return this.suspend("matching-hold", fix, nearest.distanceM); }
    const interval = this.previousTimestamp === null ? 1 : (fix.timestamp - this.previousTimestamp) / 1000;
    const dt = interval > 5 ? 1 : Math.max(0.001, interval);
    this.previousTimestamp = fix.timestamp;
    const chosen = this.matchHypotheses(projections, fix, dt);
    if (!chosen) return this.suspend("matching-hold", fix, nearest.distanceM);
    this.lastUsable = fix.timestamp;
    const previouslyTrusted = this.trusted;
    this.confirmation ??= { since: fix.timestamp, count: 0 };
    this.confirmation.count = Math.min(3, this.confirmation.count + 1);
    this.trusted = this.confirmation.count >= 3 && fix.timestamp - this.confirmation.since >= 5000;
    if (!this.trusted) {
      this.result = { ...this.result, status: "matching-hold", p: null, distanceToRouteM: nearest.distanceM, accuracy: fix.accuracy, timestamp: fix.timestamp, lastAccepted: fix, accepted: fix, updated: false };
      return this.result;
    }
    const endpoint = this.geometry.points.at(-1)!;
    const point = projectPoint(this.geometry, fix);
    const endDistance = Math.hypot(point.x - endpoint.x, point.y - endpoint.y);
    const p = chosen.p, L = this.geometry.lengthM;
    const atEnd = p >= 0.9 * L && L - p <= 20 + 1e-6 && endDistance <= 30 + 1e-6;
    // 初回・再確認の時間を到着の継続へ算入しない。
    if (atEnd && previouslyTrusted) { this.arrival ??= { since: fix.timestamp, count: 0 }; this.arrival.count = Math.min(3, this.arrival.count + 1); } else this.arrival = null;
    const arrived = this.arrival !== null && this.arrival.count >= 3 && fix.timestamp - this.arrival.since >= 5000;
    this.result = { status: arrived ? "arrived" : "tracking", p, lastConfirmedP: p, lastConfirmedTimestamp: fix.timestamp, lastAccepted: fix, distanceToRouteM: nearest.distanceM, accuracy: fix.accuracy, timestamp: fix.timestamp, accepted: fix, updated: true };
    return this.result;
  }
}
