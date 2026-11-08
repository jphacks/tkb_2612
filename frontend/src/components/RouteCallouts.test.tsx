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

function renderCallouts(
  result: RouteResponse,
  size = { width: 800, height: 600 },
  positions: Record<string, { x: number; y: number }> = {},
) {
  render(
    <RouteCallouts
      result={result}
      positions={positions}
      size={size}
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

describe("RouteCallouts bottom inset", () => {
  function lowestCardBottom(size: { width: number; height: number }) {
    const result = createDemoRouteResponse(request);
    const low = { x: size.width / 2, y: size.height };
    const group = renderCallouts(result, size, {
      [result.fastest_id]: low,
      [result.safest_id!]: low,
    });
    const cards = [...group.querySelectorAll<HTMLElement>(".map-route-card")];
    expect(cards).toHaveLength(2);
    return Math.max(
      ...cards.map((card) => parseFloat(card.style.top) + parseFloat(card.style.height)),
    );
  }

  it.each([
    [{ width: 1280, height: 728 }, 124],
    [{ width: 700, height: 736 }, 232],
    [{ width: 360, height: 676 }, 232],
    [{ width: 740, height: 296 }, 88],
  ])("keeps cards clear of the notes and legend on a %j map", (size, inset) => {
    expect(lowestCardBottom(size)).toBeLessThanOrEqual(size.height - inset);
  });
});
