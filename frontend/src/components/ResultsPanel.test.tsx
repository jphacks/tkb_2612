import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { createDemoRouteResponse, DEMO_PLACES } from "../api/demo";
import type { RouteRequest, RouteResponse } from "../lib/types";
import {
  ResultsPanel,
  type ResultsPanelProps,
  type SearchSnapshot,
} from "./ResultsPanel";

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

const request: RouteRequest = {
  origin: DEMO_PLACES[0],
  dest: DEMO_PLACES[1],
  mode: "walk",
  period: "昼",
  day_type: "平日",
  weather: "雨",
  max_detour_min: 10,
};
const snapshot: SearchSnapshot = {
  departure: "now",
  dateTime: "2026-10-25T14:03",
  auto: { period: true, day_type: false, weather: true },
};

function renderResult(
  result: RouteResponse,
  overrides: Partial<ResultsPanelProps> = {},
) {
  const props: ResultsPanelProps = {
    result,
    snapshot,
    selectedRouteId: result.safest_id ?? result.fastest_id,
    onSelectRoute: vi.fn(),
    selectedSectionId: null,
    onSelectSection: vi.fn(),
    showAccidents: true,
    onShowAccidents: vi.fn(),
    isDemo: false,
    ...overrides,
  };
  const view = render(<ResultsPanel {...props} />);
  return { ...view, props };
}

const metricRow = (label: string) =>
  within(screen.getByRole("region", { name: "2経路の指標（横スクロール）" }))
    .getByRole("rowheader", { name: label })
    .closest("tr")!;

describe("ResultsPanel", () => {
  it("shows both selection meanings, reason, warnings, risk note and spec units for two routes", () => {
    const result = example("twoSelected");
    renderResult(result);
    expect(screen.getAllByText("最短時間経路").length).toBeGreaterThan(0);
    expect(
      screen.getAllByText("事故考慮経路（取得候補内）").length,
    ).toBeGreaterThan(0);
    expect(
      screen.getByText(/全道路網の中から選んだ最適な経路ではありません/),
    ).toBeInTheDocument();
    expect(screen.getByText(result.reason)).toBeInTheDocument();
    expect(screen.getByText(result.risk_note)).toBeInTheDocument();
    // duration: minutes with one decimal, distance: km with two decimals.
    expect(
      within(metricRow("所要時間")).getByText("10.0分"),
    ).toBeInTheDocument();
    expect(
      within(metricRow("所要時間")).getByText("12.5分"),
    ).toBeInTheDocument();
    expect(within(metricRow("距離")).getByText("1.10 km")).toBeInTheDocument();
    expect(
      within(metricRow("最短比の追加時間")).getByText("+2.5分"),
    ).toBeInTheDocument();
    expect(screen.getByText("70.2%")).toBeInTheDocument();
    expect(screen.getByText("+2.5分", { selector: "dd" })).toBeInTheDocument();
  });

  it("states that identical selections are one route and keeps both meanings (single candidate)", () => {
    const result = example("singleRoute");
    renderResult(result);
    const group = screen.getByRole("group", { name: "比較結果の経路" });
    const cards = within(group).getAllByRole("button");
    expect(cards).toHaveLength(1);
    expect(cards[0]).toHaveTextContent(
      "最短時間経路・事故考慮経路（取得候補内）",
    );
    expect(
      screen.getByText(
        "今回は最短時間経路と事故考慮経路（取得候補内）が同じ経路です。",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText("経路候補が1本だけのため、他の経路と比較できません。"),
    ).toBeInTheDocument();
    for (const warning of result.warnings)
      expect(screen.getByText(warning)).toBeInTheDocument();
  });

  it("keeps safest=null explicit: no considered route, null reduction and extra time, zero is not safety", () => {
    const result = example("noAccidents");
    renderResult(result);
    expect(
      within(
        screen.getByRole("group", { name: "比較結果の経路" }),
      ).getAllByRole("button"),
    ).toHaveLength(1);
    expect(
      screen.queryByRole("columnheader", {
        name: "事故考慮経路（取得候補内）",
      }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByText("算出できません（事故考慮経路がないため）"),
    ).toBeInTheDocument();
    expect(screen.getByText("対象なし")).toBeInTheDocument();
    expect(
      screen.getByText(/事故0件は安全の証明ではありません/),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/安全であることを示すものではありません/),
    ).toBeInTheDocument();
    expect(screen.queryByText(/^0(\.0)?%$/)).not.toBeInTheDocument();
  });

  it("distinguishes zero weather matches from weather=all (weather_match=null)", () => {
    const matched = example("noAccidents");
    const { unmount } = renderResult(matched);
    expect(
      within(metricRow("指定天候と一致する事故")).getByText("0件"),
    ).toBeInTheDocument();
    unmount();

    const all = example("twoSelected");
    all.effective_conditions.weather = "全天候";
    for (const route of all.candidates) route.weather_match = null;
    renderResult(all);
    expect(
      within(metricRow("指定天候と一致する事故")).getAllByText("天候指定なし"),
    ).toHaveLength(2);
    expect(screen.getByText("全天候（自動）")).toBeInTheDocument();
  });

  it("explains thinned accident points and truncated high-risk sections per selected route", () => {
    const result = example("twoSelected");
    const safest = result.candidates.find((r) => r.id === result.safest_id)!;
    safest.accident_count = 400;
    safest.points_shown = 250;
    safest.high_risk_total = 120;
    renderResult(result);
    expect(
      within(metricRow("地図の事故点")).getByText("250 / 400件"),
    ).toBeInTheDocument();
    expect(
      within(metricRow("注意区間")).getByText("1 / 120区間"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/死亡事故を優先して最大250件まで表示/),
    ).toBeInTheDocument();
    expect(screen.getByText(/上位100区間だけを表示/)).toBeInTheDocument();
  });

  it("does not show a percentage when the baseline risk is zero", () => {
    const result = example("twoSelected");
    result.candidates[0].weighted_risk = 0;
    result.risk_reduction_percent = null;
    renderResult(result);
    expect(
      screen.getByText("算出できません（最短時間経路の基準リスクが0のため）"),
    ).toBeInTheDocument();
  });

  it("shows the requested departure time and whether each condition was automatic or manual", () => {
    const result = example("twoSelected");
    renderResult(result);
    const applied = screen.getByRole("region", { name: "適用された条件" });
    expect(
      within(applied).getByText("現在（2026/10/25 14:03の検索時点）"),
    ).toBeInTheDocument();
    const { period, day_type, weather } = result.effective_conditions;
    expect(within(applied).getByText(`${period}（自動）`)).toBeInTheDocument();
    expect(
      within(applied).getByText(`${day_type}（手動）`),
    ).toBeInTheDocument();
    expect(within(applied).getByText(`${weather}（自動）`)).toBeInTheDocument();
    expect(
      within(applied).getByText(/雨・霧・雪を指定した場合だけ/),
    ).toBeInTheDocument();
    expect(within(applied).getByText(/気象モデルの推定値/)).toBeInTheDocument();
  });

  it("labels a custom departure and keeps aggregation_ms apart from total communication time", () => {
    const result = example("twoSelected");
    result.aggregation_ms = 12.34;
    renderResult(result, {
      snapshot: {
        ...snapshot,
        departure: "custom",
        dateTime: "2026-11-03T08:30",
      },
    });
    expect(screen.getByText("指定：2026/11/3 08:30")).toBeInTheDocument();
    expect(
      screen.getByText("サーバーでの集計・選定時間（通信時間を含みません）"),
    ).toBeInTheDocument();
    expect(screen.getByText("12ms")).toBeInTheDocument();
    expect(screen.getByText("72m")).toBeInTheDocument();
    expect(screen.getByText("24m")).toBeInTheDocument();
  });

  it("connects route, section, and accident-display controls to their response IDs", () => {
    const result = createDemoRouteResponse(request);
    const { props } = renderResult(result, { isDemo: true });
    const group = screen.getByRole("group", { name: "比較結果の経路" });
    fireEvent.click(
      within(group).getByRole("button", { name: /^最短時間経路/ }),
    );
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

  it("selects an alternate candidate from the comparison table", () => {
    const result = createDemoRouteResponse(request);
    const { props } = renderResult(result);
    fireEvent.click(
      screen.getByRole("button", { name: "候補1", hidden: true }),
    );
    expect(props.onSelectRoute).toHaveBeenCalledWith(result.candidates[0].id);
  });
});
