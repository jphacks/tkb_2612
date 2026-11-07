import { act, render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { NavigationGuidanceBar } from "./NavigationGuidanceBar";
import type { NavigationAlert } from "./NavigationAlertPopup";

const alert: NavigationAlert = {
  phase: "tracking",
  alertMessage: "この先に、過去事故の参考区間があります。周囲に注意してください",
  alertIds: ["section-1"],
};
const guidance = { id: "turn-1", maneuver: "right" as const, distanceM: 96 };

describe("ナビ上部バー", () => {
  it("次の案内と接近通知を一つのバーに表示し、更新距離をlive領域へ入れない", () => {
    const view = render(<NavigationGuidanceBar alert={alert} guidance={guidance} />);
    const bar = screen.getByRole("region", { name: "ナビ案内" });
    expect(within(bar).getByText("100 m先")).toBeVisible();
    expect(within(bar).getByText("右折")).toBeVisible();
    expect(within(bar).getByLabelText("接近通知")).toHaveTextContent("この先に事故多発区間があります");
    expect(view.container.querySelector(".nav-approach-region")).not.toHaveTextContent("100 m先");
    view.rerender(<NavigationGuidanceBar alert={{ ...alert, alertMessage: null, alertIds: [] }} guidance={{ ...guidance, distanceM: 29 }} />);
    expect(within(bar).getByText("30 m先")).toBeVisible();
    expect(within(bar).queryByLabelText("接近通知")).not.toBeInTheDocument();
  });

  it.each(["waiting", "poor-accuracy", "matching-hold", "deviated", "paused", "arrived"] as const)("%sでは古い距離と方向と注意通知を表示しない", phase => {
    render(<NavigationGuidanceBar alert={{ ...alert, phase }} guidance={guidance} />);
    expect(screen.queryByText("100 m先")).not.toBeInTheDocument();
    expect(screen.queryByText("右折")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("接近通知")).not.toBeInTheDocument();
  });

  it("案内データのない実APIでは固定の曲がり角や距離を表示しない", () => {
    render(<NavigationGuidanceBar alert={alert} />);
    expect(screen.getByText("経路に沿って進む")).toBeVisible();
    expect(screen.queryByText(/m先/)).not.toBeInTheDocument();
  });

  it("サイドバー開閉で表示場所を変えても開始文を繰り返し通知しない", () => {
    vi.useFakeTimers();
    try {
      const props = { alert, guidance, announcement: "ナビを開始します" };
      const view = render(<NavigationGuidanceBar {...props} />);
      const live = view.container.querySelector(".nav-approach-region")!;
      expect(live).toHaveTextContent("ナビを開始します");
      act(() => vi.advanceTimersByTime(1000));
      view.rerender(<NavigationGuidanceBar {...props} hidden />);
      view.rerender(<NavigationGuidanceBar {...props} />);
      expect(live).not.toHaveTextContent("ナビを開始します");
      expect(screen.getByLabelText("接近通知")).toBeVisible();
    } finally {
      vi.useRealTimers();
    }
  });
});
