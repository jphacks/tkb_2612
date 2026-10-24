import { describe, expect, it } from "vitest";
import {
  createDemoConditions,
  createDemoRouteResponse,
  DEMO_PLACES,
  findDemoPlaces,
} from "./demo";
import {
  validateConditions,
  validatePlaces,
  validateRouteRequest,
  validateRouteResponse,
} from "./validate";
import type { RouteRequest, RouteResponse } from "../lib/types";

const request: RouteRequest = {
  origin: { lat: 35.6812, lon: 139.7671 },
  dest: { lat: 35.7138, lon: 139.7773 },
  mode: "walk",
  period: "昼",
  weather: "雨",
  day_type: "平日",
  max_detour_min: 10,
};

describe("API 2.0 contract and cross-field validation", () => {
  it("validates normal, same-route and zero-accident fixtures", () => {
    for (const scenario of ["normal", "same", "zero"] as const) {
      const result = createDemoRouteResponse(request, scenario);
      expect(validateRouteResponse(result, request)).toBe(result);
    }
  });

  it("does not select a safest route or invent a reduction when all records are zero", () => {
    const result = validateRouteResponse(
      createDemoRouteResponse(request, "zero"),
      request,
    );
    expect(result.safest_id).toBeNull();
    expect(result.extra_minutes).toBeNull();
    expect(result.risk_reduction_percent).toBeNull();
    expect(result.relaxed_conditions).toEqual([]);
    expect(
      result.candidates.every(
        (route) =>
          route.accident_count === 0 &&
          route.accident_points.features.length === 0,
      ),
    ).toBe(true);
  });

  it("obeys the detour limit instead of relaxing conditions", () => {
    const zeroDetour = { ...request, max_detour_min: 0 };
    const result = validateRouteResponse(
      createDemoRouteResponse(zeroDetour),
      zeroDetour,
    );
    expect(result.safest_id).toBe(result.fastest_id);
    expect(result.candidates[1].within_detour).toBe(false);
  });

  it("uses null weather_match for all-weather and a count object for specified weather", () => {
    const all = { ...request, weather: "all" as const };
    expect(
      validateRouteResponse(createDemoRouteResponse(all), all).candidates[0]
        .weather_match,
    ).toBeNull();
    expect(
      validateRouteResponse(createDemoRouteResponse(request), request)
        .candidates[0].weather_match?.accident_count,
    ).toBe(3);
  });

  it.each<[string, (result: RouteResponse) => void]>([
    [
      "unknown route reference",
      (result) => {
        result.safest_id = "route-6";
      },
    ],
    [
      "duplicate route IDs",
      (result) => {
        result.candidates[1].id = "route-1";
      },
    ],
    [
      "mismatched points count",
      (result) => {
        result.candidates[0].points_shown = 0;
      },
    ],
    [
      "mismatched high-risk count",
      (result) => {
        result.candidates[0].high_risk_total = 0;
      },
    ],
    [
      "wrong high-risk route ownership",
      (result) => {
        result.candidates[0].high_risk_sections.features[0].properties.route_id =
          "route-2";
      },
    ],
    [
      "invalid source vertex index",
      (result) => {
        result.candidates[0].high_risk_sections.features[0].properties.source_vertex_end_index = 9;
      },
    ],
    [
      "invented condition relaxation",
      (result) => {
        result.relaxed_conditions.push("weather");
      },
    ],
    [
      "invalid accident split",
      (result) => {
        result.candidates[0].fatal_accidents = 99;
      },
    ],
    [
      "incorrect weather nullability",
      (result) => {
        result.candidates[0].weather_match = null;
      },
    ],
    [
      "wrong coordinate order",
      (result) => {
        result.candidates[0].geometry.coordinates[0] = [35.6, 139.7];
      },
    ],
    [
      "API version drift",
      (result) => {
        Object.assign(result, { api_version: "1.0" });
      },
    ],
  ])("rejects %s", (_, mutate) => {
    const result = createDemoRouteResponse(request);
    mutate(result);
    expect(() => validateRouteResponse(result, request)).toThrow(/API契約/);
  });

  it("requires weather and source to agree with weather_status", () => {
    const result = createDemoConditions(
      "2026-10-24T12:00+09:00",
      DEMO_PLACES[0],
    );
    result.weather_source = null;
    expect(() => validateConditions(result)).toThrow(/API契約/);
    expect(
      validateConditions(createDemoConditions("2026-10-24T12:00+09:00"))
        .weather_status,
    ).toBe("no_location");
  });

  it("keeps geographic search labels and attribution from the contract", () => {
    const result = validatePlaces(findDemoPlaces("つくば"));
    expect(result.results.length).toBeGreaterThan(0);
    expect(result.results.length).toBeLessThanOrEqual(5);
    expect(result.attribution[0].url).toMatch(/^https:\/\//);
  });

  it("rejects identical places and distances beyond the API domain", () => {
    expect(() =>
      validateRouteRequest({ ...request, dest: request.origin }),
    ).toThrow(/20m/);
    expect(() =>
      validateRouteRequest({ ...request, dest: DEMO_PLACES[5] }),
    ).toThrow(/30km/);
  });
});
