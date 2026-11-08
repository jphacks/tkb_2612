import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { createDemoRouteResponse, DEMO_PLACES } from "../api/demo";
import type { RouteRequest, RouteResponse } from "../lib/types";
import { RouteResultNotes } from "./RouteResultNotes";

const request: RouteRequest = {
  origin: DEMO_PLACES[0],
  dest: DEMO_PLACES[1],
  mode: "walk",
  period: "昼",
  weather: "晴",
  day_type: "平日",
  max_detour_min: 10,
};

function renderNotes(result: RouteResponse) {
  render(<RouteResultNotes result={result} />);
  return screen.getByRole("region", { name: "検索結果の説明" });
}

describe("RouteResultNotes", () => {
  it("shows every warning without opening the details", () => {
    const result = createDemoRouteResponse(request);
    const section = renderNotes(result);
    const details = section.querySelector("details");
    expect(details).not.toBeNull();
    expect(details).not.toHaveAttribute("open");
    for (const warning of result.warnings) {
      const item = within(section).getByText(warning);
      expect(details).not.toContainElement(item);
    }
  });

  it("notes the accident-count reversal only when the safest route has more accidents", () => {
    const result = createDemoRouteResponse(request);
    const safest = result.candidates.find(
      (route) => route.id === result.safest_id,
    )!;
    const fastest = result.candidates.find(
      (route) => route.id === result.fastest_id,
    )!;
    safest.accident_count = 5;
    fastest.accident_count = 3;
    const section = renderNotes(result);
    expect(
      within(section).getByText(
        /安全ルートは、過去事故の件数が最短ルートより多くなっています。/,
      ),
    ).toBeInTheDocument();
  });

  it.each([
    ["同数", 3, 3],
    ["少ない", 1, 3],
  ])(
    "does not note a reversal when the safest count is %s",
    (_label, safestCount, fastestCount) => {
      const result = createDemoRouteResponse(request);
      const safest = result.candidates.find(
        (route) => route.id === result.safest_id,
      )!;
      const fastest = result.candidates.find(
        (route) => route.id === result.fastest_id,
      )!;
      safest.accident_count = safestCount;
      fastest.accident_count = fastestCount;
      const section = renderNotes(result);
      expect(
        within(section).queryByText(/過去事故の件数が最短ルートより/),
      ).not.toBeInTheDocument();
    },
  );

  it("explains that no safest route was selected when safest_id is null", () => {
    const result = createDemoRouteResponse(request, "zero");
    expect(result.safest_id).toBeNull();
    const section = renderNotes(result);
    expect(
      within(section).getByText(
        "全候補で該当する事故がないため、安全ルートは選んでいません。",
      ),
    ).toBeInTheDocument();
    expect(
      within(section).queryByText(/過去事故の件数が最短ルートより/),
    ).not.toBeInTheDocument();
  });

  it("shows an unknown reduction as-is instead of turning null into zero", () => {
    const result = createDemoRouteResponse(request, "zero");
    expect(result.risk_reduction_percent).toBeNull();
    const section = renderNotes(result);
    expect(within(section).getByText("算出できません")).toBeInTheDocument();
    expect(within(section).queryByText("0.0%")).not.toBeInTheDocument();
  });

  it("shows a numeric reduction with one decimal place", () => {
    const result = createDemoRouteResponse(request);
    expect(result.risk_reduction_percent).not.toBeNull();
    const section = renderNotes(result);
    expect(
      within(section).getByText(
        `${result.risk_reduction_percent!.toFixed(1)}%`,
      ),
    ).toBeInTheDocument();
  });

  it("puts reason and risk_note inside the details", () => {
    const result = createDemoRouteResponse(request);
    const section = renderNotes(result);
    const details = section.querySelector("details")!;
    expect(within(details).getByText(result.reason)).toBeInTheDocument();
    expect(within(details).getByText(result.risk_note)).toBeInTheDocument();
  });

  it("renders reason as text even when it contains HTML", () => {
    const result = createDemoRouteResponse(request);
    result.reason = "理由 <b>x</b>";
    const section = renderNotes(result);
    expect(within(section).getByText("理由 <b>x</b>")).toBeInTheDocument();
    expect(section.querySelector("b")).toBeNull();
  });

  it("never shows weighted_risk or risk_per_km values", () => {
    const result = createDemoRouteResponse(request);
    for (const candidate of result.candidates) {
      candidate.weighted_risk = 123.45;
      candidate.risk_per_km = 456.789;
    }
    const section = renderNotes(result);
    expect(section).not.toHaveTextContent("123.45");
    expect(section).not.toHaveTextContent("456.789");
  });
});
