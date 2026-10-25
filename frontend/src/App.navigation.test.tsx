import { fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createDemoRouteResponse } from "./api/demo";
import type { RouteRequest } from "./lib/types";
import { createFetchCounter } from "./test/fetchCounter";
import { api, resetApi } from "./test/apiMock";
import { advance, beginNavigation, installGeolocation, openResults, startGroup } from "./test/navigationApp";

vi.mock("./api/client", async () => (await import("./test/apiMock")).clientMock());
vi.mock("./components/LogoIntro", () => ({
  LogoIntro: ({ onComplete }: { onComplete: () => void }) => (
    <button onClick={onComplete}>動画をスキップして開始</button>
  ),
}));

beforeEach(() => {
  resetApi();
  vi.stubEnv("VITE_ENABLE_SIMULATION", "true");
});

afterEach(() => {
  vi.unstubAllEnvs();
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(navigator, "geolocation");
});

describe("ナビ中の画面構成と結線（手順10）", () => {
  it("開始操作は選定2経路の既存カードから開く", async () => {
    await openResults();
    expect(screen.getAllByRole("button", { name: /でナビを開始/ })).toHaveLength(2);
    for (const name of ["最短ルート", "安全ルート"]) {
      const group = startGroup(name);
      expect(group.getByRole("button", { name: "実機GPSで開始" })).toBeEnabled();
      expect(group.getByRole("button", { name: "模擬走行で開始" })).toBeEnabled();
    }
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it.each(["zero", "same"] as const)("%sでは1枚の選定カードだけから開始できる", async (scenario) => {
    api.searchRoutes.mockImplementationOnce((request: RouteRequest) =>
      Promise.resolve(createDemoRouteResponse(request, scenario)),
    );
    await openResults();
    expect(screen.getAllByRole("button", { name: /でナビを開始/ })).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: /でナビを開始/ }));
    expect(screen.getAllByRole("button", { name: "実機GPSで開始" })).toHaveLength(1);
    expect(screen.getAllByRole("button", { name: "模擬走行で開始" })).toHaveLength(1);
    expect(screen.queryByRole("group", { name: "安全ルート" })).not.toBeInTheDocument();
    if (scenario === "same") expect(within(screen.getByRole("dialog")).getByText("同一経路")).toBeVisible();
  });

  it("開始方法を閉じると既存カードに戻り、フォーカスも紙飛行機ボタンへ戻す", async () => {
    await openResults();
    const button = screen.getByRole("button", { name: "最短ルートでナビを開始" });
    button.focus();
    fireEvent.click(button);
    const dialog = screen.getByRole("dialog", { name: "ナビ開始" });
    expect(within(dialog).getByRole("button", { name: "実機GPSで開始" })).toHaveFocus();
    const close = within(dialog).getByRole("button", { name: "閉じる" });
    close.focus();
    fireEvent.keyDown(close, { key: "Tab" });
    expect(within(dialog).getByRole("button", { name: "実機GPSで開始" })).toHaveFocus();
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(button).toHaveFocus();
  });

  it("地図を使えないときは開始を拒否し、操作可能な要素にフォーカスを置く", async () => {
    const user = await openResults();
    await user.click(screen.getByText("デモ状態切替"));
    await user.click(screen.getByRole("button", { name: "地図利用不可" }));
    await screen.findByText("地図を利用できません");
    await screen.findByRole("button", { name: "最短ルートでナビを開始" });
    const group = startGroup("最短ルート");
    expect(group.getByRole("button", { name: "実機GPSで開始" })).toBeDisabled();
    expect(group.getByRole("button", { name: "模擬走行で開始" })).toBeDisabled();
    expect(screen.getByText("模擬走行の設定").closest("summary")).toHaveFocus();
    expect(screen.queryByRole("complementary", { name: "ナビ" })).not.toBeInTheDocument();
  });

  it("模擬走行を開始すると、サイドバーの結果と条件エディタがナビ表示に置き換わり、地図は残る", async () => {
    await openResults();
    vi.useFakeTimers();
    beginNavigation("安全ルート", "模擬走行で開始");
    const panel = screen.getByRole("complementary", { name: "ナビ" });
    expect(within(panel).getByRole("button", { name: "ナビを停止" })).toBeVisible();
    expect(within(panel).queryByRole("combobox", { name: "目的地" })).not.toBeInTheDocument();
    expect(within(panel).queryByRole("button", { name: "再検索" })).not.toBeInTheDocument();
    expect(within(panel).queryByText("集計条件と全候補")).not.toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "表示する経路" })).not.toBeInTheDocument();
    expect(screen.getByLabelText("地図の凡例")).toHaveTextContent("ナビ経路");
    advance(3000);
    expect(document.querySelector(".map-nav-location")).not.toBeNull();
    // 比較入力の抑止：ヘッダーのロゴ（地点入力へ戻る）と状態切替も使えない。
    expect(screen.getByRole("button", { name: "AcciLessNavi 地点入力へ" })).toBeDisabled();
    fireEvent.click(within(panel).getByRole("button", { name: "ナビを停止" }));
    expect(screen.getByRole("complementary", { name: "検索条件" })).toBeVisible();
    expect(screen.getByRole("group", { name: "表示する経路" })).toBeVisible();
  });

  it("模擬だけの移動では health・conditions・route・geocode・fetch を一切呼ばない", async () => {
    await openResults();
    const counter = createFetchCounter(() => Promise.reject(new Error("通信しない")));
    vi.stubGlobal("fetch", counter.fetch);
    const before = {
      health: api.getHealth.mock.calls.length,
      conditions: api.getConditions.mock.calls.length,
      route: api.searchRoutes.mock.calls.length,
      places: api.searchPlaces.mock.calls.length,
    };
    vi.useFakeTimers();
    beginNavigation("最短ルート", "模擬走行で開始");
    advance(120_000);
    expect(api.getHealth).toHaveBeenCalledTimes(before.health);
    expect(api.getConditions).toHaveBeenCalledTimes(before.conditions);
    expect(api.searchRoutes).toHaveBeenCalledTimes(before.route);
    expect(api.searchPlaces).toHaveBeenCalledTimes(before.places);
    expect(counter.count()).toBe(0);
  });

  it("実機GPSの開始はwatchPositionを指定オプションで1回だけ登録し、停止でclearWatchする。模擬では登録しない", async () => {
    const watchPosition = vi.fn((..._args: unknown[]) => 7);
    const clearWatch = vi.fn();
    Object.defineProperty(navigator, "geolocation", {
      configurable: true,
      value: { watchPosition, clearWatch, getCurrentPosition: vi.fn() },
    });
    await openResults();
    vi.useFakeTimers();
    beginNavigation("最短ルート", "模擬走行で開始");
    expect(watchPosition).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "ナビを停止" }));
    beginNavigation("最短ルート", "実機GPSで開始");
    expect(watchPosition).toHaveBeenCalledTimes(1);
    expect(watchPosition.mock.calls[0][2]).toEqual({
      enableHighAccuracy: true,
      timeout: 10000,
      maximumAge: 1000,
    });
    advance(30_000);
    expect(watchPosition).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "ナビを停止" }));
    expect(clearWatch).toHaveBeenCalledWith(7);
  });

  it("位置情報が使えない環境では実機GPSの開始を拒否し、理由と模擬走行を案内する", async () => {
    await openResults();
    beginNavigation("最短ルート", "実機GPSで開始");
    expect(screen.getByRole("alert")).toHaveTextContent("模擬走行");
    expect(screen.getByRole("complementary", { name: "検索条件" })).toBeVisible();
  });

  it("開始・終了でサイドバーの見出しへフォーカスを移し、前の内容のスクロール位置を引き継がない", async () => {
    await openResults();
    fireEvent.click(screen.getByRole("button", { name: "条件サイドバーを開く" }));
    const panel = screen.getByRole("complementary", { name: "検索条件" });
    const scrollTo = vi.fn();
    panel.scrollTo = scrollTo as unknown as typeof panel.scrollTo;
    vi.useFakeTimers();
    beginNavigation("最短ルート", "模擬走行で開始");
    expect(screen.getByRole("heading", { name: "ナビ", level: 1 })).toHaveFocus();
    expect(scrollTo).toHaveBeenCalledWith(0, 0);
    fireEvent.click(screen.getByRole("button", { name: "ナビを停止" }));
    expect(screen.getByRole("heading", { name: "検索条件", level: 1 })).toHaveFocus();
  });

  it("位置・進捗・通知済みIDをストレージ・URL・履歴・コンソールへ出さない", async () => {
    await openResults();
    const setItem = vi.spyOn(Storage.prototype, "setItem");
    const pushState = vi.spyOn(window.history, "pushState");
    const replaceState = vi.spyOn(window.history, "replaceState");
    const log = vi.spyOn(console, "log");
    const warn = vi.spyOn(console, "warn");
    const error = vi.spyOn(console, "error");
    const href = window.location.href;
    vi.useFakeTimers();
    beginNavigation("最短ルート", "模擬走行で開始");
    advance(120_000);
    expect(setItem).not.toHaveBeenCalled();
    expect(pushState).not.toHaveBeenCalled();
    expect(replaceState).not.toHaveBeenCalled();
    expect(window.location.href).toBe(href);
    expect(window.history.state ?? null).toBeNull();
    for (const spy of [log, warn, error]) expect(spy).not.toHaveBeenCalled();
    expect(sessionStorage.length + localStorage.length).toBe(0);
  });
});


describe("模擬走行を含めないビルド", () => {
  it.each([undefined, "false", "1", "TRUE"])("フラグ %s では紙飛行機から注意確認へ直接進み、確認後だけGPSを開始する", async (flag) => {
    vi.stubEnv("VITE_ENABLE_SIMULATION", flag);
    const gps = installGeolocation();
    await openResults();
    const button = screen.getByRole("button", { name: "最短ルートでナビを開始" });
    button.focus();
    fireEvent.click(button);
    expect(screen.getByRole("dialog", { name: "ナビを開始する前に" })).toBeVisible();
    expect(screen.queryByRole("dialog", { name: "ナビ開始" })).not.toBeInTheDocument();
    expect(document.body).not.toHaveTextContent("模擬");
    expect(gps.watchPosition).not.toHaveBeenCalled();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(button).toHaveFocus();
    expect(gps.watchPosition).not.toHaveBeenCalled();
    fireEvent.click(button);
    fireEvent.click(screen.getByRole("button", { name: "確認して開始" }));
    expect(gps.watchPosition).toHaveBeenCalledTimes(1);
    expect(screen.getByText("測位待ち")).toBeVisible();
    expect(document.body).not.toHaveTextContent("模擬");
    expect(screen.queryByRole("button", { name: "逸脱を試す" })).not.toBeInTheDocument();
    gps.fail(1);
    expect(screen.getByText("位置情報の利用が許可されていません。設定を変更して再開してください")).toBeVisible();
    expect(document.body).not.toHaveTextContent("模擬");
  });

  it("非対応環境の開始拒否も模擬走行を案内しない", async () => {
    vi.stubEnv("VITE_ENABLE_SIMULATION", "false");
    await openResults();
    fireEvent.click(screen.getByRole("button", { name: "最短ルートでナビを開始" }));
    fireEvent.click(screen.getByRole("button", { name: "確認して開始" }));
    expect(screen.getByRole("alert")).toHaveTextContent("このブラウザーでは位置情報を利用できません。");
    expect(document.body).not.toHaveTextContent("模擬");
  });
});
