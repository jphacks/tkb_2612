import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { createDemoRouteResponse, DEMO_PLACES } from "../api/demo";
import type { RouteRequest, RouteResponse } from "../lib/types";
import { RouteCallouts } from "./RouteCallouts";

const request: RouteRequest = {
  origin: DEMO_PLACES[0],
  dest: DEMO_PLACES[1],
  mode: "walk",
  period: "昼",
  weather: "晴",
  day_type: "平日",
  max_detour_min: 10,
};

function renderCallouts(result: RouteResponse) {
  render(
    <RouteCallouts
      result={result}
      positions={{}}
      size={{ width: 800, height: 600 }}
      selectedRouteId={null}
      hoveredRouteId={null}
      onSelectRoute={vi.fn()}
      onHoverRoute={vi.fn()}
      onNavigate={vi.fn()}
    />,
  );
  return screen.getByRole("group", { name: "表示する経路" });
}

describe("RouteCallouts risk reduction", () => {
  it("shows the reduction only on the safest card when the two routes differ", () => {
    const result = createDemoRouteResponse(request);
    expect(result.safest_id).not.toBe(result.fastest_id);
    expect(result.risk_reduction_percent).not.toBeNull();
    const group = renderCallouts(result);
    const considered = group.querySelector(".route-considered") as HTMLElement;
    const fastest = group.querySelector(".route-fastest") as HTMLElement;
    expect(
      within(considered).getByText(
        `推定リスク ${result.risk_reduction_percent!.toFixed(1)}%減`,
      ),
    ).toBeInTheDocument();
    expect(within(fastest).queryByText(/推定リスク/)).not.toBeInTheDocument();
  });

  it.each(["same", "zero"] as const)(
    "does not show the reduction for the %s scenario",
    (scenario) => {
      const result = createDemoRouteResponse(request, scenario);
      const group = renderCallouts(result);
      expect(within(group).queryByText(/推定リスク/)).not.toBeInTheDocument();
    },
  );
});
