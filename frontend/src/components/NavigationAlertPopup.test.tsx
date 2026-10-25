import { act, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { NavigationAlertPopup } from "./NavigationAlertPopup";

it("複数区間の既存文言と件数をまとめ、進捗停止・解除で消す", () => {
  const alert = {
    phase: "tracking" as const,
    alertMessage: "この先に、過去事故の参考区間が続きます" as const,
    alertIds: ["route-1:nav-2-2", "route-1:nav-4-4"],
  };
  const view = render(<NavigationAlertPopup alert={alert} />);
  expect(screen.getByLabelText("接近通知")).toHaveTextContent("参考区間 2件");
  expect(screen.getByLabelText("接近通知")).toHaveTextContent(alert.alertMessage);
  view.rerender(<NavigationAlertPopup alert={{ ...alert, phase: "paused" }} />);
  expect(screen.queryByLabelText("接近通知")).not.toBeInTheDocument();
  view.rerender(<NavigationAlertPopup alert={{ ...alert, alertMessage: null, alertIds: [] }} />);
  expect(screen.queryByLabelText("接近通知")).not.toBeInTheDocument();
});

it("既存のlive領域で開始を通知し、接近通知では開始文を繰り返さない", () => {
  vi.useFakeTimers();
  try {
    const view = render(<NavigationAlertPopup />);
    const live = view.container.querySelector(".nav-approach-region")!;
    expect(live).toHaveAttribute("aria-live", "polite");
    view.rerender(<NavigationAlertPopup announcement="ナビを開始します" />);
    expect(view.container.querySelector(".nav-approach-region")).toBe(live);
    expect(live.querySelector(".sr-only")).toHaveTextContent("ナビを開始します");
    act(() => vi.advanceTimersByTime(1000));
    expect(live).toBeEmptyDOMElement();
    view.rerender(<NavigationAlertPopup announcement="ナビを開始します" alert={{
      phase: "tracking", alertMessage: "過去事故の参考区間にいます", alertIds: ["route-1:nav-0-0"],
    }} />);
    expect(live).not.toHaveTextContent("ナビを開始します");
    expect(screen.getByLabelText("接近通知")).toHaveTextContent("過去事故の参考区間にいます");
  } finally {
    vi.useRealTimers();
  }
});
