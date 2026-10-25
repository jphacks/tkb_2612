import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { NavigationData } from "../lib/types";
import { createRouteGeometry, pointAtProgress, projectPoint, unprojectPoint, nearestRoutePoint } from "./geometry";
import { checkNavigationStart } from "./preflight";
import { calculateRemaining } from "./remaining";

const fixture = JSON.parse(readFileSync("../docs/api/fixtures/navigation-cases.json", "utf8")) as {
  distance_tolerance_m: number;
  cases: { name: string; route_id: string; geometry: { coordinates: [number, number][]; }; expected: { navigation: NavigationData; high_risk_total: number; }; }[];
};
import { localPosition, testRoute } from "./test-utils";

describe("局所平面・開始前検査・残量（7.2/7.4）", () => {
  for (const c of fixture.cases) it(`共有fixture: ${c.name}`, () => {
    const g = createRouteGeometry(c.geometry.coordinates, c.expected.navigation);
    g.computedDistances.forEach((d, i) => expect(Math.abs(d - c.expected.navigation.vertex_distances_m[i])).toBeLessThanOrEqual(fixture.distance_tolerance_m));
    const route = { ...testRoute(), id: c.route_id, geometry: { type: "LineString" as const, coordinates: c.geometry.coordinates }, navigation: c.expected.navigation, high_risk_total: c.expected.high_risk_total };
    const result = checkNavigationStart({ route, selection: "fastest", fastestId: c.route_id, safestId: null, responseGeneration: 2, currentGeneration: 2, mapAvailable: true });
    expect(result.ok).toBe(c.expected.navigation.length_m > 0);
  });
  it("経路最短距離・重複頂点・辺境界の補間", () => {
    const route = testRoute([0, 0, 200, 100]);
    const g = createRouteGeometry(route.geometry.coordinates, route.navigation!);
    expect(nearestRoutePoint(g, localPosition(150, 120)).distanceM).toBeCloseTo(120, 6);
    expect(pointAtProgress(g, 200).segmentIndex).toBe(2);
    expect(pointAtProgress(g, 300).segmentIndex).toBe(2);
    expect(unprojectPoint(g, projectPoint(g, localPosition(123, 45)))).toEqual(localPosition(123, 45));
  });
  it("選定・世代・地図・不正距離軸・sidを拒否し、同一IDの種別は保持する", () => {
    const args = { route: testRoute(), selection: "safest" as const, fastestId: "route-1", safestId: "route-1", responseGeneration: 2, currentGeneration: 2, mapAvailable: true };
    expect(checkNavigationStart(args)).toMatchObject({ ok: true, selection: "safest" });
    for (const patch of [{ safestId: null }, { currentGeneration: 3 }, { mapAvailable: false }, { route: { ...args.route, navigation: null } }, { route: { ...args.route, navigation: { ...args.route.navigation!, vertex_distances_m: [0, 400] } } }]) expect(checkNavigationStart({ ...args, ...patch }).ok).toBe(false);
    const bad = testRoute(); bad.navigation!.sections = [{ id: "route-1:nav-99-99", start_segment_index: 99, end_segment_index: 99, start_m: 0, end_m: 500, severity_weight: 3 }]; bad.high_risk_total = 1;
    expect(checkNavigationStart({ ...args, route: bad }).ok).toBe(false);
  });
  it("距離軸の割合をOSRM残量へ変換し、後退で増加、未確定はnull", () => {
    const r = { distance: 1200, duration: 125, navigation: { length_m: 500 } };
    expect(calculateRemaining(r, null)).toBeNull();
    expect(calculateRemaining(r, 250)).toEqual({ distanceM: 600, durationS: 62.5, displayDistanceM: 600, displayMinutes: 2 });
    expect(calculateRemaining(r, -1)?.distanceM).toBe(1200);
    expect(calculateRemaining(r, 600)?.distanceM).toBe(0);
    expect(calculateRemaining(r, 200)!.distanceM).toBeGreaterThan(calculateRemaining(r, 250)!.distanceM);
  });
});
