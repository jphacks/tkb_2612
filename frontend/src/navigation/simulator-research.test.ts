import { describe, expect, it, vi } from "vitest";
import { createRouteGeometry, projectPoint } from "./geometry";
import { NavigationSimulator } from "./simulator";
import { testRoute } from "./test-utils";

const route = testRoute();
const geometry = createRouteGeometry(route.geometry.coordinates, route.navigation!);
function stopped(seed = 42, jitter = false) {
  const consume = vi.fn();
  const sim = new NavigationSimulator(route, { seed, jitter });
  sim.start(0, consume);
  sim.pump(2500);
  sim.pause();
  return { sim, consume };
}

describe("再検索用の停止位置測位", () => {
  it("tickの進捗を進めず、現在の仮想時刻を毎回1秒進め、consumerには配信しない", () => {
    const { sim, consume } = stopped();
    const first = sim.measureStoppedPosition()!;
    const second = sim.measureStoppedPosition()!;
    expect(projectPoint(geometry, first).x).toBeCloseTo(20, 6);
    expect(projectPoint(geometry, second).x).toBeCloseTo(20, 6);
    expect(first).toEqual({ ...consume.mock.calls.at(-1)![0], timestamp: 3500 });
    expect(second).toEqual({ ...first, timestamp: 4500 });
    expect(first.accuracy).toBe(10);
    expect(sim.clock.now()).toBe(4500);
    expect(sim.state).toBe("paused");
    expect(consume).toHaveBeenCalledTimes(3);
    sim.resume(10_000);
    sim.pump(10_500);
    expect(consume.mock.calls.at(-1)![0].timestamp).toBe(5000);
    expect(projectPoint(geometry, consume.mock.calls.at(-1)![0]).x).toBeCloseTo(30, 6);
  });
  it.each([1, 42, 4294967295])("同じseedと操作列なら停止測位も決定的: seed=%s", (seed) => {
    const a = stopped(seed, true).sim;
    const b = stopped(seed, true).sim;
    expect(a.measureStoppedPosition()).toEqual(b.measureStoppedPosition());
    expect(a.measureStoppedPosition()).toEqual(b.measureStoppedPosition());
  });
  it("揺らぎOFFでも毎回LCGを2回進め、その後のONの測位へ乱数状態を保持する", () => {
    const off = stopped(1, false).sim;
    const on = stopped(1, true).sim;
    off.measureStoppedPosition();
    on.measureStoppedPosition();
    off.setJitter(true);
    expect(off.measureStoppedPosition()).toEqual(on.measureStoppedPosition());
    // 次の停止測位は6測位目。通常走行の6測位目と揺らぎが一致する。
    const measured = projectPoint(geometry, off.measureStoppedPosition()!);
    const reference = new NavigationSimulator(route, { seed: 1, jitter: true });
    const fixes: { lon: number; lat: number }[] = [];
    reference.start(0, p => fixes.push(p));
    for (let t = 1; t <= 5; t++) reference.pump(t * 1000);
    const expected = projectPoint(geometry, fixes[5]);
    expect(measured.x - 20).toBeCloseTo(expected.x - 50, 6);
    expect(measured.y).toBeCloseTo(expected.y, 6);
  });
  it("逸脱試験中は停止した辺の左120mオフセットを新時刻で適用し、20秒で解除する", () => {
    const sim = new NavigationSimulator(route, { jitter: false });
    sim.start(0, () => {});
    sim.tryDeviation();
    sim.pump(1000);
    sim.pause();
    const fix = sim.measureStoppedPosition()!;
    expect(projectPoint(geometry, fix).x).toBeCloseTo(10, 6);
    expect(projectPoint(geometry, fix).y).toBeCloseTo(120, 6);
    for (let i = 0; i < 17; i++) sim.measureStoppedPosition();
    expect(projectPoint(geometry, sim.measureStoppedPosition()!).y).toBeCloseTo(0, 6);
  });
  it("injectの欠測・精度変更を適用し、欠測でも仮想時刻と乱数を保持する", () => {
    const inject = vi.fn((fix, seconds) => seconds === 2 ? null : { ...fix, accuracy: 31 });
    const sim = new NavigationSimulator(route, { seed: 1, inject });
    const consume = vi.fn();
    sim.start(0, consume);
    sim.pump(1000);
    sim.pause();
    expect(sim.measureStoppedPosition()).toBeNull();
    expect(sim.clock.now()).toBe(2000);
    const next = sim.measureStoppedPosition()!;
    expect(next).toMatchObject({ timestamp: 3000, accuracy: 31 });
    expect(inject.mock.calls.map(call => call[1])).toEqual([0, 1, 2, 3]);
    const reference = new NavigationSimulator(route, { seed: 1 });
    const fixes: { lon: number; lat: number }[] = [];
    reference.start(0, p => fixes.push(p));
    reference.pump(3000);
    expect(projectPoint(geometry, next).x - 10).toBeCloseTo(projectPoint(geometry, fixes[3]).x - 30, 6);
    expect(next.lat).toBe(fixes[3].lat);
    expect(consume).toHaveBeenCalledTimes(2);
  });
  it("期限停止からも測位でき、stop後の新規開始はseed・仮想時刻・進捗を初期化する", () => {
    const consume = vi.fn();
    const sim = new NavigationSimulator(route, { seed: 42 });
    sim.start(0, consume);
    const first = consume.mock.calls[0][0];
    for (let t = 1; t <= 65; t++) sim.pump(t * 1000);
    expect(sim.state).toBe("stopped");
    expect(sim.measureStoppedPosition()!.timestamp).toBe(66000);
    sim.stop();
    sim.start(100_000, consume);
    expect(consume.mock.calls.at(-1)![0]).toEqual(first);
  });
  it("停止前の呼出しは拒否し、時計と乱数を進めない", () => {
    const sim = new NavigationSimulator(route);
    expect(() => sim.measureStoppedPosition()).toThrow();
    sim.start(0, () => {});
    expect(() => sim.measureStoppedPosition()).toThrow();
    expect(sim.clock.now()).toBe(0);
  });
});
