import { act, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createDemoRouteResponse } from "./api/demo";
import { ApiError } from "./api/errors";
import { api, resetApi } from "./test/apiMock";
import { openNavigationSidebar, advance, startGroup, beginNavigation, openResults } from "./test/navigationApp";
import type { RouteRequest, RouteResponse } from "./lib/types";

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
  Reflect.deleteProperty(document, "visibilityState");
});

const RESEARCH = "安全な場所で停止してから現在地から再検索";
const phase = () => within(screen.getByLabelText("ナビの状態")).getByRole("status");

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((accept, decline) => {
    resolve = accept;
    reject = decline;
  });
  return { promise, resolve, reject };
}

/** 4倍の模擬走行を開始し、逸脱の表示まで進める。 */
async function deviate() {
  await openResults();
  startGroup("最短ルート");
  fireEvent.click(screen.getByText("模擬走行の設定"));
  fireEvent.click(screen.getByRole("radio", { name: "4倍" }));
  fireEvent.change(screen.getByLabelText(/seed/), { target: { value: "42" } });
  vi.useFakeTimers();
  beginNavigation("最短ルート", "模擬走行で開始");
  openNavigationSidebar();
  advance(10_000);
  fireEvent.click(screen.getByRole("button", { name: "逸脱を試す" }));
  // 4倍なので、実時間3秒は仮想12秒（逸脱の確定は10秒・3測位）。
  advance(3_000);
  expect(phase()).toHaveTextContent("経路から外れています");
}

const searches = () => api.searchRoutes.mock.calls.length;
const conditionCalls = () => api.getConditions.mock.calls.length;
function setVisibility(state: "hidden" | "visible") {
  Object.defineProperty(document, "visibilityState", { configurable: true, value: state });
  document.dispatchEvent(new Event("visibilitychange"));
}

describe("現在地からの明示的再検索の画面（手順12・7.6・7.7）", () => {
  it("同一IDから事故考慮として開始し、再検索で経路が分かれても事故考慮を引き継ぐ", async () => {
    api.searchRoutes.mockImplementationOnce((request: RouteRequest) =>
      Promise.resolve(createDemoRouteResponse(request, "same")),
    );
    await openResults();
    vi.useFakeTimers();
    beginNavigation("安全ルート", "模擬走行で開始");
    openNavigationSidebar();
    expect(screen.getByText("安全ルート", { selector: "dd" })).toBeVisible();
    advance(10_000);
    fireEvent.click(screen.getByRole("button", { name: "逸脱を試す" }));
    advance(12_000);
    fireEvent.click(screen.getByRole("button", { name: RESEARCH }));
    await act(async () => { await vi.advanceTimersByTimeAsync(10); });
    const response = await api.searchRoutes.mock.results.at(-1)!.value;
    expect(response.fastest_id).not.toBe(response.safest_id);
    expect(screen.getByText("安全ルート", { selector: "dd" })).toBeVisible();
    expect(api.searchRoutes).toHaveBeenCalledTimes(2);
  });

  it("開始直後の位置確認中には再検索を案内しない", async () => {
    await openResults();
    vi.useFakeTimers();
    beginNavigation("最短ルート", "模擬走行で開始");
    openNavigationSidebar();
    advance(3_000);
    expect(phase()).toHaveTextContent("照合保留");
    expect(screen.queryByRole("button", { name: RESEARCH })).not.toBeInTheDocument();
  });

  it("逸脱表示から押すと、模擬位置から条件と経路を1回ずつ取得し、同じ種別・倍率・seedで新経路のナビを継続する", async () => {
    await deviate();
    const firstRequest: RouteRequest = api.searchRoutes.mock.calls[0][0];
    const before = { searches: searches(), conditions: conditionCalls() };
    expect(screen.getByText("模擬位置から条件と経路を取得します")).toBeVisible();
    const pending = deferred<RouteResponse>();
    api.searchRoutes.mockImplementationOnce(() => pending.promise);
    fireEvent.click(screen.getByRole("button", { name: RESEARCH }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10);
    });
    // 再検索中：中止・停止・一時停止の操作がなく、入力と旧結果は解除されている。
    const panel = screen.getByRole("complementary", { name: "ナビ" });
    expect(within(panel).getByRole("status")).toHaveTextContent("再検索中");
    expect(within(panel).queryAllByRole("button")).toHaveLength(0);
    expect(screen.getByText("模擬位置から条件と経路を取得します")).toBeVisible();
    expect(screen.getByRole("button", { name: "AcciLessNavi 地点入力へ" })).toBeDisabled();
    expect(conditionCalls()).toBe(before.conditions + 1);
    expect(searches()).toBe(before.searches + 1);
    const request: RouteRequest = api.searchRoutes.mock.calls.at(-1)![0];
    expect(request.origin).not.toEqual(firstRequest.origin);
    expect(request.dest).toEqual(firstRequest.dest);
    expect(request).toMatchObject({ mode: firstRequest.mode, max_detour_min: firstRequest.max_detour_min });
    // 経路上の位置ではなく、模擬の測位（揺らぎを含む実測相当）をoriginにする。
    const routeStart = firstRequest.origin;
    expect(Math.abs(request.origin.lat - routeStart.lat) + Math.abs(request.origin.lon - routeStart.lon)).toBeGreaterThan(0);
    await act(async () => {
      pending.resolve(createDemoRouteResponse(request));
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10);
    });
    expect(screen.getByText("模擬走行中（実際の現在地ではありません）")).toBeVisible();
    expect(screen.getByText("倍率 4倍・進行中")).toBeVisible();
    // 新経路の先頭から、初回の位置確認をやり直す。
    expect(phase()).toHaveTextContent(/測位待ち|照合保留/);
    expect(within(screen.getByLabelText("模擬走行の操作")).getByText("42")).toBeVisible();
    expect(screen.getByRole("button", { name: "逸脱を試す" })).toBeEnabled();
    // 追加の通信はない。
    advance(30_000);
    expect(searches()).toBe(before.searches + 1);
    expect(conditionCalls()).toBe(before.conditions + 1);
    // 入力欄の出発地は新しいoriginになる。
    fireEvent.click(screen.getByRole("button", { name: "ナビを停止" }));
    expect(screen.getByRole("combobox", { name: "出発地" })).toHaveValue("模擬位置");
  });

  it("503では停止したまま再試行を提示し、自動再送しない。再試行で同種別のナビが戻り、入力編集では通常入力へ戻る", async () => {
    await deviate();
    const failing = vi.fn().mockRejectedValue(new ApiError("混雑", "service_busy", 503));
    api.searchRoutes.mockImplementationOnce(failing);
    fireEvent.click(screen.getByRole("button", { name: RESEARCH }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10);
    });
    const failed = searches();
    expect(screen.queryByRole("complementary", { name: "ナビ" })).not.toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("サーバーが混み合っています");
    expect(screen.queryByText("ナビを停止")).not.toBeInTheDocument();
    advance(10 * 60_000);
    expect(searches()).toBe(failed);
    const conditionsBefore = conditionCalls();
    fireEvent.click(screen.getByRole("button", { name: "再試行" }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10);
    });
    expect(conditionCalls()).toBe(conditionsBefore + 1);
    expect(searches()).toBe(failed + 1);
    expect(screen.getByText("模擬走行中（実際の現在地ではありません）")).toBeVisible();
    expect(phase()).toHaveTextContent(/測位待ち|照合保留/);
  });

  it("失敗後に入力を編集すると、再試行ボタンは消えて通常の入力へ戻る", async () => {
    await deviate();
    api.searchRoutes.mockRejectedValueOnce(new ApiError("混雑", "service_busy", 503));
    fireEvent.click(screen.getByRole("button", { name: RESEARCH }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10);
    });
    expect(screen.getByRole("button", { name: "再試行" })).toBeVisible();
    fireEvent.click(screen.getByRole("radio", { name: "自転車" }));
    expect(screen.queryByRole("button", { name: "再試行" })).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("再検索中のhiddenで中断し、遅い成功を反映せず、復帰後は手動の再試行だけを案内する", async () => {
    await deviate();
    const pending = deferred<RouteResponse>();
    let signal!: AbortSignal;
    api.searchRoutes.mockImplementationOnce((_request: RouteRequest, received: AbortSignal) => {
      signal = received;
      return pending.promise;
    });
    fireEvent.click(screen.getByRole("button", { name: RESEARCH }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10);
    });
    const sent = searches();
    act(() => setVisibility("hidden"));
    expect(signal.aborted).toBe(true);
    await act(async () => {
      pending.resolve(createDemoRouteResponse(api.searchRoutes.mock.calls.at(-1)![0]));
    });
    act(() => setVisibility("visible"));
    advance(60_000);
    expect(screen.getByRole("alert")).toHaveTextContent("再検索を中断しました。安全な場所で再試行してください");
    expect(screen.queryByRole("complementary", { name: "ナビ" })).not.toBeInTheDocument();
    expect(searches()).toBe(sent);
    expect(screen.getByRole("button", { name: "再試行" })).toBeVisible();
  });

  it("事故考慮経路なしで返ったときは、最短時間へ自動で切り替えず開始操作を案内する", async () => {
    await openResults();
    vi.useFakeTimers();
    beginNavigation("安全ルート", "模擬走行で開始");
    openNavigationSidebar();
    advance(10_000);
    fireEvent.click(screen.getByRole("button", { name: "逸脱を試す" }));
    advance(12_000);
    api.searchRoutes.mockImplementationOnce((request: RouteRequest) =>
      Promise.resolve(createDemoRouteResponse(request, "zero")),
    );
    fireEvent.click(screen.getByRole("button", { name: RESEARCH }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10);
    });
    expect(screen.queryByRole("complementary", { name: "ナビ" })).not.toBeInTheDocument();
    expect(
      screen.getByText("安全ルートがありません。最短ルートから開始する場合は、開始操作を行ってください"),
    ).toBeVisible();
    expect(screen.queryByRole("group", { name: "安全ルート" })).not.toBeInTheDocument();
    expect(startGroup("最短ルート").getByRole("button", { name: "模擬走行で開始" })).toBeVisible();
  });
});
