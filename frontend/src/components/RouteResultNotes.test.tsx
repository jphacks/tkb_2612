import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { createDemoRouteResponse, DEMO_PLACES } from "../api/demo";
import type { RouteRequest, RouteResponse } from "../lib/types";
import {
  FASTEST_LABEL,
  MIDDLE_LABEL,
  SAFE_LABEL,
} from "./RouteCallouts";
import { RouteResultNotes, relativeRisk } from "./RouteResultNotes";

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
        new RegExp(
          `${SAFE_LABEL}の経路は、過去事故の件数が${FASTEST_LABEL}の経路より多くなっています。`,
        ),
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
        within(section).queryByText(
          new RegExp(`過去事故の件数が${FASTEST_LABEL}の経路より`),
        ),
      ).not.toBeInTheDocument();
    },
  );

  it("explains that no safest route was selected when safest_id is null", () => {
    const result = createDemoRouteResponse(request, "zero");
    expect(result.safest_id).toBeNull();
    const section = renderNotes(result);
    expect(
      within(section).getByText(
        `全候補で該当する事故がないため、${SAFE_LABEL}の経路は選んでいません。`,
      ),
    ).toBeInTheDocument();
    expect(
      within(section).queryByText(
        new RegExp(`過去事故の件数が${FASTEST_LABEL}の経路より`),
      ),
    ).not.toBeInTheDocument();
  });

  function routes(result: RouteResponse) {
    return {
      safest: result.candidates.find((route) => route.id === result.safest_id)!,
      fastest: result.candidates.find(
        (route) => route.id === result.fastest_id,
      )!,
    };
  }

  it("states the extra time and the risk reduction when the routes differ", () => {
    const result = createDemoRouteResponse(request);
    result.extra_minutes = 4.4;
    result.risk_reduction_percent = 77.23;
    const section = renderNotes(result);
    const details = section.querySelector("details")!;
    expect(
      within(details).getByText(
        `${SAFE_LABEL}の経路は${FASTEST_LABEL}の経路より4分長くかかりますが、推定リスクは77.2%低くなります。`,
      ),
    ).toBeInTheDocument();
  });

  it("says the fastest route is also the lowest risk when both are the same", () => {
    const result = createDemoRouteResponse(request, "same");
    expect(result.safest_id).toBe(result.fastest_id);
    const section = renderNotes(result);
    expect(
      within(section).getByText(
        new RegExp(`${FASTEST_LABEL}の経路が推定リスクもいちばん低い経路です`),
      ),
    ).toBeInTheDocument();
    const rows = within(section).getAllByRole("row");
    expect(rows).toHaveLength(2);
    expect(rows[1]).toHaveTextContent(`${FASTEST_LABEL}・${SAFE_LABEL}`);
  });

  it("lists the middle route in time order in the comparison table", () => {
    const result = createDemoRouteResponse(request, "middle");
    const section = renderNotes(result);
    const rows = within(section).getAllByRole("row").slice(1);
    expect(
      rows.map((row) => within(row).getByRole("rowheader").textContent),
    ).toEqual([FASTEST_LABEL, MIDDLE_LABEL, SAFE_LABEL]);
  });

  it("shows the estimated risk relative to the fastest route (=100) and the total accident count per route", () => {
    const result = createDemoRouteResponse(request);
    const { safest, fastest } = routes(result);
    fastest.weighted_risk = 123.98;
    fastest.accident_count = 3;
    fastest.total_accident_count = 56;
    safest.weighted_risk = 28.01;
    safest.accident_count = 1;
    safest.total_accident_count = 12;
    const section = renderNotes(result);
    const table = within(section).getByRole("table");
    const [fastestRow, safestRow] = within(table)
      .getAllByRole("row")
      .slice(1);
    expect(within(fastestRow).getByRole("rowheader")).toHaveTextContent(
      FASTEST_LABEL,
    );
    expect(within(fastestRow).getAllByRole("cell").map((c) => c.textContent))
      .toEqual(["100", "56件"]);
    expect(within(safestRow).getByRole("rowheader")).toHaveTextContent(
      SAFE_LABEL,
    );
    expect(within(safestRow).getAllByRole("cell").map((c) => c.textContent))
      .toEqual(["22.6", "12件"]);
  });

  it.each([
    [123.98, 28.01, "22.6"],
    [50, 50, "100.0"],
    [10, 0.144, "1.4"],
    [10, 0, "0.0"],
  ])(
    "shows risk %d → %d as %s relative to the fastest route",
    (base, value, expected) => {
      const result = createDemoRouteResponse(request);
      const { safest, fastest } = routes(result);
      fastest.weighted_risk = base;
      safest.weighted_risk = value;
      expect(relativeRisk(fastest, fastest)).toBe("100");
      expect(relativeRisk(safest, fastest)).toBe(expected);
    },
  );

  it("shows a dash for the relative risk when the fastest route's risk is zero", () => {
    const result = createDemoRouteResponse(request);
    const { safest, fastest } = routes(result);
    fastest.weighted_risk = 0;
    safest.weighted_risk = 0;
    expect(relativeRisk(safest, fastest)).toBeNull();
    const section = renderNotes(result);
    const [fastestRow, safestRow] = within(section)
      .getAllByRole("row")
      .slice(1);
    expect(within(fastestRow).getAllByRole("cell")[0]).toHaveTextContent("100");
    expect(within(safestRow).getAllByRole("cell")[0]).toHaveTextContent("—");
  });

  it("shows a dash when the total accident count is not available", () => {
    const result = createDemoRouteResponse(request);
    routes(result).fastest.total_accident_count = null;
    const section = renderNotes(result);
    const fastestRow = within(section).getAllByRole("row")[1];
    expect(within(fastestRow).getAllByRole("cell")[1]).toHaveTextContent("—");
  });

  it("explains how the risk is estimated, adding the car and weather lines only when they apply", () => {
    const result = createDemoRouteResponse(request);
    result.effective_conditions = {
      ...result.effective_conditions,
      mode: "walk",
      weather: "晴",
    };
    const { unmount } = render(<RouteResultNotes result={result} />);
    let section = screen.getByRole("region", { name: "検索結果の説明" });
    expect(section).toHaveTextContent("過去の事故を、起きた道路や交差点ごとに数えています。");
    expect(section).not.toHaveTextContent("事故に遭う確率");
    expect(section).not.toHaveTextContent("絞っていません");
    expect(section).not.toHaveTextContent("生活道路");
    expect(section).not.toHaveTextContent("雨・霧・雪を指定したとき");
    unmount();

    result.effective_conditions = {
      ...result.effective_conditions,
      mode: "car",
      weather: "雪",
    };
    section = renderNotes(result);
    expect(section).toHaveTextContent("生活道路は交通量が少ないぶん");
    expect(section).toHaveTextContent("雨・霧・雪を指定したときは");
  });

  it("does not show the developer-facing reason, risk note or risk per km", () => {
    const result = createDemoRouteResponse(request);
    for (const candidate of result.candidates) candidate.risk_per_km = 456.789;
    const section = renderNotes(result);
    expect(within(section).queryByText(result.reason)).not.toBeInTheDocument();
    expect(within(section).queryByText(result.risk_note)).not.toBeInTheDocument();
    expect(section).not.toHaveTextContent("456");
  });

  it("renders warnings as text even when they contain HTML", () => {
    const result = createDemoRouteResponse(request);
    result.warnings = ["警告 <b>x</b>"];
    const section = renderNotes(result);
    expect(within(section).getByText("警告 <b>x</b>")).toBeInTheDocument();
    expect(section.querySelector("b")).toBeNull();
  });
});
