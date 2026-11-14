import { describe, expect, it } from "vitest";
import type { RouteRequest } from "../lib/types";
import {
  autoDetourLimitMin,
  createDemoRouteResponse,
  DEMO_PLACES,
  findDemoPlaces,
} from "./demo";

const request: RouteRequest = {
  origin: DEMO_PLACES[0],
  dest: DEMO_PLACES[1],
  mode: "walk",
  period: "昼",
  weather: "晴",
  day_type: "平日",
};

describe("findDemoPlaces", () => {
  it("matches addresses as well as labels and returns the two Chiyoda stations", () => {
    const results = findDemoPlaces("千代田区").results;
    expect(results.map((place) => place.label)).toEqual(["東京駅", "秋葉原駅"]);
  });
});

describe("デモの許容時間", () => {
  it.each([
    [300, 3],
    [1200, 6],
    [2700, 13.5],
    [7200, 30],
    [1800, 9],
    [619, 3],
    [620, 3.1],
    [639, 3.1],
    [640, 3.2],
  ])("T0=%i秒なら%f分（0.1分単位の切り捨て）", (seconds, expected) => {
    expect(autoDetourLimitMin(seconds)).toBe(expected);
  });

  it("max_detour_min を省略したら自動の値を応答に返す", () => {
    const result = createDemoRouteResponse(request);
    const fastest = result.candidates[0];
    expect(result.detour_limit_auto).toBe(true);
    expect(result.detour_limit_min).toBe(
      autoDetourLimitMin(fastest.duration),
    );
    expect(result.detour_limit_min).toBeGreaterThanOrEqual(3);
    expect(result.detour_limit_min).toBeLessThanOrEqual(30);
    for (const route of result.candidates)
      expect(route.within_detour).toBe(
        route.duration <=
          fastest.duration + result.detour_limit_min * 60 + 1e-6,
      );
  });

  it("手動では要求の値をそのまま応答に返す", () => {
    const result = createDemoRouteResponse({
      ...request,
      max_detour_min: 7.5,
    });
    expect(result.detour_limit_auto).toBe(false);
    expect(result.detour_limit_min).toBe(7.5);
    expect(result.candidates.map((route) => route.within_detour)).toEqual([
      true,
      true,
    ]);
  });
});
