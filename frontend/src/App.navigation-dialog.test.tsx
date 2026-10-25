import { fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SAFETY_NOTICE, WALKING_NOTICE } from "./components/NavigationConfirmDialog";
import { resetApi } from "./test/apiMock";
import { advance, installGeolocation, openResults, startGroup } from "./test/navigationApp";

vi.mock("./api/client", async () => (await import("./test/apiMock")).clientMock());
vi.mock("./components/LogoIntro", () => ({
  LogoIntro: ({ onComplete }: { onComplete: () => void }) => (
    <button onClick={onComplete}>動画をスキップして開始</button>
  ),
}));

beforeEach(resetApi);
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(navigator, "geolocation");
});

describe("開始前の注意ダイアログ（7.9）", () => {
  it("確認の操作があるまで開始せず、位置源も登録しない。注意文は仕様の文言のまま表示する", async () => {
    const gps = installGeolocation();
    await openResults();
    vi.useFakeTimers();
    fireEvent.click(startGroup("最短時間").getByRole("button", { name: "実機GPSで開始" }));
    const dialog = screen.getByRole("dialog", { name: "ナビを開始する前に" });
    expect(dialog).toHaveAttribute("aria-modal", "true");
    expect(within(dialog).getByText(SAFETY_NOTICE)).toBeVisible();
    expect(within(dialog).getByText(WALKING_NOTICE)).toBeVisible();
    expect(within(dialog).getByText("最短時間経路を実機GPSで開始します")).toBeVisible();
    expect(within(dialog).getByRole("button", { name: "確認して開始" })).toHaveFocus();
    advance(10_000);
    expect(gps.watchPosition).not.toHaveBeenCalled();
    expect(screen.queryByRole("complementary", { name: "ナビ" })).not.toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "確認して開始" }));
    expect(gps.watchPosition).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("complementary", { name: "ナビ" })).toBeVisible();
  });

  it("キャンセルとEscapeでは開始せず、開始ボタンへフォーカスを戻す", async () => {
    const gps = installGeolocation();
    await openResults();
    const button = startGroup("事故履歴を考慮").getByRole("button", { name: "模擬走行で開始" });
    button.focus();
    fireEvent.click(button);
    fireEvent.click(screen.getByRole("button", { name: "キャンセル" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(button).toHaveFocus();
    fireEvent.click(button);
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.queryByRole("complementary", { name: "ナビ" })).not.toBeInTheDocument();
    expect(gps.watchPosition).not.toHaveBeenCalled();
  });

  it("模擬走行も同じ確認を要し、Tabはダイアログ内に留まる", async () => {
    await openResults();
    fireEvent.click(startGroup("最短時間").getByRole("button", { name: "模擬走行で開始" }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText("最短時間経路を模擬走行で開始します")).toBeVisible();
    const cancel = within(dialog).getByRole("button", { name: "キャンセル" });
    const confirm = within(dialog).getByRole("button", { name: "確認して開始" });
    confirm.focus();
    fireEvent.keyDown(confirm, { key: "Tab" });
    expect(cancel).toHaveFocus();
    fireEvent.keyDown(cancel, { key: "Tab", shiftKey: true });
    expect(confirm).toHaveFocus();
  });
});
