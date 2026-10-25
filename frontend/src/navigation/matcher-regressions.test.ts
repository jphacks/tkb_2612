import { afterAll, describe, expect, it } from "vitest";
import { NavigationMatcher, type MatchResult } from "./matcher";
import { NavigationSimulator, lcgNext, type SimulationMultiplier } from "./simulator";
import { VirtualClock } from "./clock";
import { localPosition, testRoute, MatchingAudit } from "./test-utils";
import { createRouteGeometry } from "./geometry";

const audit = new MatchingAudit();
afterAll(() => { if (process.env.NAV_MATCH_REPORT) console.info("反例集計", JSON.stringify(audit.summaries())); });

const seeds = [1, 2, 3, 42];

// 利用者決定（2026-10-25）：最大半径8mの揺らぎでは、頂点2〜5m手前の
// 反転とM1/M2の正常な折返しを位置だけで識別できない入力がある。
// M1/M2の期限内の確定・到着も必須のため、以下の6入力は誤差20mの検査から外す。
// skipせず非到着と出力の契約を検査し、現在の誤確定値や誤確定の発生は期待値にしない。
const identificationLimits = [
  { offset: 2, seed: 1 },
  { offset: 2, seed: 2 },
  { offset: 2, seed: 3 },
  { offset: 2, seed: 42 },
  { offset: 5, seed: 1 },
  { offset: 5, seed: 2 },
] as const;

function runPreVertexReversal(offset: number, jitter: boolean, seed: number, multiplier: SimulationMultiplier = 1) {
  const route = testRoute([-100, 200, 0, 100], 60);
  const trueX = (t: number) => t <= (300 - offset) / 10 ? -100 + 10 * t : Math.max(-100, 500 - 2 * offset - 10 * t);
  const sim = new NavigationSimulator(route, { jitter, seed, multiplier, inject: (fix, t) => {
    const baseX = t <= 30 ? -100 + 10 * t : t <= 50 ? 500 - 10 * t : Math.min(100, 10 * t - 500);
    return { ...fix, lon: fix.lon + localPosition(trueX(t)).lon - localPosition(baseX).lon };
  } });
  const matcher = new NavigationMatcher(route.geometry, route.navigation!, sim.clock);
  const results: { t: number; trueP: number; value: MatchResult }[] = [];
  sim.start(0, fix => {
    const t = fix.timestamp / 1000;
    results.push({ t, trueP: trueX(t) + 100, value: matcher.update(fix) });
  });
  for (let t = 1; t <= 70; t++) sim.pump(t * 1000 / multiplier);
  // 最初の誤差で中断せず、0〜70秒の全71測位について非到着と出力の整合性を検査する。
  expect(results).toHaveLength(71);
  for (const { t, value: r } of results) {
    expect(r.status).not.toBe("arrived");
    expect(["matching-hold", "tracking"]).toContain(r.status);
    expect(typeof r.updated).toBe("boolean");
    expect(r.updated).toBe(r.status === "tracking");
    if (r.updated) {
      expect(Number.isFinite(r.p)).toBe(true);
      expect(r.p).toBeGreaterThanOrEqual(0);
      expect(r.p).toBeLessThanOrEqual(route.navigation!.length_m);
      expect(r.lastConfirmedP).toBe(r.p);
      expect(r.lastConfirmedTimestamp).toBe(t * 1000);
    } else {
      expect(r.p).toBeNull();
    }
    if (r.lastConfirmedP !== null) {
      expect(Number.isFinite(r.lastConfirmedP)).toBe(true);
      expect(r.lastConfirmedP).toBeGreaterThanOrEqual(0);
      expect(r.lastConfirmedP).toBeLessThanOrEqual(route.navigation!.length_m);
      expect(Number.isFinite(r.lastConfirmedTimestamp)).toBe(true);
      expect(r.lastConfirmedTimestamp).toBeGreaterThanOrEqual(0);
      expect(r.lastConfirmedTimestamp).toBeLessThanOrEqual(t * 1000);
    } else {
      expect(r.lastConfirmedTimestamp).toBeNull();
    }
  }
  audit.record(`手前${offset}m ${jitter ? "ON" : "OFF"}`, results);
  return results;
}

describe("頂点手前の反転：誤差20m以内または保留を必須とする入力（再レビュー1）", () => {
  for (const offset of [2, 5, 10, 15, 20]) for (const jitter of [false, true]) for (const seed of seeds) {
    // 限界の入力は直下の専用テストで全件実行する。通る入力の誤差検査は維持する。
    if (jitter && identificationLimits.some(input => input.offset === offset && input.seed === seed)) continue;
    it(`頂点${offset}m手前、jitter=${jitter}, seed=${seed}`, () => {
      for (const { t, trueP, value: r } of runPreVertexReversal(offset, jitter, seed)) {
        if (r.updated) expect(Math.abs(r.p! - trueP), `t=${t}, p=${r.p}, 真値=${trueP}`).toBeLessThanOrEqual(20);
      }
    });
  }
});

describe("頂点手前の反転：識別の限界の記録（利用者決定 2026-10-25）", () => {
  for (const { offset, seed } of identificationLimits) {
    it(`頂点${offset}m手前、jitter=true, seed=${seed}：全期間で非到着・出力の型と範囲が正常`, () => {
      runPreVertexReversal(offset, true, seed);
    });
  }
});

describe("重複区間の再確認（再レビュー2）", () => {
  for (const duration of [50, 51]) for (const jitter of [false, true]) for (const seed of seeds) for (const mode of ["pause", "gap"] as const) {
    it(`${duration}秒, ${mode}, jitter=${jitter}, seed=${seed}`, () => {
      const route = testRoute([0, 200, 0, 100], duration);
      const sim = new NavigationSimulator(route, { jitter, seed });
      const matcher = new NavigationMatcher(route.geometry, route.navigation!, sim.clock);
      const after: MatchResult[] = [];
      sim.start(0, fix => {
        const t = fix.timestamp / 1000;
        if (mode === "gap" && t > 9 && t < 40) return;
        const r = matcher.update(fix);
        if (t >= 41) after.push(r);
      });
      let real = 0;
      for (let t = 1; t < duration; t++) {
        if (mode === "pause" && t === 41) {
          sim.pause(); matcher.pause(); real += 3000; sim.pump(real);
          sim.resume(real); matcher.resume();
        }
        real += 1000; sim.pump(real);
      }
      expect(after).toHaveLength(duration - 41);
      expect(after.every(r => r.p === null && r.status === "matching-hold"), JSON.stringify(after.map(r => r.p))).toBe(true);
    });
  }
});

describe("交差から非隣接辺への移行（再レビュー3）", () => {
  for (const jitter of [false, true]) for (const seed of seeds) for (const mirrored of [false, true]) it(`3測位・5秒を再確認、jitter=${jitter}, seed=${seed}, 反転=${mirrored}`, () => {
    const route = testRoute();
    route.geometry.coordinates = [[-100, -100], [100, 100], [-100, 100], [100, -100]].map(([x, y]) => {
      const p = localPosition(x, mirrored ? -y : y); return [p.lon, p.lat];
    });
    const n = route.navigation!;
    n.vertex_distances_m = createRouteGeometry(route.geometry.coordinates, n).computedDistances;
    n.length_m = n.vertex_distances_m.at(-1)!;
    const clock = new VirtualClock(); const matcher = new NavigationMatcher(route.geometry, n, clock);
    const results: MatchResult[] = [];
    let rng = seed;
    for (let t = 0; t <= 17; t++) {
      clock.set(t * 1000);
      rng = lcgNext(rng); const r = 8 * Math.sqrt(rng / 4294967296);
      rng = lcgNext(rng); const theta = 2 * Math.PI * rng / 4294967296;
      const x = t <= 10 ? -100 + 10 * t : 10 * (t - 10);
      const y = t <= 10 ? -100 + 10 * t : -10 * (t - 10);
      const p = localPosition(x + (jitter ? r * Math.cos(theta) : 0), (mirrored ? -y : y) + (jitter ? r * Math.sin(theta) : 0));
      results.push(matcher.update({ ...p, timestamp: t * 1000, accuracy: 10 }));
    }
    // 交差近傍を通過しても、別辺を確定するまで新しい5秒が必要。
    expect(results[10].p).toBeNull();
    for (let t = 11; t <= 15; t++) expect(results[t].p, `t=${t}`).toBeNull();
    const first = results.findIndex((r, t) => t > 10 && r.updated);
    expect(first).toBeGreaterThanOrEqual(16); expect(first).toBeLessThanOrEqual(17);
    expect(Math.abs(results[first].p! - n.vertex_distances_m[2] - Math.sqrt(2) * (100 + 10 * (first - 10)))).toBeLessThanOrEqual(20);
  });
  it("交差後は5秒経過しても新しい2測位だけでは確定しない", () => {
    const route = testRoute();
    route.geometry.coordinates = [[-100, -100], [100, 100], [-100, 100], [100, -100]].map(([x, y]) => {
      const p = localPosition(x, y); return [p.lon, p.lat];
    });
    const n = route.navigation!;
    n.vertex_distances_m = createRouteGeometry(route.geometry.coordinates, n).computedDistances;
    n.length_m = n.vertex_distances_m.at(-1)!;
    const clock = new VirtualClock(); const matcher = new NavigationMatcher(route.geometry, n, clock);
    const update = (t: number, x: number, y: number) => {
      clock.set(t * 1000); return matcher.update({ ...localPosition(x, y), accuracy: 10, timestamp: t * 1000 });
    };
    for (let t = 0; t <= 10; t++) update(t, -100 + 10 * t, -100 + 10 * t);
    expect(update(11, 10, -10).p).toBeNull();
    expect(update(16, 60, -60).p).toBeNull();
    expect(update(17, 70, -70).status).toBe("tracking");
  });
});

// 全倍率の再現入力と、倍率1の追加seedを別々に実行する。
function expectProgressError(results: ReturnType<typeof runPreVertexReversal>): void {
  for (const { t, trueP, value } of results) if (value.updated) {
    expect(Math.abs(value.p! - trueP), `t=${t}, p=${value.p}, 真値=${trueP}`).toBeLessThanOrEqual(20 + 1e-6);
  }
}
function runRepeatedRoundTrip(seed: number, multiplier: SimulationMultiplier = 1) {
  const route = testRoute([0, 200, 0, 200, 0, 100], 50);
  const sim = new NavigationSimulator(route, { jitter: true, seed, multiplier });
  const matcher = new NavigationMatcher(route.geometry, route.navigation!, sim.clock);
  const results: { t: number; value: MatchResult; trueP: number }[] = [];
  sim.start(0, fix => {
    const t = fix.timestamp / 1000, value = matcher.update(fix);
    const trueP = Math.min(900, 18 * t);
    results.push({ t, value, trueP });
    if (value.updated) expect(Math.abs(value.p! - trueP), `seed=${seed}, t=${t}, p=${value.p}, 真値=${trueP}`).toBeLessThanOrEqual(20 + 1e-6);
    sim.report(value.status);
  });
  for (let t = 1; t <= 64; t++) sim.pump(t * 1000 / multiplier);
  const arrival = results.find(r => r.value.status === "arrived");
  expect(arrival, `seed=${seed}：未到着`).toBeDefined();
  expect(arrival!.t).toBeGreaterThanOrEqual(50); expect(arrival!.t).toBeLessThanOrEqual(64);
  expect(results.filter(r => r.t >= arrival!.t - 5).every(r => r.value.updated)).toBe(true);
  audit.record("通常往復2回 ON", results);
  return results;
}

describe("追加レビューの再現入力：全倍率", () => {
  for (const multiplier of [1, 2, 4, 8] as const) {
    it(`10m手前反転 seed=4、倍率=${multiplier}`, () => { expectProgressError(runPreVertexReversal(10, true, 4, multiplier)); });
    it(`通常往復2回 seed=3、倍率=${multiplier}`, () => { runRepeatedRoundTrip(3, multiplier); });
  }
});
describe("追加seedの頑健さ：揺らぎON・倍率1", () => {
  for (let seed = 1; seed <= 512; seed++) {
    it(`10m手前反転 seed=${seed}：全確定の誤差20m以内・非到着`, () => { expectProgressError(runPreVertexReversal(10, true, seed)); });
    it(`通常往復2回 seed=${seed}：全確定の誤差20m以内・期限内到着`, () => { runRepeatedRoundTrip(seed); });
  }
});
