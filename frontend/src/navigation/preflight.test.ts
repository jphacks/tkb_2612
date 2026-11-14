import { describe, expect, it } from "vitest";
import { checkNavigationStart } from "./preflight";
import { createRouteGeometry } from "./geometry";
import { testRoute } from "./test-utils";

/** 1m間隔の短い辺を `shortEdges` 本並べ、最後に `longM` の長い辺を1本つなぐ経路。
 *  選定区間は小線分を1本おきに `sectionCount` 件置く。 */
function longRoute(shortEdges: number, longM: number, sectionCount: number) {
  const xs = Array.from({ length: shortEdges + 1 }, (_, i) => i);
  xs.push(shortEdges + longM);
  const route = testRoute(xs);
  const n = route.navigation!;
  const c = route.geometry.coordinates;
  n.projection_lat_deg = c.reduce((sum, p) => sum + p[1], 0) / c.length;
  const g = createRouteGeometry(c, n);
  const d = g.computedDistances;
  n.vertex_distances_m = d; n.length_m = d.at(-1)!; route.distance = n.length_m;
  const starts: number[] = [], ends: number[] = [];
  for (let i = 1; i < d.length; i++) {
    const count = Math.max(1, Math.ceil(Math.hypot((c[i][0] - c[i - 1][0]) * g.kx, (c[i][1] - c[i - 1][1]) * g.ky) / 75));
    for (let j = 0; j < count; j++) { starts.push(d[i - 1] + (d[i] - d[i - 1]) * j / count); ends.push(d[i - 1] + (d[i] - d[i - 1]) * (j + 1) / count); }
  }
  n.sections = Array.from({ length: sectionCount }, (_, k) => {
    const a = 2 * k;
    return { id: `${route.id}:nav-${a}-${a}`, start_segment_index: a, end_segment_index: a, start_m: starts[a], end_m: ends[a], severity_weight: 3 };
  });
  route.high_risk_total = sectionCount;
  return { route, segments: starts.length };
}

const start = (route: ReturnType<typeof testRoute>) =>
  checkNavigationStart({ route, selection: "fastest", fastestId: route.id, middleId: null, safestId: null, responseGeneration: 1, currentGeneration: 1, mapAvailable: true });

describe("250kmに広げたナビの上限（issue #39）", () => {
  it("旧上限（110,000m・8,233区間）を超える経路でも開始できる", () => {
    // API上限（経路250km）の範囲内で、旧上限だけを超える経路。
    const { route, segments } = longRoute(14_000, 230_000, 8_500);
    expect(route.navigation!.length_m).toBeGreaterThan(110_000);
    expect(route.distance).toBeLessThanOrEqual(250_000);
    expect(segments).toBeGreaterThan(17_000);
    expect(start(route)).toEqual({ ok: true, route, selection: "fastest" });
  });
  it("9,767区間を超えたら開始しない", () => {
    const { route } = longRoute(13_000, 300_000, 8_500);
    const n = route.navigation!;
    n.sections = Array.from({ length: 9_768 }, () => n.sections[0]);
    expect(start(route)).toEqual({ ok: false, reason: "ナビ区間の数が不正です" });
  });
  it("全長340,000m超は開始しない", () => {
    const ok = longRoute(10, 339_900, 1).route;
    expect(start(ok).ok).toBe(true);
    const over = longRoute(10, 340_100, 1).route;
    expect(over.navigation!.length_m).toBeGreaterThan(340_000);
    expect(start(over)).toEqual({ ok: false, reason: "累積距離配列が不正です" });
  });
});
