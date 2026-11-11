import { fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { SafetyReport } from "../community/reports";
import { CommunityReview } from "./CommunityReview";

const report: SafetyReport = {
  id: "0123456789abcdef0123456789abcdef",
  lat: 35.6812,
  lon: 139.7671,
  created_at: "2026-11-04T09:15:00.000Z",
  safe_count: 1,
  danger_count: 2,
  my_vote: "danger",
  comments: [
    {
      id: "c1",
      kind: "danger",
      text: "夜は見通しが悪くて危険",
      created_at: "2026-11-04T09:15:00.000Z",
    },
    {
      id: "c2",
      kind: "safe",
      text: "昼は人通りが多い",
      created_at: "2026-11-05T03:00:00.000Z",
    },
  ],
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
            onVote={vi.fn()}
            onClose={vi.fn()}
            error=""
          />
        </div>,
      );
      fireEvent.focus(screen.getByRole("button", { name: "安全に投票" }));
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
          onVote={vi.fn()}
          onClose={vi.fn()}
          error=""
        />
      </div>,
    );
    fireEvent.focus(screen.getByRole("button", { name: "安全に投票" }));
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

describe("community review", () => {
  it("shows the safe and danger tallies and each comment's kind", () => {
    render(<CommunityReview report={report} onVote={vi.fn()} onClose={vi.fn()} error="" />);
    const region = screen.getByRole("region", { name: "口コミ" });
    expect(within(region).getByLabelText("安全1票・危険2票")).toHaveTextContent(
      "安全1票危険2票",
    );
    const items = within(region).getAllByRole("listitem");
    expect(items[0]).toHaveTextContent("危険");
    expect(items[0]).toHaveTextContent("夜は見通しが悪くて危険");
    expect(items[1]).toHaveTextContent("安全");
    expect(items[1]).toHaveTextContent("昼は人通りが多い");
  });

  it("offers a safe or danger vote until this browser has voted", () => {
    const onVote = vi.fn();
    const { rerender } = render(
      <CommunityReview
        report={{ ...report, my_vote: null }}
        onVote={onVote}
        onClose={vi.fn()}
        error=""
      />,
    );
    const group = screen.getByRole("group", { name: "この地点に投票" });
    fireEvent.click(within(group).getByRole("button", { name: "危険に投票" }));
    expect(onVote).toHaveBeenCalledWith("danger");
    rerender(<CommunityReview report={report} onVote={onVote} onClose={vi.fn()} error="" />);
    for (const button of within(group).getAllByRole("button"))
      expect(button).toHaveAttribute("aria-disabled", "true");
    expect(within(group).getByRole("button", { name: "危険に投票" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    fireEvent.click(within(group).getByRole("button", { name: "安全に投票" }));
    expect(onVote).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("tooltip")).toHaveTextContent(
      "同一地点には一度しか入力できません",
    );
  });
});