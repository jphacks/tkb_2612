import { describe, expect, it } from "vitest";
import { createDemoRouteResponse } from "../api/demo";
import type { RouteRequest } from "../lib/types";
import { createDemoGuidance } from "./demoGuidance";
import {
  createGuidanceHistory,
  GUIDANCE_BACKTRACK_M,
  nextGuidance,
  updateGuidanceHistory,
  type GuidanceStep,
} from "./guidance";

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

describe("通過済み案内のヒステリシス", () => {
  // 東京駅→上野駅・徒歩・最短経路・揺らぎON・seed 1 の最初の曲がり角と次の右折。
  const turnSteps: GuidanceStep[] = [
    { id: "first-right", maneuver: "right", atM: 1335.434 },
    { id: "second-right", maneuver: "right", atM: 1336.965 + 1233 },
    { id: "arrival", maneuver: "arrive", atM: 3000 },
  ];

  it("曲がり角を越えた直後に進捗が揺れても、通過した右折へ戻らない", () => {
    let history = createGuidanceHistory();
    history = updateGuidanceHistory(turnSteps, 1336.965, history);
    const atCorner = nextGuidance(turnSteps, 1336.965, history);
    expect(atCorner?.id).toBe("second-right");
    expect(atCorner?.distanceM).toBeCloseTo(1233, 6);

    const jittered = nextGuidance(turnSteps, 1330.401, history);
    expect(jittered?.id).toBe("second-right");
    expect(jittered?.distanceM).toBeCloseTo(1239.564, 6);
    // 履歴がなければ、通過した右折まで約5mの案内へ戻ってしまう。
    const withoutHistory = nextGuidance(turnSteps, 1330.401);
    expect(withoutHistory?.id).toBe("first-right");
    expect(withoutHistory?.distanceM).toBeCloseTo(5.033, 6);
  });

  it("揺らぎの幅を超えて引き返すと、通過した案内を戻す", () => {
    let history = updateGuidanceHistory(turnSteps, 1336.965, createGuidanceHistory());
    const withinJitter = 1335.434 - GUIDANCE_BACKTRACK_M + 1;
    expect(nextGuidance(turnSteps, withinJitter, history)?.id).toBe("second-right");

    const backtracked = 1335.434 - GUIDANCE_BACKTRACK_M - 1;
    expect(nextGuidance(turnSteps, backtracked, history)?.id).toBe("first-right");
    expect(nextGuidance(turnSteps, backtracked, history)?.distanceM).toBeCloseTo(21);
    history = updateGuidanceHistory(turnSteps, backtracked, history);
    expect(history.has("first-right")).toBe(false);
    expect(nextGuidance(turnSteps, backtracked, history)?.id).toBe("first-right");
  });

  it("セッションが変わると通過状態を持ち越さない", () => {
    const previousSession = updateGuidanceHistory(turnSteps, 1336.965, createGuidanceHistory());
    expect(nextGuidance(turnSteps, 1330.401, previousSession)?.id).toBe("second-right");

    const nextSession = createGuidanceHistory();
    expect(nextGuidance(turnSteps, 1330.401, nextSession)?.id).toBe("first-right");
  });
});
