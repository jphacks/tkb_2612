import { describe, expect, it } from "vitest";
import { createDemoRouteResponse } from "../api/demo";
import type { RouteRequest } from "../lib/types";
import { createDemoGuidance } from "./demoGuidance";
import { nextGuidance, type GuidanceStep } from "./guidance";

const steps: GuidanceStep[] = [
  { id: "right", maneuver: "right", atM: 100 },
  { id: "left", maneuver: "left", atM: 250 },
  { id: "arrival", maneuver: "arrive", atM: 400 },
];

describe("案内地点と確定進捗", () => {
  it("距離を減らし、通過した案内を次の案内へ切り替える", () => {
    expect(nextGuidance(steps, 20)).toEqual({ id: "right", maneuver: "right", distanceM: 80 });
    expect(nextGuidance(steps, 80)?.distanceM).toBe(20);
    expect(nextGuidance(steps, 100)).toEqual({ id: "left", maneuver: "left", distanceM: 150 });
    expect(nextGuidance(steps, 300)).toEqual({ id: "arrival", maneuver: "arrive", distanceM: 100 });
    expect(nextGuidance(steps, 410)?.distanceM).toBe(0);
  });

  it.each([null, NaN, Infinity, -1])("進捗が%sなら距離を作らない", progress => {
    expect(nextGuidance(steps, progress)).toBeNull();
  });

  it("案内データがない場合は架空の右左折を返さない", () => {
    expect(nextGuidance([], 20)).toBeNull();
  });

  it("デモの折れ曲がりと到着を既存の距離軸に合わせる", () => {
    const request: RouteRequest = { origin: { lat: 35.68, lon: 139.76 }, dest: { lat: 35.72, lon: 139.76 }, mode: "walk", period: "all", weather: "all", day_type: "all", max_detour_min: 10 };
    const result = createDemoRouteResponse(request);
    const [fastest, safest] = result.candidates;
    const right = createDemoGuidance(fastest);
    const left = createDemoGuidance(safest);
    expect(right.map(step => step.maneuver)).toEqual(["right", "right", "arrive"]);
    expect(left.map(step => step.maneuver)).toEqual(["left", "left", "arrive"]);
    expect(right.map(step => step.atM)).toEqual(fastest.navigation!.vertex_distances_m.slice(1));
    const straight = structuredClone(fastest);
    straight.geometry.coordinates = [[139.76, 35.68], [139.76, 35.69], [139.76, 35.70], [139.76, 35.72]];
    expect(createDemoGuidance(straight).map(step => step.maneuver)).toEqual(["arrive"]);
    straight.navigation = null;
    expect(createDemoGuidance(straight)).toEqual([]);
  });
});
