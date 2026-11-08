import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { SafetyReport } from "../community/reports";
import { CommunityReview } from "./CommunityReview";

const report: SafetyReport = {
  id: "tooltip",
  kind: "danger",
  lat: 35.6812,
  lon: 139.7671,
  text: "夜は見通しが悪くて危険",
  createdAt: null,
  supportCount: 1,
  comments: [],
};
const rect = (bottom: number, height: number): DOMRect => ({
  x: 0,
  y: bottom - height,
  left: 0,
  right: 320,
  top: bottom - height,
  bottom,
  width: 320,
  height,
  toJSON: () => ({}),
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("community warning placement", () => {
  it.each([
    [260, 640, 768, "bottom"],
    [600, 640, 768, "top"],
    [650, 1000, 700, "top"],
  ] as const)(
    "uses %s/%s/%s available bounds to place the tooltip %s",
    (cardBottom, mapBottom, viewportBottom, expected) => {
      vi.stubGlobal("innerHeight", viewportBottom);
      vi.spyOn(
        HTMLElement.prototype,
        "getBoundingClientRect",
      ).mockImplementation(function (this: HTMLElement) {
        if (this.classList.contains("community-review-shell"))
          return rect(cardBottom, 160);
        if (this.classList.contains("community-vote-notice"))
          return rect(44, 44);
        return rect(mapBottom, mapBottom);
      });
      render(
        <div className="route-map-shell">
          <CommunityReview
            report={report}
            supported
            onSupport={vi.fn()}
            onClose={vi.fn()}
            error=""
          />
        </div>,
      );
      fireEvent.focus(screen.getByRole("button", { name: "投票" }));
      const tooltip = screen.getByRole("tooltip");
      expect(tooltip).toHaveAttribute("data-placement", expected);
      expect(
        screen.getByRole("region", { name: "口コミ" }),
      ).not.toContainElement(tooltip);
    },
  );

  it("recalculates the direction when the available area resizes", () => {
    let mapBottom = 640;
    vi.stubGlobal("innerHeight", 768);
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(
      function (this: HTMLElement) {
        if (this.classList.contains("community-review-shell"))
          return rect(400, 160);
        if (this.classList.contains("community-vote-notice"))
          return rect(44, 44);
        return rect(mapBottom, mapBottom);
      },
    );
    render(
      <div className="route-map-shell">
        <CommunityReview
          report={report}
          supported
          onSupport={vi.fn()}
          onClose={vi.fn()}
          error=""
        />
      </div>,
    );
    fireEvent.focus(screen.getByRole("button", { name: "投票" }));
    expect(screen.getByRole("tooltip")).toHaveAttribute(
      "data-placement",
      "bottom",
    );
    mapBottom = 430;
    fireEvent(window, new Event("resize"));
    expect(screen.getByRole("tooltip")).toHaveAttribute(
      "data-placement",
      "top",
    );
    mapBottom = 640;
    fireEvent(window, new Event("resize"));
    expect(screen.getByRole("tooltip")).toHaveAttribute(
      "data-placement",
      "bottom",
    );
  });
});
