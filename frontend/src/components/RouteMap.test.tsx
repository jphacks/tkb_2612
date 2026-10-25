import { fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createDemoRouteResponse, DEMO_PLACES } from "../api/demo";
import { RouteMap, type RouteMapProps } from "./RouteMap";
import type { RouteResponse } from "../lib/types";

const request = {
  origin: DEMO_PLACES[0],
  dest: DEMO_PLACES[1],
  mode: "walk" as const,
  period: "昼" as const,
  weather: "晴" as const,
  day_type: "平日" as const,
  max_detour_min: 10,
};
function mapProps(result: RouteResponse): RouteMapProps {
  return {
    result,
    selectedRouteId: result.safest_id ?? result.fastest_id,
    selectedSectionId: null,
    onSelectRoute: vi.fn(),
    onSelectSection: vi.fn(),
    showAccidents: true,
    unavailable: false,
  };
}

describe("RouteMap", () => {
  it.each([
    [390, 720],
    [390, 400],
    [1048, 720],
  ])(
    "pairs callouts with paths and keeps cards within a %i by %i map",
    (width, height) => {
      const bounds = vi
        .spyOn(HTMLElement.prototype, "getBoundingClientRect")
        .mockReturnValue({
          width,
          height,
          top: 0,
          left: 0,
          bottom: height,
          right: width,
          x: 0,
          y: 0,
          toJSON: () => ({}),
        });
      const props = mapProps(createDemoRouteResponse(request));
      render(<RouteMap {...props} />);
      const group = screen.getByRole("group", { name: "表示する経路" });
      const cards = [...group.querySelectorAll<HTMLElement>(".map-route-card")];
      expect(cards).toHaveLength(2);
      const considered = cards.find((card) =>
        card.textContent?.includes("安全ルート"),
      )!;
      const fastest = cards.find((card) =>
        card.textContent?.includes("最短ルート"),
      )!;
      expect(considered).toHaveClass("route-considered");
      expect(fastest).toHaveClass("route-fastest");
      // Each card offers selection and a navigation start button.
      expect(
        within(fastest).getByRole("button", { name: "最短ルートでナビを開始" }),
      ).toBeInTheDocument();
      const line = screen.getByRole("button", { name: "最短ルートを選択" });
      expect(line).toHaveClass("route-fastest");
      fireEvent.pointerEnter(fastest);
      expect(line).toHaveClass("is-hovered");
      fireEvent.click(
        within(fastest).getByRole("button", {
          name: /^最短ルート/,
          pressed: false,
        }),
      );
      expect(props.onSelectRoute).toHaveBeenCalledWith(
        props.result!.fastest_id,
      );
      for (const card of cards) {
        expect(parseFloat(card.style.left)).toBeGreaterThanOrEqual(0);
        expect(
          parseFloat(card.style.left) + parseFloat(card.style.width),
        ).toBeLessThanOrEqual(width);
        expect(
          parseFloat(card.style.top) + parseFloat(card.style.height),
        ).toBeLessThanOrEqual(height);
      }
      bounds.mockRestore();
    },
  );
  beforeEach(() => {
    vi.stubEnv("VITE_API_MODE", "demo");
  });
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.restoreAllMocks();
  });

  it("connects route and section selection, and only shows factual accident fields", () => {
    const props = mapProps(createDemoRouteResponse(request));
    render(<RouteMap {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "最短ルートを選択" }));
    expect(props.onSelectRoute).toHaveBeenCalledWith(props.result?.fastest_id);
    const selected = props.result!.candidates.find(
      (route) => route.id === props.selectedRouteId,
    )!;
    fireEvent.click(screen.getByRole("button", { name: "注意箇所 1 を選択" }));
    expect(props.onSelectSection).toHaveBeenCalledWith(
      selected.high_risk_sections.features[0].id,
    );
    const accident = selected.accident_points.features[0];
    fireEvent.click(
      screen.getByRole("button", {
        name: `事故記録 ${accident.id + 1} を表示`,
      }),
    );
    expect(
      screen.getByRole("region", { name: "過去の事故記録" }),
    ).toBeVisible();
    expect(screen.getByText("死傷区分")).toBeVisible();
    expect(screen.queryByText(/原因|見通し|交差点/)).not.toBeInTheDocument();
  });

  it("removes accident details when points are hidden and when a new result has no accidents", () => {
    const props = mapProps(createDemoRouteResponse(request));
    const { rerender } = render(<RouteMap {...props} />);
    fireEvent.click(
      screen.getAllByRole("button", { name: /事故記録 .* を表示/ })[0],
    );
    rerender(<RouteMap {...props} showAccidents={false} />);
    expect(
      screen.queryByRole("region", { name: "過去の事故記録" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /事故記録 .* を表示/ }),
    ).not.toBeInTheDocument();
    rerender(
      <RouteMap {...mapProps(createDemoRouteResponse(request, "zero"))} />,
    );
    expect(
      screen.queryByRole("button", { name: /事故記録 .* を表示/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /注意箇所/ }),
    ).not.toBeInTheDocument();
  });

  it("labels coincident recommendations as one route and preserves comparison when the map is unavailable", () => {
    const props = mapProps(createDemoRouteResponse(request, "same"));
    const { rerender } = render(<RouteMap {...props} />);
    expect(
      screen.getAllByRole("button", {
        name: "最短ルート・安全ルート（同一経路）を選択",
      }),
    ).toHaveLength(1);
    rerender(<RouteMap {...props} unavailable />);
    expect(screen.getByText("地図を利用できません")).toBeVisible();
    expect(screen.getByRole("group", { name: "表示する経路" })).toBeVisible();
  });

  it("tells that navigation is not available yet instead of doing nothing", () => {
    const props = mapProps(createDemoRouteResponse(request));
    render(<RouteMap {...props} />);
    fireEvent.click(
      screen.getByRole("button", { name: "安全ルートでナビを開始" }),
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      "ナビの開始は準備中です。",
    );
    expect(props.onSelectRoute).not.toHaveBeenCalled();
  });
});
