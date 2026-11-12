import { fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createDemoRouteResponse } from "./api/demo";
import { FASTEST_LABEL, SAFE_LABEL } from "./components/RouteCallouts";
import type { RouteRequest } from "./lib/types";
import { createFetchCounter } from "./test/fetchCounter";
import { api, resetApi } from "./test/apiMock";
import { openNavigationSidebar, advance, beginNavigation, installGeolocation, openResults, startGroup } from "./test/navigationApp";

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
  it.each([1280, 390])("幅%dでナビ開始後もサイドバーは閉じ、既存ボタンへフォーカスを移し、明示的に開けばナビを表示する", async (width) => {
    vi.stubGlobal("innerWidth", width);
    await openResults();
    const live = document.querySelector(".nav-approach-region")!;
    expect(live).toHaveAttribute("aria-live", "polite");
    expect(live).toHaveTextContent("");
    vi.useFakeTimers();
    beginNavigation("最短時間", "模擬走行で開始");
    const toggle = screen.getByRole("button", { name: "条件サイドバーを開く" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).toHaveFocus();
    expect(document.getElementById("search-conditions")).not.toBeVisible();
    const guidanceBar = screen.getByRole("region", { name: "ナビ案内" });
    expect(guidanceBar).toBeVisible();
    expect(guidanceBar.closest(".nav-guidance-overlay")?.parentElement).toHaveClass("result-page");
    expect(screen.queryByRole("complementary", { name: "ナビ" })).not.toBeInTheDocument();
    expect(document.querySelector(".nav-approach-region")).toBe(live);
    expect(live).toHaveTextContent("模擬走行を開始します。実際の現在地ではありません");
    expect(live.querySelector(".sr-only")).not.toBeNull();
    expect(within(document.querySelector(".nav-guidance-overlay") as HTMLElement).getByText("模擬走行中（実際の現在地ではありません）")).toBeVisible();
    advance(1000);
    fireEvent.click(toggle);
    const panel = screen.getByRole("complementary", { name: "ナビ" });
    expect(panel).toBeVisible();
    expect(within(panel).queryByRole("region", { name: "ナビ案内" })).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "ナビ案内" })).toBe(guidanceBar);
    expect(guidanceBar).toBeVisible();
    expect(live).not.toHaveTextContent("模擬走行を開始します");
    expect(screen.getByRole("button", { name: "ナビを停止" })).toBeVisible();
    fireEvent.click(toggle);
    expect(screen.getByRole("region", { name: "ナビ案内" })).toBe(guidanceBar);
    expect(guidanceBar).toBeVisible();
    expect(live).not.toHaveTextContent("模擬走行を開始します");
  });

  it("ナビ中は検索結果の説明を出さない", async () => {
    await openResults();
    expect(
      screen.getByRole("region", { name: "検索結果の説明" }),
    ).toBeVisible();
    vi.useFakeTimers();
    beginNavigation("最短時間", "模擬走行で開始");
    expect(
      screen.queryByRole("region", { name: "検索結果の説明" }),
    ).not.toBeInTheDocument();
  });

  it("開始操作は選定2経路の既存カードから開く", async () => {
    await openResults();
    expect(screen.getAllByRole("button", { name: /でナビを開始/ })).toHaveLength(2);
    for (const name of [FASTEST_LABEL, SAFE_LABEL]) {
      const group = startGroup(name);
      expect(group.getByRole("button", { name: "実機GPSで開始" })).toBeEnabled();
      expect(group.getByRole("button", { name: "模擬走行で開始" })).toBeEnabled();
    }
    // 比較表は出さない。表は説明の欄（経路ごとの推定リスクと事故件数）の中だけにある。
    const notes = screen.getByRole("region", { name: "検索結果の説明" });
    for (const table of screen.queryAllByRole("table")) {
      expect(notes).toContainElement(table);
    }
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
    expect(screen.getByRole("group", { name: scenario === "same" ? SAFE_LABEL : FASTEST_LABEL })).toBeVisible();
    if (scenario === "same") expect(within(screen.getByRole("dialog")).getByText("同一経路")).toBeVisible();
  });

  it("開始方法を閉じると既存カードに戻り、フォーカスも紙飛行機ボタンへ戻す", async () => {
    await openResults();
    const button = screen.getByRole("button", { name: "最短時間でナビを開始" });
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
    await screen.findByRole("button", { name: "最短時間でナビを開始" });
    const group = startGroup("最短時間");
    expect(group.getByRole("button", { name: "実機GPSで開始" })).toBeDisabled();
    expect(group.getByRole("button", { name: "模擬走行で開始" })).toBeDisabled();
    expect(screen.getByText("模擬走行の設定").closest("summary")).toHaveFocus();
    expect(screen.queryByRole("complementary", { name: "ナビ" })).not.toBeInTheDocument();
  });

  it("模擬走行を開始すると、サイドバーの結果と条件エディタがナビ表示に置き換わり、地図は残る", async () => {
    await openResults();
    vi.useFakeTimers();
    beginNavigation(SAFE_LABEL, "模擬走行で開始");
    openNavigationSidebar();
    const panel = screen.getByRole("complementary", { name: "ナビ" });
    expect(within(panel).getByRole("button", { name: "ナビを停止" })).toBeVisible();
    expect(within(panel).queryByRole("combobox", { name: "目的地" })).not.toBeInTheDocument();
    expect(within(panel).queryByRole("button", { name: "再検索" })).not.toBeInTheDocument();
    expect(within(panel).queryByText("集計条件と全候補")).not.toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "表示する経路" })).not.toBeInTheDocument();
    expect(screen.getByLabelText("地図の凡例")).toHaveTextContent("ナビ経路");
    advance(3000);
    expect(document.querySelector(".map-nav-location")).not.toBeNull();
    expect(screen.getByRole("region", { name: "ナビ案内" })).toBeVisible();
    // 比較入力と状態切替を抑止。ロゴは動画・モード選択へ戻る入口として使用できる。
    expect(screen.getByRole("button", { name: "AcciLessNavi ホームへ" })).toBeEnabled();
    fireEvent.click(within(panel).getByRole("button", { name: "ナビを停止" }));
    expect(screen.getByRole("complementary", { name: "検索条件" })).toBeVisible();
    expect(screen.getByRole("group", { name: "表示する経路" })).toBeVisible();
    expect(screen.queryByRole("region", { name: "ナビ案内" })).not.toBeInTheDocument();
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
    beginNavigation("最短時間", "模擬走行で開始");
    openNavigationSidebar();
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
    beginNavigation("最短時間", "模擬走行で開始");
    openNavigationSidebar();
    expect(watchPosition).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "ナビを停止" }));
    beginNavigation("最短時間", "実機GPSで開始");
    openNavigationSidebar();
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
    beginNavigation("最短時間", "実機GPSで開始");
    openNavigationSidebar();
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
    beginNavigation("最短時間", "模擬走行で開始");
    openNavigationSidebar();
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
    beginNavigation("最短時間", "模擬走行で開始");
    openNavigationSidebar();
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
    const button = screen.getByRole("button", { name: "最短時間でナビを開始" });
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
    openNavigationSidebar();
    expect(gps.watchPosition).toHaveBeenCalledTimes(1);
    expect(screen.getByText("測位待ち")).toBeVisible();
    expect(document.body).not.toHaveTextContent("模擬");
    expect(screen.queryByRole("button", { name: "逸脱を試す" })).not.toBeInTheDocument();
    gps.fail(1);
    expect(screen.getByText("位置情報の利用が許可されていません。設定を変更して再開してください")).toBeVisible();
    expect(document.body).not.toHaveTextContent("模擬");
  });

  it("フラグなしの同一IDの紙飛行機も事故考慮として開始する", async () => {
    vi.stubEnv("VITE_ENABLE_SIMULATION", undefined);
    installGeolocation();
    api.searchRoutes.mockImplementationOnce((request: RouteRequest) =>
      Promise.resolve(createDemoRouteResponse(request, "same")),
    );
    await openResults();
    fireEvent.click(screen.getByRole("button", { name: /でナビを開始/ }));
    expect(screen.getByRole("dialog")).toHaveTextContent(`${SAFE_LABEL}を実機GPSで開始します`);
    fireEvent.click(screen.getByRole("button", { name: "確認して開始" }));
    openNavigationSidebar();
    expect(screen.getByText(SAFE_LABEL, { selector: "dd" })).toBeVisible();
  });

  it("非対応環境の開始拒否も模擬走行を案内しない", async () => {
    vi.stubEnv("VITE_ENABLE_SIMULATION", "false");
    await openResults();
    fireEvent.click(screen.getByRole("button", { name: "最短時間でナビを開始" }));
    fireEvent.click(screen.getByRole("button", { name: "確認して開始" }));
    expect(screen.getByRole("alert")).toHaveTextContent("このブラウザーでは位置情報を利用できません。");
    expect(document.body).not.toHaveTextContent("模擬");
  });
});
