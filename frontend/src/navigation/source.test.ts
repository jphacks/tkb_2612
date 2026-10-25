import { describe, expect, it } from "vitest";
import { VirtualClock, RealClock } from "./clock";
import { NavigationSimulator, lcgNext } from "./simulator";
import type { PositionFix } from "./source";
import { testRoute } from "./test-utils";
import { createRouteGeometry, projectPoint } from "./geometry";

describe("時計・模擬位置源（7.7）", () => {
  it("LCGのseed=1手計算ベクトル", () => {
    let seed = 1;
    for (const value of [1015568748, 1586005467, 2165703038, 3027450565]) { seed = lcgNext(seed); expect(seed).toBe(value); }
  });
  it("実時間時計の鮮度時計と単調時計は独立", () => {
    const clock = new RealClock(() => 12345, () => 900);
    expect(clock.now()).toBe(12345); expect(clock.monotonicNow()).toBe(900);
    const virtual = new VirtualClock(); virtual.set(5000); expect(virtual.now()).toBe(5000);
  });
  for (const seed of [1, 2, 3, 42]) for (const jitter of [false, true]) it(`倍率・一時停止・OFFでの乱数消費: seed=${seed}, jitter=${jitter}`, () => {
    const run = (multiplier: 1 | 2 | 4 | 8, pause: boolean) => {
      const fixes: PositionFix[] = [];
      const sim = new NavigationSimulator(testRoute(), { seed, jitter, multiplier });
      sim.start(0, p => fixes.push(p));
      let real = 0;
      for (let t = 1; t <= 55; t++) {
        if (pause && t === 21) { sim.pause(); real += 3000; sim.pump(real); expect(sim.clock.now()).toBe(20000); sim.resume(real); }
        real += 1000 / multiplier; sim.pump(real);
      }
      return { fixes, sim };
    };
    const reference = run(1, false);
    for (const multiplier of [1, 2, 4, 8] as const) {
      expect(run(multiplier, false).fixes).toEqual(reference.fixes);
      expect(run(multiplier, true).fixes).toEqual(reference.fixes);
    }
    const sim = new NavigationSimulator(testRoute(), { seed, jitter: false }); const off: PositionFix[] = [];
    sim.start(0, p => off.push(p)); sim.pump(1000); sim.setJitter(true); sim.pump(2000);
    const on: PositionFix[] = []; const other = new NavigationSimulator(testRoute(), { seed, jitter: true }); other.start(0, p => on.push(p)); other.pump(1000); other.pump(2000);
    expect(off[2]).toEqual(on[2]);
  });
  it("5秒ちょうどは進め、5秒超は追いつかせず明示再開を待つ", () => {
    const values: PositionFix[] = []; const sim = new NavigationSimulator(testRoute());
    sim.start(0, p => values.push(p)); sim.pump(5000); expect(values.length).toBe(6);
    sim.pump(10001); expect(sim.state).toBe("paused"); expect(values.length).toBe(6);
    sim.pump(11001); expect(values.length).toBe(6); sim.resume(11001); sim.pump(12001); expect(values.at(-1)!.timestamp).toBe(6000);
  });
  it("左120mを仮想20秒だけ加え、辺境界と終点は正長辺を使う", () => {
    const route = testRoute([0, 0, 200, 100]); const g = createRouteGeometry(route.geometry.coordinates, route.navigation!);
    const fixes: PositionFix[] = []; const sim = new NavigationSimulator(route, { jitter: false }); sim.start(0, p => fixes.push(p)); sim.tryDeviation();
    for (let t = 1; t <= 21; t++) sim.pump(t * 1000);
    expect(projectPoint(g, fixes[1]).y).toBeCloseTo(120, 6); expect(projectPoint(g, fixes[19]).y).toBeCloseTo(120, 6); expect(projectPoint(g, fixes[20]).y).toBeCloseTo(0, 6);
    for (let t = 22; t <= 50; t++) sim.pump(t * 1000);
    sim.tryDeviation(); sim.pump(51000); expect(projectPoint(g, fixes.at(-1)!).y).toBeCloseTo(-120, 6);
  });
  it("欠測でも時計と乱数は進み、accuracy注入は揺らぎを変えない", () => {
    const fixes: PositionFix[] = []; const sim = new NavigationSimulator(testRoute(), { inject: (p, t) => t === 1 ? null : { ...p, accuracy: 31 } });
    sim.start(0, p => fixes.push(p)); sim.pump(1000); sim.pump(2000);
    expect(fixes.map(p => p.timestamp)).toEqual([0, 2000]); expect(fixes[1].accuracy).toBe(31); expect(sim.clock.now()).toBe(2000);
  });
});

describe("模擬停止期限と再開始", () => {
  it("到着フィードバックがなく欠測が続いても終点後15秒で停止する", () => {
    const sim = new NavigationSimulator(testRoute(), { inject: () => null });
    sim.start(0, () => { throw new Error("欠測を配信してはいけません"); });
    for (let t = 1; t <= 65; t++) sim.pump(t * 1000);
    expect(sim.state).toBe("stopped"); expect(sim.clock.now()).toBe(65000);
  });
  it("終了・新規開始ではseedと停止期限を初期化する", () => {
    const fixes: PositionFix[] = []; const sim = new NavigationSimulator(testRoute(), { seed: 42 });
    sim.start(0, p => fixes.push(p)); sim.pump(1000); sim.stop();
    expect(sim.clock.now()).toBe(0); sim.start(2000, p => fixes.push(p)); sim.pump(3000);
    expect(fixes.slice(2)).toEqual(fixes.slice(0, 2));
  });
});

it("seed=1の最初の2乱数を半径・偏角へ変換し、距離軸とOSRM速度を分離する", () => {
  const route = testRoute(); route.distance = 1000;
  const sim = new NavigationSimulator(route); const g = createRouteGeometry(route.geometry.coordinates, route.navigation!);
  const fixes: PositionFix[] = []; sim.start(0, p => fixes.push(p));
  expect(projectPoint(g, fixes[0]).x).toBeCloseTo(-2.64995806120035, 6);
  expect(projectPoint(g, fixes[0]).y).toBeCloseTo(2.8479599525381905, 6);
  expect(sim.baseSpeedMps).toBe(20); expect(sim.progressSpeedMps).toBe(10);
});
