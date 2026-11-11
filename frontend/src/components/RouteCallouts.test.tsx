import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { createDemoRouteResponse, DEMO_PLACES } from "../api/demo";
import type { RouteRequest, RouteResponse } from "../lib/types";
import {
  FASTEST_LABEL,
  MIDDLE_LABEL,
  RouteCallouts,
  SAFE_LABEL,
  featuredRoutes,
  routeSymbol,
} from "./RouteCallouts";

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
  onNavigate = vi.fn(),
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
      onNavigate={onNavigate}
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


describe("RouteCallouts featured routes", () => {
  it("orders fastest, middle, safest and keeps one card per ID", () => {
    const withMiddle = createDemoRouteResponse(request, "middle");
    expect(
      featuredRoutes(withMiddle).map((route) => route.id),
    ).toEqual([
      withMiddle.fastest_id,
      withMiddle.middle_id,
      withMiddle.safest_id,
    ]);
    const withoutMiddle = createDemoRouteResponse(request);
    expect(
      featuredRoutes(withoutMiddle).map((route) => route.id),
    ).toEqual([withoutMiddle.fastest_id, withoutMiddle.safest_id]);
    const same = createDemoRouteResponse(request, "same");
    expect(featuredRoutes(same).map((route) => route.id)).toEqual([
      same.fastest_id,
    ]);
  });

  it("puts A/B/C before the label and shows the extra time in time order", () => {
    const result = createDemoRouteResponse(request, "middle");
    const group = renderCallouts(result);
    const cards = [
      ...group.querySelectorAll<HTMLElement>(".map-route-card"),
    ];
    expect(cards).toHaveLength(3);
    const routes = featuredRoutes(result);
    const extra = routes.map((route) =>
      ((route.duration - routes[0].duration) / 60).toFixed(1),
    );
    const expected = [
      FASTEST_LABEL,
      `${MIDDLE_LABEL}（+${extra[1]}分）`,
      `${SAFE_LABEL}（+${extra[2]}分）`,
    ];
    cards.forEach((card, index) => {
      expect(
        within(card).getByText(routeSymbol(index), {
          selector: ".map-card-symbol",
        }),
      ).toBeInTheDocument();
      expect(
        card.querySelector(".map-card-title > span:last-child")?.textContent,
      ).toBe(expected[index]);
    });
    expect(cards[0]).toHaveClass("route-fastest");
    expect(cards[1]).toHaveClass("route-middle");
    expect(cards[2]).toHaveClass("route-considered");
  });

  it("uses each candidate's reduction and shows an increase for negative values", () => {
    const result = createDemoRouteResponse(request, "middle");
    const middle = result.candidates.find(
      (route) => route.id === result.middle_id,
    )!;
    const safest = result.candidates.find(
      (route) => route.id === result.safest_id,
    )!;
    middle.risk_reduction_percent = 12.34;
    safest.risk_reduction_percent = -5.67;
    result.risk_reduction_percent = 99.9;
    const group = renderCallouts(result);
    const cards = [
      ...group.querySelectorAll<HTMLElement>(".map-route-card"),
    ];
    expect(within(cards[0]).queryByText(/推定リスク/)).not.toBeInTheDocument();
    expect(
      within(cards[1]).getByText("推定リスク 12.3%減"),
    ).toBeInTheDocument();
    expect(
      within(cards[2]).getByText("推定リスク 5.7%増"),
    ).toBeInTheDocument();
  });

  it("does not show a reduction for a null candidate value", () => {
    const result = createDemoRouteResponse(request, "middle");
    const middle = result.candidates.find(
      (route) => route.id === result.middle_id,
    )!;
    middle.risk_reduction_percent = null;
    const group = renderCallouts(result);
    const middleCard = group.querySelector(
      ".route-middle",
    ) as HTMLElement;
    expect(
      within(middleCard).queryByText(/推定リスク/),
    ).not.toBeInTheDocument();
  });

  it("uses a vertical three-row list on a compact map and starts from each row", () => {
    const result = createDemoRouteResponse(request, "middle");
    const onNavigate = vi.fn();
    const group = renderCallouts(
      result,
      { width: 360, height: 600 },
      {},
      onNavigate,
    );
    expect(group).toHaveClass("is-list");
    expect(group.querySelector(".map-callout-connectors")).toBeNull();
    const rows = [
      ...group.querySelectorAll<HTMLElement>(".map-route-list-row"),
    ];
    expect(rows).toHaveLength(3);
    rows.forEach((row, index) => {
      expect(
        within(row).getByText(routeSymbol(index), {
          selector: ".map-card-symbol",
        }),
      ).toBeInTheDocument();
      expect(
        within(row).getByRole("button", { name: /でナビを開始$/ }),
      ).toBeInTheDocument();
      expect(
        within(row).getByText(/\d+\.\d分/, {
          selector: ".map-card-duration",
        }),
      ).toBeInTheDocument();
      expect(within(row).getByText(/過去事故/)).toBeInTheDocument();
      if (index > 0)
        expect(within(row).getByText(/%減/)).toBeInTheDocument();
    });
    fireEvent.click(
      within(rows[1]).getByRole("button", {
        name: `${MIDDLE_LABEL}でナビを開始`,
      }),
    );
    expect(onNavigate).toHaveBeenCalledWith(result.middle_id);
  });

  it("marks the two-row list parts on each row", () => {
    const result = createDemoRouteResponse(request, "middle");
    const group = renderCallouts(result, { width: 390, height: 844 });
    const rows = [
      ...group.querySelectorAll<HTMLElement>(".map-route-list-row"),
    ];
    expect(rows).toHaveLength(3);
    rows.forEach((row) => {
      const select = row.querySelector<HTMLElement>(".map-card-select");
      expect(select).not.toBeNull();
      expect(
        select!.querySelector(".map-route-list-title"),
      ).not.toBeNull();
      expect(
        select!.querySelector(".map-route-list-duration"),
      ).not.toBeNull();
      expect(
        select!.querySelector(".map-route-list-accidents"),
      ).not.toBeNull();
      expect(select!.querySelector(".map-route-list-risk")).not.toBeNull();
    });
  });

  it("renders the extra time as its own element in the card heading", () => {
    const result = createDemoRouteResponse(request, "middle");
    const group = renderCallouts(result);
    const middleCard = group.querySelector(".route-middle") as HTMLElement;
    const title = middleCard.querySelector(".map-card-title") as HTMLElement;
    const name = title.querySelector(".map-card-title-name") as HTMLElement;
    const extra = title.querySelector(".map-card-title-extra") as HTMLElement;
    expect(name).not.toBeNull();
    expect(extra).not.toBeNull();
    expect(name).toHaveTextContent(MIDDLE_LABEL);
    expect(extra).toHaveTextContent(/^（\+\d+\.\d分）$/);
    expect(name).not.toBe(extra);
    expect(title).toHaveTextContent(`${MIDDLE_LABEL}${extra.textContent}`);
  });

  it("stacks three cards without overlap on a wide map", () => {
    const result = createDemoRouteResponse(request, "middle");
    const size = { width: 800, height: 700 };
    const anchor = { x: size.width / 2, y: size.height / 2 };
    const group = renderCallouts(result, size, {
      [result.fastest_id]: anchor,
      [result.middle_id!]: anchor,
      [result.safest_id!]: anchor,
    });
    expect(group).not.toHaveClass("is-list");
    const boxes = [
      ...group.querySelectorAll<HTMLElement>(".map-route-card"),
    ].map((card) => {
      const left = parseFloat(card.style.left);
      const top = parseFloat(card.style.top);
      return {
        left,
        top,
        right: left + parseFloat(card.style.width),
        bottom: top + parseFloat(card.style.height),
      };
    });
    expect(boxes).toHaveLength(3);
    for (const box of boxes) {
      expect(box.left).toBeGreaterThanOrEqual(0);
      expect(box.top).toBeGreaterThanOrEqual(0);
      expect(box.right).toBeLessThanOrEqual(size.width);
      expect(box.bottom).toBeLessThanOrEqual(size.height);
    }
    for (let first = 0; first < boxes.length; first++)
      for (let second = first + 1; second < boxes.length; second++) {
        const a = boxes[first];
        const b = boxes[second];
        expect(a.right <= b.left || b.right <= a.left || a.bottom <= b.top || b.bottom <= a.top).toBe(true);
      }
  });
});

describe("RouteCallouts on a low map", () => {
  function cardBoxes(
    size: { width: number; height: number },
    anchor: { x: number; y: number },
  ) {
    const result = createDemoRouteResponse(request);
    const group = renderCallouts(result, size, {
      [result.fastest_id]: anchor,
      [result.safest_id!]: anchor,
    });
    const cards = [...group.querySelectorAll<HTMLElement>(".map-route-card")];
    expect(cards).toHaveLength(2);
    return {
      group,
      boxes: cards.map((card) => {
        const left = parseFloat(card.style.left);
        const top = parseFloat(card.style.top);
        return {
          left,
          top,
          right: left + parseFloat(card.style.width),
          bottom: top + parseFloat(card.style.height),
        };
      }),
    };
  }

  it.each([
    [{ width: 360, height: 262 }],
    [{ width: 360, height: 398 }],
    [{ width: 740, height: 207 }],
  ])("keeps both cards inside a %j map, side by side", (size) => {
    const { group, boxes } = cardBoxes(size, {
      x: size.width / 2,
      y: size.height / 2,
    });
    expect(group).toHaveClass("is-tight");
    for (const box of boxes) {
      expect(box.left).toBeGreaterThanOrEqual(0);
      expect(box.top).toBeGreaterThanOrEqual(0);
      expect(box.right).toBeLessThanOrEqual(size.width);
      expect(box.bottom).toBeLessThanOrEqual(size.height);
    }
    expect(boxes[0].right).toBeLessThanOrEqual(boxes[1].left);
  });

  it("does not use the tight layout on a tall map", () => {
    const size = { width: 360, height: 676 };
    const { group } = cardBoxes(size, { x: 180, y: 338 });
    expect(group).not.toHaveClass("is-tight");
  });
});
