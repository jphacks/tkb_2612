import { vi } from "vitest";
import {
  createDemoConditions,
  createDemoRouteResponse,
  demoHealth,
  findDemoPlaces,
} from "../api/demo";
import type { LatLon, RouteRequest, Scenario } from "../lib/types";

/** App全体の結合テスト用。demoのデータ生成を使い、通信は呼び出し回数だけを数える。 */
export const api = {
  getHealth: vi.fn(),
  getConditions: vi.fn(),
  searchPlaces: vi.fn(),
  searchRoutes: vi.fn(),
};

export async function clientMock() {
  return {
    ApiError: (await import("../api/errors")).ApiError,
    isDemo: true,
    ...api,
  };
}

export function resetApi() {
  api.getHealth.mockReset().mockResolvedValue(demoHealth);
  api.searchPlaces
    .mockReset()
    .mockImplementation((query: string) => Promise.resolve(findDemoPlaces(query)));
  api.getConditions
    .mockReset()
    .mockImplementation((datetime: string, origin?: LatLon, dest?: LatLon) =>
      Promise.resolve(createDemoConditions(datetime, origin && dest ? origin : undefined)),
    );
  api.searchRoutes
    .mockReset()
    .mockImplementation((request: RouteRequest, _signal?: AbortSignal, scenario?: Scenario) =>
      Promise.resolve(
        createDemoRouteResponse(
          request,
          scenario === "same" || scenario === "zero" ? scenario : "normal",
        ),
      ),
    );
}
