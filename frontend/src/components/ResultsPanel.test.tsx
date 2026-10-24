import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { createDemoRouteResponse, DEMO_PLACES } from "../api/demo";
import type { RouteRequest, RouteResponse } from "../lib/types";
import { ResultsPanel, type ResultsPanelProps } from "./ResultsPanel";

const request: RouteRequest = {
  origin: DEMO_PLACES[0],
  dest: DEMO_PLACES[1],
  mode: "walk",
  period: "昼",
  day_type: "平日",
  weather: "雨",
  max_detour_min: 10,
};

function renderResult(
  result: RouteResponse,
  overrides: Partial<ResultsPanelProps> = {},
) {
  const props: ResultsPanelProps = {
    result,
    selectedRouteId: result.safest_id ?? result.fastest_id,
    onSelectRoute: vi.fn(),
    selectedSectionId: null,
    onSelectSection: vi.fn(),
    showAccidents: true,
    onShowAccidents: vi.fn(),
    isDemo: true,
    ...overrides,
  };
  const view = render(<ResultsPanel {...props} />);
  return { ...view, props };
}

describe("ResultsPanel", () => {
  it("connects route, section, and accident-display controls to their response IDs", () => {
    const result = createDemoRouteResponse(request);
    const { props } = renderResult(result);
    const routeGroup = screen.getByRole("group", { name: "表示する経路" });
    const fastestCard = within(routeGroup).getByRole("button", {
      name: /最短時間/,
    });
    fireEvent.click(fastestCard);
    expect(props.onSelectRoute).toHaveBeenCalledWith(result.fastest_id);

    const considered = result.candidates.find(
      (route) => route.id === result.safest_id,
    )!;
    const section = considered.high_risk_sections.features[0];
    fireEvent.click(
      screen.getByRole("button", {
        name: `区間 ${section.properties.segment_index + 1}`,
      }),
    );
    expect(props.onSelectSection).toHaveBeenCalledWith(section.id);
    fireEvent.click(screen.getByRole("switch", { name: "事故点を地図に表示" }));
    expect(props.onShowAccidents).toHaveBeenCalledWith(false);
  });

  it("shows one selectable route and an unavailable reduction for zero accidents", () => {
    const result = createDemoRouteResponse(request, "zero");
    renderResult(result);
    expect(
      within(screen.getByRole("group", { name: "表示する経路" })).getAllByRole(
        "button",
      ),
    ).toHaveLength(1);
    expect(screen.queryByText("同一経路")).not.toBeInTheDocument();
    expect(
      screen.getByText(/事故履歴から優先経路を判断できません/),
    ).toBeInTheDocument();
    expect(screen.getByText("算出不可")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /^区間 \d+$/ }),
    ).not.toBeInTheDocument();
  });

  it("explicitly identifies identical routes while keeping both comparison labels", () => {
    const result = createDemoRouteResponse(request, "same");
    renderResult(result);
    expect(screen.getByText("同一経路")).toBeInTheDocument();
    const buttons = within(
      screen.getByRole("group", { name: "表示する経路" }),
    ).getAllByRole("button");
    expect(buttons).toHaveLength(2);
    buttons.forEach((button) =>
      expect(button).toHaveAttribute("aria-pressed", "true"),
    );
  });

  it("does not present a percentage when the baseline is zero and exposes section totals", () => {
    const result = createDemoRouteResponse(request);
    result.candidates[0].weighted_risk = 0;
    result.risk_reduction_percent = 100;
    result.candidates[1].high_risk_total = 120;
    renderResult(result);
    expect(screen.getByText("算出不可")).toBeInTheDocument();
    expect(screen.queryByText("100%")).not.toBeInTheDocument();
    expect(screen.getByText("1 / 120区間")).toBeInTheDocument();
  });

  it("selects an alternate candidate from the comparison table", () => {
    const result = createDemoRouteResponse(request);
    const { props } = renderResult(result);
    fireEvent.click(
      screen.getByRole("button", { name: "候補1", hidden: true }),
    );
    expect(props.onSelectRoute).toHaveBeenCalledWith(result.candidates[0].id);
  });
});
