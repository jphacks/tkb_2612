import { describe, expect, it } from "vitest";
import { NavigationMatcher } from "./matcher";
import { VirtualClock } from "./clock";
import { localPosition, testRoute } from "./test-utils";
import { NavigationSimulator } from "./simulator";
import { checkNavigationStart } from "./preflight";
import { createRouteGeometry } from "./geometry";
import type { PositionFix } from "./source";
import { ACCURACY_LIMIT_M as A, CONFIRM_FIXES as N, CONFIRM_MS, FIX_FUTURE_SKEW_MS, FIX_MAX_AGE_MS, NO_FIX_GRACE_MS } from "./thresholds";

/** 照合確定の秒数と、測位なしの猶予秒数。 */
const C = CONFIRM_MS / 1000, G = NO_FIX_GRACE_MS / 1000;

function setup(xs = [0, 500]) {
  const route = testRoute(xs); const clock = new VirtualClock(); const matcher = new NavigationMatcher(route.geometry, route.navigation!, clock);
  const fix = (t: number, x = 100, accuracy = 10, y = 0, timestamp = t * 1000) => { clock.set(t * 1000); return matcher.update({ ...localPosition(x, y), accuracy, timestamp }); };
  return { route, clock, matcher, fix };
}

describe("受理と継続判定の境界（7.3/11章）", () => {
  it("規定の測位数かつ確認時間、どちらか片方では確定しない", () => {
    // 測位数は足りても確認時間の直前では確定しない。
    const a = setup(); expect(a.fix(0).p).toBeNull();
    for (let i = 1; i < N; i++) expect(a.fix(i * 0.1).p).toBeNull();
    expect(a.fix(C - 0.001).p).toBeNull(); expect(a.fix(C).status).toBe("tracking");
    // 確認時間を過ぎても測位数が足りなければ確定しない。
    const b = setup(); expect(b.fix(0).p).toBeNull();
    for (let i = 1; i < N - 1; i++) expect(b.fix(C + i * 0.1).p).toBeNull();
    expect(b.fix(C + 1).status).toBe("tracking");
  });
  it("accuracyが上限ちょうどなら利用し、猶予内の単発の超過は状態を保つ", () => {
    const { fix } = setup(); for (let t = 0; t < C; t++) fix(t, 100, A); expect(fix(C, 100, A).status).toBe("tracking");
    const bad = fix(C + 1, 100, A + 0.001); expect(bad).toMatchObject({ status: "tracking", p: null, accepted: null, updated: false });
    // 単発の後は確認し直さずに進行を続ける。
    expect(fix(C + 2)).toMatchObject({ status: "tracking", updated: true });
  });
  it("上限超過が猶予を超えて続けば精度不足になり、回復は新しい確認時間を要する", () => {
    const { fix } = setup(); for (let t = 0; t < C; t++) fix(t); expect(fix(C).status).toBe("tracking");
    for (let t = C + 1; t <= C + G; t++) expect(fix(t, 100, A + 1).status).toBe("tracking");
    expect(fix(C + G + 1, 100, A + 1).status).toBe("poor-accuracy");
    const back = C + G + 2;
    for (let t = back; t < back + C; t++) expect(fix(t).p).toBeNull(); expect(fix(back + C).status).toBe("tracking");
  });
  it("正の極小accuracyでも重複候補を有限の状態で保留する", () => {
    const { fix } = setup([0, 200, 0, 100]);
    for (let t = 0; t <= 10; t++) expect(fix(t, 100, Number.MIN_VALUE)).toMatchObject({
      status: "matching-hold", p: null, updated: false, accepted: { accuracy: Number.MIN_VALUE }
    });
  });
  it("鮮度5秒を受理し、5秒超・未来・逆順・重複・非有限・矩形外を棄却", () => {
    const { fix, matcher, clock } = setup();
    expect(fix(FIX_MAX_AGE_MS / 1000, 100, 10, 0, 0).accepted?.timestamp).toBe(0);
    expect(fix(6, 100, 10, 0, 6000 - FIX_MAX_AGE_MS - 1).accepted).toBeNull();
    expect(fix(6, 100, 10, 0, 6000 + FIX_FUTURE_SKEW_MS + 1).accepted).toBeNull();
    // 端末時計のずれとして、許容幅までの未来は受理する。
    const ahead = 6000 + FIX_FUTURE_SKEW_MS;
    expect(fix(6, 100, 10, 0, ahead).accepted?.timestamp).toBe(ahead);
    expect(fix(7, 100, 10, 0, ahead).accepted).toBeNull(); expect(fix(7, 100, 10, 0, 5000).accepted).toBeNull();
    const base: PositionFix = { ...localPosition(100), accuracy: 10, timestamp: 8000 }; clock.set(8000);
    for (const patch of [{ lon: NaN }, { lat: Infinity }, { lon: 121.999 }, { lon: 154.001 }, { lat: 19.999 }, { lat: 46.001 }, { accuracy: 0 }, { accuracy: -1 }, { accuracy: NaN }, { timestamp: NaN }]) expect(matcher.update({ ...base, ...patch }).accepted).toBeNull();
    expect(matcher.update(base).accepted).toEqual(base);
  });
  it("猶予を超える欠測・無効位置が確認/到着の時間を進めない", () => {
    const { fix, matcher, clock } = setup(); for (let t = 0; t <= C + 1; t++) fix(t, 500);
    const last = (C + 1) * 1000;
    clock.set(last + NO_FIX_GRACE_MS); expect(matcher.poll().status).toBe("tracking");
    clock.set(last + NO_FIX_GRACE_MS + 1); expect(matcher.poll().status).toBe("poor-accuracy");
    const back = C + 1 + G + 1;
    expect(fix(back, 500).p).toBeNull(); for (let t = back + 1; t < back + C; t++) fix(t, 500); expect(fix(back + C, 500).status).toBe("tracking");
    // 到着の3測位・5秒は再確認の後から数える。
    for (let t = back + C + 1; t < back + C + 6; t++) expect(fix(t, 500).status).toBe("tracking"); expect(fix(back + C + 6, 500).status).toBe("arrived");
  });
  it("30秒ちょうどの停止と超過の双方で5秒再確認、超過は旧枝を解除", () => {
    for (const gap of [30, 30.001]) {
      const { fix } = setup([-100, 200, 0, 100]);
      for (let t = 0; t <= 17; t++) fix(t, -100 + 10 * t);
      const first = 17 + gap; expect(fix(first, 100).p).toBeNull();
      for (let t = first + 1; t < first + C; t++) expect(fix(t, 100).p).toBeNull();
      if (gap > 30) for (let t = first + C; t <= first + C + 5; t++) expect(fix(t, 100).p).toBeNull();
      else expect(fix(first + C, 100).p).toBeCloseTo(200, 6);
    }
  });
  it("一意性と横距離は独立し、保留中も最短距離と受理位置を公開", () => {
    const { fix } = setup([0, 200, 0, 100]);
    const ambiguous = fix(0, 100); expect(ambiguous.status).toBe("matching-hold"); expect(ambiguous.distanceToRouteM).toBeCloseTo(0, 6); expect(ambiguous.accepted?.accuracy).toBe(10);
    const outside = fix(1, 100, 10, 120); expect(outside.p).toBeNull(); expect(outside.distanceToRouteM).toBeCloseTo(120, 6); expect(outside.accepted?.timestamp).toBe(1000);
  });
  it("短い座標ジャンプ後は旧確定を更新せず再確認する", () => {
    const { fix } = setup([0, 4000]); for (let t = 0; t < C; t++) fix(t); expect(fix(C).p).not.toBeNull();
    const at = C + 1;
    const jump = fix(at, 2000); expect(jump.p).toBeNull(); expect(jump.lastConfirmedP).toBeCloseTo(100, 6);
    for (let t = at + 1; t < at + C; t++) expect(fix(t, 2000).p).toBeNull(); expect(fix(at + C, 2000).p).toBeCloseTo(2000, 6);
  });
  it("終了で位置と旧進捗を破棄し、停止後の入力を採用しない", () => {
    const { fix, matcher } = setup(); fix(0); fix(1); fix(5); matcher.stop();
    expect(fix(6)).toMatchObject({ status: "stopped", p: null, lastConfirmedP: null, accepted: null });
  });
  it("保持状態の要素数は走行時間とともに増えない", () => {
    const { fix, matcher } = setup([0, 200, 0, 100]);
    const size = (value: unknown): number => value !== null && typeof value === "object" ? 1 + Object.values(value).reduce<number>((sum, item) => sum + size(item), 0) : 1;
    for (let t = 0; t < 100; t++) fix(t, 100); const before = size(matcher);
    for (let t = 100; t < 10000; t++) fix(t, 100);
    expect(size(matcher)).toBe(before);
  });
});

describe("到着の各境界（7.4）", () => {
  for (const { length, x, y, arrived } of [
    { length: 100, x: 89.99, y: 0, arrived: false },
    { length: 100, x: 90, y: 0, arrived: true },
    { length: 500, x: 479.99, y: 0, arrived: false },
    { length: 500, x: 480, y: 0, arrived: true },
    { length: 500, x: 530, y: 0, arrived: true },
    { length: 500, x: 530.01, y: 0, arrived: false },
    { length: 500, x: 500, y: 30, arrived: true },
    { length: 500, x: 500, y: 30.01, arrived: false },
  ]) it(`L=${length}, x=${x}, y=${y}: 到着=${arrived}`, () => {
    const { fix } = setup([0, length]);
    // 横距離30mの境界ではaccuracy=30を使い、照合確認の条件も満たす。
    // 確定(C秒)の次の測位から到着の3測位・5秒を数える。
    for (let t = 0; t <= C + 5; t++) expect(fix(t, x, 30, y).status).not.toBe("arrived");
    expect(fix(C + 6, x, 30, y).status === "arrived").toBe(arrived);
  });
});

describe("独立レビューの反例", () => {
  it("重複の途中から後退する入力を順方向の別枝へ誤確定しない", () => {
    const { fix } = setup([-100, 200, 0, 100]);
    for (let t = 0; t <= 30; t++) {
      const x = 180 - 10 * t; const result = fix(t, Math.max(-100, x));
      if (result.updated) expect(Math.abs(result.p! - Math.max(0, x + 100))).toBeLessThanOrEqual(20);
      if (t < 19) expect(result.p).toBeNull();
    }
  });
  for (const step of [5, 10, 20, 50]) it(`${step}mの元辺でも単純直線の進行を確定し到着する`, () => {
    const { fix } = setup(Array.from({ length: 500 / step + 1 }, (_, i) => i * step));
    const first: number[] = []; const arrived: number[] = [];
    for (let t = 0; t <= 56; t++) { const r = fix(t, Math.min(500, 10 * t)); if (r.updated && !first.length) first.push(t); if (r.status === "arrived") { arrived.push(t); break; } }
    expect(first[0]).toBeGreaterThanOrEqual(C); expect(first[0]).toBeLessThanOrEqual(C + 5);
    expect(arrived[0]).toBeGreaterThanOrEqual(50); expect(arrived[0]).toBeLessThanOrEqual(56);
  });
  it("pollや到着後の呼出しは新規測位を返さない", () => {
    const { fix, clock, matcher } = setup(); const first = fix(0); clock.set(1000);
    expect(matcher.poll()).toMatchObject({ accepted: null, lastAccepted: first.accepted, updated: false });
    for (let t = 1; t <= 17; t++) fix(t, 500);
    expect(fix(18, 500).accepted).toBeNull();
  });
});

it("頂点5m手前での反転を頂点通過と誤確定しない", () => {
  const { fix } = setup([-100, 200, 0, 100]);
  for (let t = 0; t <= 70; t++) {
    const x = t <= 29 ? -100 + 10 * t : Math.max(-100, 490 - 10 * t);
    const r = fix(t, x);
    if (r.updated) expect(Math.abs(r.p! - (x + 100)), `t=${t}, p=${r.p}, true=${x + 100}`).toBeLessThanOrEqual(20);
  }
});

it("減速して頂点5m手前で反転しても復路へ誤確定しない", () => {
  const { fix } = setup([-100, 200, 0, 100]);
  for (let t = 0; t <= 70; t++) {
    const x = t <= 29 ? -100 + 10 * t : t === 30 ? 195 : Math.max(-100, 500 - 10 * t);
    const r = fix(t, x); if (r.updated) expect(Math.abs(r.p! - (x + 100)), `t=${t}`).toBeLessThanOrEqual(20);
  }
});

for (const amplitude of [0.001, 1, 5]) it(`僅かに曲がる短辺列（振幅${amplitude}m）でも進行・到着する`, () => {
  const route = testRoute();
  route.geometry.coordinates = Array.from({ length: 101 }, (_, i) => { const p = localPosition(5 * i, i % 2 * amplitude); return [p.lon, p.lat]; });
  const n = route.navigation!;
  n.projection_lat_deg = route.geometry.coordinates.reduce((sum, p) => sum + p[1], 0) / 101;
  const calculated = createRouteGeometry(route.geometry.coordinates, n);
  n.vertex_distances_m = calculated.computedDistances; n.length_m = calculated.computedDistances.at(-1)!;
  route.distance = n.length_m;
  expect(checkNavigationStart({ route, selection: "fastest", fastestId: route.id, safestId: null, responseGeneration: 1, currentGeneration: 1, mapAvailable: true }).ok).toBe(true);
  const sim = new NavigationSimulator(route, { jitter: false }); const matcher = new NavigationMatcher(route.geometry, n, sim.clock);
  let first: number | null = null, arrived: number | null = null;
  sim.start(0, fix => { const r = matcher.update(fix); if (r.updated) { first ??= fix.timestamp / 1000; expect(Math.abs(r.p! - Math.min(n.length_m, n.length_m * fix.timestamp / 50000))).toBeLessThanOrEqual(20); } if (r.status === "arrived") arrived = fix.timestamp / 1000; sim.report(r.status); });
  for (let t = 1; t <= 56; t++) sim.pump(t * 1000);
  expect(first).not.toBeNull(); expect(first!).toBeGreaterThanOrEqual(C); expect(first!).toBeLessThanOrEqual(C + 5);
  expect(arrived).not.toBeNull(); expect(arrived!).toBeGreaterThanOrEqual(50); expect(arrived!).toBeLessThanOrEqual(56);
});

it("初回の測位待ちも猶予超なら精度不足とし旧受理を作らない", () => {
  const { clock, matcher } = setup(); clock.set(NO_FIX_GRACE_MS); expect(matcher.poll().status).toBe("waiting");
  clock.set(NO_FIX_GRACE_MS + 1); expect(matcher.poll()).toMatchObject({ status: "poor-accuracy", accepted: null });
});

it("直角に接続する往復でも静止した重複位置を確定しない", () => {
  const route = testRoute();
  route.geometry.coordinates = [[0, 0], [10, 0], [10, 200], [10, 0], [20, 0]].map(([x, y]) => { const p = localPosition(x, y); return [p.lon, p.lat]; });
  const n = route.navigation!; n.projection_lat_deg = route.geometry.coordinates.reduce((sum, p) => sum + p[1], 0) / 5;
  n.vertex_distances_m = createRouteGeometry(route.geometry.coordinates, n).computedDistances; n.length_m = n.vertex_distances_m.at(-1)!; route.distance = n.length_m;
  expect(checkNavigationStart({ route, selection: "fastest", fastestId: route.id, safestId: null, responseGeneration: 1, currentGeneration: 1, mapAvailable: true }).ok).toBe(true);
  const clock = new VirtualClock(); const matcher = new NavigationMatcher(route.geometry, n, clock);
  for (let t = 0; t <= 70; t++) { clock.set(t * 1000); const r = matcher.update({ ...localPosition(10, 5), accuracy: 10, timestamp: t * 1000 }); expect(r.p).toBeNull(); expect(r.status).toBe("matching-hold"); }
});
