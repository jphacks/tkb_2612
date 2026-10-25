import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import {
  createDemoConditions,
  createDemoRouteResponse,
  demoHealth,
  DEMO_PLACES,
  findDemoPlaces,
} from "./api/demo";
import type { LatLon, RouteRequest, Scenario } from "./lib/types";
import { createFetchCounter } from "./test/fetchCounter";

const api = vi.hoisted(() => ({
  getHealth: vi.fn(),
  getConditions: vi.fn(),
  searchPlaces: vi.fn(),
  searchRoutes: vi.fn(),
}));
vi.mock("./api/client", async () => ({
  ApiError: (await import("./api/errors")).ApiError,
  isDemo: true,
  ...api,
}));
vi.mock("./components/LogoIntro", () => ({
  LogoIntro: ({ onComplete }: { onComplete: () => void }) => (
    <button onClick={onComplete}>動画をスキップして開始</button>
  ),
}));

beforeEach(() => {
  api.getHealth.mockReset().mockResolvedValue(demoHealth);
  api.searchPlaces
    .mockReset()
    .mockImplementation((query: string) => Promise.resolve(findDemoPlaces(query)));
  api.getConditions
    .mockReset()
    .mockImplementation((datetime: string, origin?: LatLon, dest?: LatLon) =>
      Promise.resolve(createDemoConditions(datetime, origin && dest ? origin : undefined)),
    );
  api.searchRoutes
    .mockReset()
    .mockImplementation((request: RouteRequest, _signal?: AbortSignal, scenario?: Scenario) =>
      Promise.resolve(
        createDemoRouteResponse(
          request,
          scenario === "same" || scenario === "zero" ? scenario : "normal",
        ),
      ),
    );
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(navigator, "geolocation");
});

/** 検索結果画面まで進める（実時間）。 */
export async function openResults() {
  const user = userEvent.setup();
  render(<App />);
  await user.click(screen.getByRole("button", { name: "動画をスキップして開始" }));
  await user.click(screen.getByRole("button", { name: "現在地を使う（デモ）" }));
  fireEvent.change(screen.getByRole("combobox", { name: "目的地" }), { target: { value: "上野" } });
  await user.click(await screen.findByRole("option", { name: DEMO_PLACES[1].label }));
  await user.click(screen.getByRole("button", { name: "条件を選ぶ" }));
  await waitFor(() => expect(screen.getByRole("button", { name: "ルートを比較する" })).toBeEnabled());
  await user.click(screen.getByRole("button", { name: "ルートを比較する" }));
  await screen.findByRole("group", { name: "表示する経路" });
  return user;
}

function startGroup(name: string) {
  return within(screen.getByRole("group", { name: `${name}経路` }));
}

/** 以降のナビを仮想時刻で進める。 */
function advance(ms: number) {
  act(() => {
    vi.advanceTimersByTime(ms);
  });
}

describe("ナビ中の画面構成と結線（手順10）", () => {
  it("開始操作は選定2経路だけに置き、事故0件で事故考慮が無ければ最短時間だけにする", async () => {
    await openResults();
    expect(screen.getAllByRole("button", { name: "実機GPSで開始" })).toHaveLength(2);
    expect(screen.getAllByRole("button", { name: "模擬走行で開始" })).toHaveLength(2);
    startGroup("最短時間").getByRole("button", { name: "実機GPSで開始" });
    startGroup("事故履歴を考慮").getByRole("button", { name: "模擬走行で開始" });
    // 全候補の比較表には開始操作がない。
    const table = screen.getByRole("table");
    expect(within(table).queryByRole("button", { name: /開始/ })).not.toBeInTheDocument();
  });

  it("模擬走行を開始すると、サイドバーの結果と条件エディタがナビ表示に置き換わり、地図は残る", async () => {
    await openResults();
    vi.useFakeTimers();
    fireEvent.click(startGroup("事故履歴を考慮").getByRole("button", { name: "模擬走行で開始" }));
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
    expect(screen.getByRole("complementary", { name: "検索結果と条件" })).toBeVisible();
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
    fireEvent.click(startGroup("最短時間").getByRole("button", { name: "模擬走行で開始" }));
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
    fireEvent.click(startGroup("最短時間").getByRole("button", { name: "模擬走行で開始" }));
    expect(watchPosition).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "ナビを停止" }));
    fireEvent.click(startGroup("最短時間").getByRole("button", { name: "実機GPSで開始" }));
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
    fireEvent.click(startGroup("最短時間").getByRole("button", { name: "実機GPSで開始" }));
    expect(screen.getByRole("alert")).toHaveTextContent("模擬走行");
    expect(screen.getByRole("complementary", { name: "検索結果と条件" })).toBeVisible();
  });
});
