import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import App from "./App";

vi.mock("./components/LogoIntro", () => ({
  LogoIntro: ({ onComplete }: { onComplete: () => void }) => (
    <button onClick={onComplete}>動画をスキップして開始</button>
  ),
}));

describe("mode selection and logo restart", () => {
  it("opens navigation through mode selection, and replays the intro before returning home", async () => {
    const user = userEvent.setup();
    render(<App />);
    await user.click(
      screen.getByRole("button", { name: "動画をスキップして開始" }),
    );
    expect(screen.getByRole("heading", { name: "モード選択" })).toBeVisible();
    const navigation = screen.getByRole("button", {
      name: "安全経路選択ナビモード",
    });
    const community = screen.getByRole("button", {
      name: "みんなの安全情報モード",
    });
    await user.click(navigation);
    expect(navigation).toHaveAttribute("aria-pressed", "true");
    expect(community).toHaveAttribute("aria-pressed", "false");
    const input = await screen.findByRole("combobox", { name: "出発地" });
    fireEvent.change(input, { target: { value: "途中の入力" } });
    expect(screen.getByRole("button", { name: "モード選択" })).toBeVisible();
    await user.click(
      screen.getByRole("button", { name: "AcciLessNavi ホームへ" }),
    );
    await user.click(
      screen.getByRole("button", { name: "動画をスキップして開始" }),
    );
    expect(screen.getByRole("heading", { name: "モード選択" })).toBeVisible();
    expect(
      screen.queryByRole("combobox", { name: "出発地" }),
    ).not.toBeInTheDocument();
  });

  it("returns to mode selection from the place form and opens the community map", async () => {
    const user = userEvent.setup();
    render(<App />);
    await user.click(
      screen.getByRole("button", { name: "動画をスキップして開始" }),
    );
    await user.click(
      screen.getByRole("button", { name: "安全経路選択ナビモード" }),
    );
    await screen.findByRole("combobox", { name: "出発地" });
    await user.click(screen.getByRole("button", { name: "モード選択" }));
    await user.click(
      screen.getByRole("button", { name: "みんなの安全情報モード" }),
    );
    expect(
      await screen.findByRole("application", {
        name: "みんなの安全情報の地図",
      }),
    ).toBeVisible();
    expect(
      screen.getByRole("combobox", { name: "表示する場所" }),
    ).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "安全" }),
    ).not.toBeInTheDocument();
  });

  it("cancels a pending mode transition when the logo is clicked", async () => {
    const user = userEvent.setup();
    render(<App />);
    await user.click(
      screen.getByRole("button", { name: "動画をスキップして開始" }),
    );
    await user.click(
      screen.getByRole("button", { name: "みんなの安全情報モード" }),
    );
    await user.click(
      screen.getByRole("button", { name: "AcciLessNavi ホームへ" }),
    );
    await user.click(
      screen.getByRole("button", { name: "動画をスキップして開始" }),
    );
    await new Promise((resolve) => setTimeout(resolve, 180));
    expect(screen.getByRole("heading", { name: "モード選択" })).toBeVisible();
    expect(screen.queryByRole("application")).not.toBeInTheDocument();
  });
});
