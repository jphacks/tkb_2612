import { afterAll, describe, expect, it, vi } from "vitest";
import { NavigationMatcher, type MatchResult } from "./matcher";
import { NavigationSimulator } from "./simulator";
import { localPosition, testRoute, MatchingAudit } from "./test-utils";
import { checkNavigationStart } from "./preflight";
import { calculateRemaining } from "./remaining";
import type { PositionFix } from "./source";
import { readFileSync } from "node:fs";
import type { NavigationData } from "../lib/types";

const audit = new MatchingAudit();
afterAll(() => { if (process.env.NAV_MATCH_REPORT) console.info("M1〜M11集計", JSON.stringify(audit.summaries())); });

const shared = JSON.parse(readFileSync("../docs/api/fixtures/navigation-cases.json", "utf8")) as { cases: { name: string; geometry: { type: "LineString"; coordinates: [number, number][]; }; expected: { navigation: NavigationData; high_risk_total: number; }; }[]; };
const seeds = [1, 2, 3, 42];
const multipliers = [1, 2, 4, 8] as const;
const ids = ["M1", "M2", "M3", "M4", "M5", "M6-50", "M6-51", "M7-50", "M7-51", "M8", "M9", "M10", "M11-pause", "M11-hidden"];

type Output = { t: number; value: Pick<MatchResult, "status" | "p" | "updated"> };
function expectContinuousConfirmation(results: Output[], since: number): void {
  expect(results.filter(r => r.t >= since).every(r => ["tracking", "arrived"].includes(r.value.status) && r.value.updated && r.value.p !== null)).toBe(true);
}
function expectHoldLimit(results: Output[], since: number, limitAt = (_t: number) => 5): void {
  let hold: number | null = null;
  for (const r of results) {
    if (r.t < since) continue;
    if (r.value.status === "matching-hold") hold ??= r.t; else hold = null;
    if (hold !== null) expect(r.t - hold).toBeLessThanOrEqual(limitAt(r.t));
  }
}
function expectCaseHoldLimit(id: string, results: Output[], since: number): void {
  if (["M3", "M5", "M10", "M11-pause", "M11-hidden"].includes(id)) {
    expectHoldLimit(results, since, t => id === "M5" && t <= 35 ? 10 : 5);
  }
}

function run(id: string, jitter: boolean, seed: number, multiplier: 1 | 2 | 4 | 8) {
  const network = vi.spyOn(globalThis, "fetch").mockImplementation(() => { throw new Error("位置更新で通信してはいけません"); });
  const round = ["M1", "M2", "M6-50", "M6-51", "M7-50", "M7-51"].includes(id);
  const duration = id === "M2" || id.endsWith("51") ? 51 : id === "M4" ? 60 : id === "M10" ? 200 : 50;
  const route = testRoute(round ? [0, 200, 0, 100] : id === "M4" ? [-100, 200, 0, 100] : id === "M10" ? [0, 300, 4000] : [0, 300, 500], duration);
  if (round) {
    const f = shared.cases.find(c => c.name === (duration === 51 ? "overlapping_round_trip_between_fixes" : "overlapping_round_trip"))!;
    route.geometry = f.geometry; route.navigation = f.expected.navigation; route.high_risk_total = f.expected.high_risk_total;
  } else {
    // 変換後の先頭辺は300.00000000126m（M4は299.99999999867m）。ceil(length/75)に従う。
    const endSid = id === "M4" ? 3 : 4;
    route.high_risk_total = endSid + 1;
    route.navigation!.sections = [{ id: `route-1:nav-0-${endSid}`, start_segment_index: 0, end_segment_index: endSid, start_m: 0, end_m: Math.min(300, route.navigation!.length_m), severity_weight: 3 }];
  }
  expect(checkNavigationStart({ route, selection: "fastest", fastestId: route.id, safestId: null, responseGeneration: 1, currentGeneration: 1, mapAvailable: true }).ok).toBe(true);
  let trueP = 0;
  const unresolved = id.startsWith("M6") || id.startsWith("M7");
  const late = id.startsWith("M7");
  const sim = new NavigationSimulator(route, {
    seed, jitter, multiplier, inject: (fix, t) => {
      trueP = Math.min(route.navigation!.length_m, route.navigation!.length_m * t / duration);
      if (id === "M10" && t > 10 && t < 100) return null;
      if (late && t < duration - 1) return null;
      let x: number | null = null;
      if (unresolved) x = 100;
      if (id === "M4") { x = t <= 28 ? -100 + 10 * t : t <= 56 ? 460 - 10 * t : -100; trueP = x + 100; }
      if (id === "M5") { x = t <= 25 ? 10 * t : t <= 40 ? 500 - 10 * t : 100; trueP = x; }
      if (x !== null) {
        const baseP = Math.min(500, 500 * t / duration);
        const baseX = round ? baseP <= 200 ? baseP : baseP <= 400 ? 400 - baseP : baseP - 400 : id === "M4" ? (t <= 30 ? -100 + 10 * t : t <= 50 ? 500 - 10 * t : Math.min(100, 10 * t - 500)) : Math.min(500, 10 * t);
        // 入力の差し替えでも、模擬位置源が生成した同じtickの揺らぎを残す。
        fix = { ...fix, lon: fix.lon + localPosition(x).lon - localPosition(baseX).lon };
      }
      return { ...fix, accuracy: id === "M8" || (id === "M9" && t < 45) ? 31 : 10 };
    }
  });
  const matcher = new NavigationMatcher(route.geometry, route.navigation!, sim.clock);
  const results: { t: number; value: MatchResult; trueP: number; }[] = [];
  const onFix = (fix: PositionFix) => {
    const value = matcher.update(fix); const t = fix.timestamp / 1000;
    results.push({ t, value, trueP });
    if (value.p !== null && value.updated) expect(Math.abs(value.p - trueP), `${id} seed=${seed} jitter=${jitter} t=${t} p=${value.p} true=${trueP}`).toBeLessThanOrEqual(Math.max(20, 2 * fix.accuracy) + 1e-6);
    sim.report(value.status);
  };
  sim.start(0, onFix);
  let real = 0;
  const end = id === "M4" ? 70 : id === "M5" ? 60 : id === "M10" ? 214 : id === "M8" ? 65 : unresolved ? duration + 16 : duration + 14;
  for (let t = 1; t <= end; t++) {
    if (id.startsWith("M11") && t === 21) {
      sim.pause(); matcher.pause(); const before = sim.clock.now(); real += 3000; sim.pump(real);
      expect(sim.clock.now()).toBe(before); expect(matcher.poll().status).toBe("paused");
      sim.resume(real); matcher.resume();
    }
    real += 1000 / multiplier; sim.pump(real);
    if (id === "M10" && t > 15 && t < 100) {
      const absent = matcher.poll(); expect(absent.p).toBeNull(); expect(absent.updated).toBe(false);
    }
  }
  const confirms = results.filter(r => r.value.p !== null && r.value.updated);
  const arrivals = results.filter(r => r.value.status === "arrived");
  const at = (t: number) => results.find(r => r.t === t)!;
  if (unresolved) {
    expect(confirms).toHaveLength(0); expect(arrivals).toHaveLength(0);
    expect(sim.state).toBe("stopped"); expect(sim.stopReason).toBe("照合保留");
    expect(sim.clock.now()).toBe((late ? duration + 15 : 60) * 1000);
    if (late) {
      const before = sim.clock.now(); sim.resume(real); matcher.resume();
      for (let i = 1; i <= 15; i++) { real += 1000 / multiplier; sim.pump(real); }
      expect(sim.state).toBe("stopped"); expect(sim.clock.now() - before).toBe(15000);
      expect(results.every(r => r.value.p === null)).toBe(true);
    }
  } else if (id === "M8") {
    expect(confirms).toHaveLength(0); expect(arrivals).toHaveLength(0); expect(results.every(r => r.value.status === "poor-accuracy")).toBe(true);
    expect(sim.state).toBe("stopped"); expect(sim.stopReason).toBe("精度不足"); expect(sim.clock.now()).toBe(65000);
  } else if (id === "M4" || id === "M5") {
    expect(arrivals).toHaveLength(0); expect(confirms[0].t).toBeGreaterThanOrEqual(5); expect(confirms[0].t).toBeLessThanOrEqual(10);
    if (id === "M4") expect(results.filter(r => r.t >= 59).every(r => r.value.status === "tracking")).toBe(true);
    if (id === "M5") {
      expect(confirms.some(r => r.t > 25 && r.t <= 35 && r.value.p! < 240)).toBe(true);
      expect(calculateRemaining(route as Parameters<typeof calculateRemaining>[0], at(40).value.p)!.distanceM).toBeGreaterThan(calculateRemaining(route as Parameters<typeof calculateRemaining>[0], at(25).value.p)!.distanceM);
    }
  } else {
    expect(arrivals.length, `${id} seed=${seed} jitter=${jitter}: 未到着`).toBe(1);
    const minArrival = id === "M9" ? 55 : duration;
    const maxArrival = id === "M9" || id === "M10" || jitter ? duration + 14 : duration + 6;
    expect(arrivals[0].t).toBeGreaterThanOrEqual(minArrival); expect(arrivals[0].t).toBeLessThanOrEqual(maxArrival);
    if (id === "M1" || id === "M2") {
      const deadline = jitter ? 59 : duration + 1;
      expect(results.filter(r => r.t >= deadline).every(r => ["tracking", "arrived"].includes(r.value.status))).toBe(true);
    } else if (id === "M9") {
      expect(results.filter(r => r.t < 45).every(r => r.value.p === null)).toBe(true); expect(confirms[0].t).toBeGreaterThanOrEqual(50); expect(confirms[0].t).toBeLessThanOrEqual(55);
      expectContinuousConfirmation(results, confirms[0].t);
    } else {
      expect(confirms[0].t).toBeGreaterThanOrEqual(5); expect(confirms[0].t).toBeLessThanOrEqual(10);
    }
    if (id === "M10") { expect(results.filter(r => r.t >= 100 && r.t <= 104).every(r => r.value.p === null)).toBe(true); expect(confirms.find(r => r.t >= 100)!.t).toBeGreaterThanOrEqual(105); expect(confirms.find(r => r.t >= 100)!.t).toBeLessThanOrEqual(110); }
    if (id.startsWith("M11")) { expect(results.filter(r => r.t >= 21 && r.t <= 25).every(r => r.value.p === null)).toBe(true); expect(confirms.find(r => r.t >= 21)!.t).toBeGreaterThanOrEqual(26); expect(confirms.find(r => r.t >= 21)!.t).toBeLessThanOrEqual(31); }
    expect(results.filter(r => r.t >= arrivals[0].t - 5).every(r => ["tracking", "arrived"].includes(r.value.status))).toBe(true);
  }
  // 保留を短く繰り返しながら期限だけ通す実装も検出する。
  if (id.startsWith("M11")) {
    expectCaseHoldLimit(id, results.filter(r => r.t <= 20), confirms[0].t);
    expectCaseHoldLimit(id, results, confirms.find(r => r.t >= 21)!.t);
  } else expectCaseHoldLimit(id, results, id === "M10" ? confirms.find(r => r.t >= 100)!.t : confirms[0]?.t ?? 0);
  expect(network).not.toHaveBeenCalled();
  network.mockRestore();
  const reconfirmation = id === "M5" ? confirms.find(r => r.t > 25 && r.value.p! < 240)?.t
    : id === "M10" ? confirms.find(r => r.t >= 100)?.t
    : id.startsWith("M11") ? confirms.find(r => r.t >= 21)?.t : undefined;
  audit.record(`${id} ${jitter ? "ON" : "OFF"}`, results, reconfirmation);
  return results.map(r => ({ t: r.t, value: r.value }));
}

describe("受け入れ検査そのものの回帰（再レビュー4）", () => {
  const samples = (start: number, end: number): Output[] => Array.from({ length: end - start + 1 }, (_, i) => ({
    t: start + i, value: { status: start + i === end ? "arrived" : "tracking", p: 500, updated: true }
  }));
  it("M9は最後の5秒より前の1測位の保留も検出する", () => {
    const results = samples(50, 64);
    results[3].value = { status: "matching-hold", p: null, updated: false };
    expect(() => expectContinuousConfirmation(results, 50)).toThrow();
  });
  it("M11は再確定後の6秒間の連続保留を検出する", () => {
    const results = samples(26, 60);
    for (const r of results) if (r.t >= 35 && r.t <= 41) r.value = { status: "matching-hold", p: null, updated: false };
    for (const id of ["M11-pause", "M11-hidden"]) expect(() => expectCaseHoldLimit(id, results, 26)).toThrow();
  });
});

for (const id of ids) describe(`${id} 全組合せ（7.3/7.4/7.7）`, () => {
  for (const jitter of [false, true]) for (const seed of seeds) {
    const outputs: ReturnType<typeof run>[] = [];
    for (const multiplier of multipliers) it(`jitter=${jitter}, seed=${seed}, 倍率=${multiplier}`, () => {
      const output = run(id, jitter, seed, multiplier);
      if (outputs.length) expect(output).toEqual(outputs[0]);
      outputs.push(output);
    });
  }
});

for (const id of ["M1", "M2", "M3", "M4", "M5"]) describe(`${id} 追加seedの頑健さ`, () => {
  for (let seed = 1; seed <= 512; seed++) it(`jitter=true, seed=${seed}, 倍率=1`, () => { run(id, true, seed, 1); });
});

for (const [id, seed] of [["M1", 8], ["M2", 5]] as const) describe(`${id} 追加レビューの正常走行反例`, () => {
  for (const multiplier of multipliers) it(`seed=${seed}, 倍率=${multiplier}`, () => { run(id, true, seed, multiplier); });
});
