import { render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
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
