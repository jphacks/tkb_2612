import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import App from "./App";

vi.mock("./components/LogoIntro", () => ({
  LogoIntro: ({ onComplete }: { onComplete: () => void }) => (
    <button onClick={onComplete}>動画をスキップして開始</button>
  ),
}));

async function start() {
  const user = userEvent.setup();
  render(<App />);
  await user.click(
    screen.getByRole("button", { name: "動画をスキップして開始" }),
  );
  return user;
}

describe("mode selection and logo restart", () => {
  it("centers a heading-free selector and requires two clicks even for the initial navigation selection", async () => {
    const user = await start();
    expect(
      screen.queryByRole("heading", { name: "モード選択" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("main", { name: "モード選択" })).toBeVisible();
    const navigation = screen.getByRole("button", {
      name: "安全経路選択ナビモード",
    });
    const community = screen.getByRole("button", {
      name: "みんなの安全情報モード",
    });
    expect(navigation).toHaveAttribute("aria-pressed", "true");
    fireEvent.pointerEnter(community);
    fireEvent.focus(community);
    expect(community).toHaveAttribute("aria-pressed", "false");
    await user.click(navigation);
    expect(
      screen.queryByRole("combobox", { name: "出発地" }),
    ).not.toBeInTheDocument();
    await user.click(navigation);
    const input = await screen.findByRole("combobox", { name: "出発地" });
    fireEvent.change(input, { target: { value: "途中の入力" } });
    await user.click(
      screen.getByRole("button", { name: "AcciLessNavi ホームへ" }),
    );
    await user.click(
      screen.getByRole("button", { name: "動画をスキップして開始" }),
    );
    expect(screen.getByRole("main", { name: "モード選択" })).toBeVisible();
    expect(
      screen.queryByRole("combobox", { name: "出発地" }),
    ).not.toBeInTheDocument();
  });

  it("selects on the first click and enters on the second after changing modes", async () => {
    const user = await start();
    const navigation = screen.getByRole("button", {
      name: "安全経路選択ナビモード",
    });
    const community = screen.getByRole("button", {
      name: "みんなの安全情報モード",
    });
    await user.click(community);
    expect(community).toHaveAttribute("aria-pressed", "true");
    expect(navigation).toHaveAttribute("aria-pressed", "false");
    expect(screen.queryByRole("application")).not.toBeInTheDocument();
    await user.click(navigation);
    expect(navigation).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    await user.click(navigation);
    await screen.findByRole("combobox", { name: "出発地" });
    await user.click(screen.getByRole("button", { name: "モード選択" }));
    const nextCommunity = screen.getByRole("button", {
      name: "みんなの安全情報モード",
    });
    await user.click(nextCommunity);
    expect(screen.queryByRole("application")).not.toBeInTheDocument();
    await user.click(nextCommunity);
    expect(
      await screen.findByRole("application", {
        name: "みんなの安全情報の地図",
      }),
    ).toBeVisible();
    expect(
      screen.getByRole("combobox", { name: "表示する場所" }),
    ).toBeVisible();
    expect(
      screen.queryByText(/マップ上の地点を押して/),
    ).not.toBeInTheDocument();
  });

  it("cancels a pending mode transition when the logo is clicked", async () => {
    const user = await start();
    const community = screen.getByRole("button", {
      name: "みんなの安全情報モード",
    });
    await user.click(community);
    await user.click(community);
    await user.click(
      screen.getByRole("button", { name: "AcciLessNavi ホームへ" }),
    );
    await user.click(
      screen.getByRole("button", { name: "動画をスキップして開始" }),
    );
    await new Promise((resolve) => setTimeout(resolve, 180));
    expect(screen.getByRole("main", { name: "モード選択" })).toBeVisible();
    expect(screen.queryByRole("application")).not.toBeInTheDocument();
  });
});
