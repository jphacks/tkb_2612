import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { RouteResponse } from "../lib/types";
import { ResultsPanel, type ResultsPanelProps } from "./ResultsPanel";

// OpenAPI 200 examples (= backend/tests/fixtures/route-responses.json, C0-a).
// Tests run from frontend/ (npm test), as for the C0-c commands.
const examples = JSON.parse(
  readFileSync(
    resolve(process.cwd(), "../backend/tests/fixtures/route-responses.json"),
    "utf8",
  ),
).examples as Record<string, RouteResponse>;
const example = (name: string): RouteResponse =>
  structuredClone(examples[name]);

function renderResult(
  result: RouteResponse,
  overrides: Partial<ResultsPanelProps> = {},
) {
  const props: ResultsPanelProps = {
    result,
    selectedRouteId: result.safest_id ?? result.fastest_id,
    onSelectRoute: vi.fn(),
    ...overrides,
  };
  const view = render(<ResultsPanel {...props} />);
  return { ...view, props };
}

// The table sits in a collapsed <details>, so queries include hidden elements.
const table = () =>
  within(
    screen.getByRole("region", {
      name: "全候補の比較表（横スクロール）",
      hidden: true,
    }),
  );
const row = (label: string) =>
  table().getByRole("button", { name: label, hidden: true }).closest("tr")!;

describe("ResultsPanel", () => {
  it("lists every candidate with selection marks and spec units", () => {
    const result = example("twoSelected");
    renderResult(result);
    expect(table().getAllByRole("row", { hidden: true })).toHaveLength(
      result.candidates.length + 1,
    );
    const fastest = row("候補1");
    expect(within(fastest).getByText("最短")).toBeInTheDocument();
    expect(within(fastest).getByText("10.0分")).toBeInTheDocument();
    expect(within(fastest).getByText("1.00 km")).toBeInTheDocument();
    const safest = row("候補2");
    expect(within(safest).getByText("事故考慮")).toBeInTheDocument();
    expect(within(safest).getByText("+2.5分")).toBeInTheDocument();
    expect(within(row("候補3")).getByText("—")).toBeInTheDocument();
  });

  it("marks one candidate as both selections when they are the same route", () => {
    renderResult(example("singleRoute"));
    expect(
      within(row("候補1")).getByText("最短 / 事故考慮"),
    ).toBeInTheDocument();
  });

  it("distinguishes zero weather matches from weather=all (weather_match=null)", () => {
    const { unmount } = renderResult(example("noAccidents"));
    expect(within(row("候補1")).getAllByText("0件")).toHaveLength(2);
    unmount();

    const all = example("twoSelected");
    for (const route of all.candidates) route.weather_match = null;
    renderResult(all);
    expect(within(row("候補1")).getByText("指定なし")).toBeInTheDocument();
  });

  it("shows aggregation settings and keeps aggregation_ms apart from communication time", () => {
    const result = example("twoSelected");
    result.aggregation_ms = 12.34;
    renderResult(result);
    expect(
      screen.getByText("サーバーでの集計・選定時間（通信時間を含みません）"),
    ).toBeInTheDocument();
    expect(screen.getByText("12ms")).toBeInTheDocument();
    expect(screen.getByText("72m")).toBeInTheDocument();
    expect(screen.getByText("24m")).toBeInTheDocument();
    expect(screen.getByText("3本")).toBeInTheDocument();
  });

  it("selects a candidate from the comparison table", () => {
    const result = example("twoSelected");
    const { props } = renderResult(result);
    expect(
      table().getByRole("button", { name: "候補2", hidden: true }),
    ).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(
      table().getByRole("button", { name: "候補1", hidden: true }),
    );
    expect(props.onSelectRoute).toHaveBeenCalledWith(result.candidates[0].id);
  });
});
