import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
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
import { ApiError } from "./api/errors";
import type {
  ConditionsResponse,
  LatLon,
  RouteRequest,
  RouteResponse,
  Scenario,
} from "./lib/types";

const api = vi.hoisted(() => ({
  getHealth: vi.fn(),
  getConditions: vi.fn(),
  searchPlaces: vi.fn(),
  searchRoutes: vi.fn(),
}));
const mode = vi.hoisted(() => ({ demo: true }));

vi.mock("./api/client", async () => ({
  ApiError: (await import("./api/errors")).ApiError,
  get isDemo() {
    return mode.demo;
  },
  ...api,
}));
vi.mock("./components/LogoIntro", () => ({
  LogoIntro: ({ onComplete }: { onComplete: () => void }) => (
    <button onClick={onComplete}>動画をスキップして開始</button>
  ),
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((accept, decline) => {
    resolve = accept;
    reject = decline;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  mode.demo = true;
  api.getHealth.mockReset().mockResolvedValue(demoHealth);
  api.searchPlaces
    .mockReset()
    .mockImplementation((query: string) =>
      Promise.resolve(findDemoPlaces(query)),
    );
  api.getConditions
    .mockReset()
    .mockImplementation(
      (
        datetime: string,
        origin?: LatLon,
        dest?: LatLon,
        _signal?: AbortSignal,
        scenario?: Scenario,
      ) => {
        if (scenario === "conditions-error")
          return Promise.reject(
            new ApiError(
              "デモ：条件を取得できませんでした。",
              "internal_error",
              500,
            ),
          );
        return Promise.resolve(
          createDemoConditions(datetime, origin && dest ? origin : undefined),
        );
      },
    );
  api.searchRoutes
    .mockReset()
    .mockImplementation(
      (request: RouteRequest, _signal?: AbortSignal, scenario?: Scenario) =>
        Promise.resolve(
          createDemoRouteResponse(
            request,
            scenario === "same" || scenario === "zero" ? scenario : "normal",
          ),
        ),
    );
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(navigator, "geolocation");
});

async function openConditions() {
  const user = userEvent.setup();
  render(<App />);
  await user.click(
    screen.getByRole("button", { name: "動画をスキップして開始" }),
  );
  await user.click(
    screen.getByRole("button", { name: "現在地を使う（デモ）" }),
  );
  fireEvent.change(screen.getByRole("combobox", { name: "目的地" }), {
    target: { value: "上野" },
  });
  await user.click(
    await screen.findByRole("option", { name: DEMO_PLACES[1].label }),
  );
  await user.click(screen.getByRole("button", { name: "条件を選ぶ" }));
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "ルートを比較する" }),
    ).toBeEnabled(),
  );
  return user;
}

async function openResults() {
  const user = await openConditions();
  await user.click(screen.getByRole("button", { name: "ルートを比較する" }));
  await screen.findByRole("group", { name: "表示する経路" });
  return user;
}

async function switchScenario(
  user: ReturnType<typeof userEvent.setup>,
  label: string,
) {
  await user.click(screen.getByText("デモ状態切替", { exact: true }));
  await user.click(screen.getByRole("button", { name: label }));
}

describe("App search flow and condition ownership", () => {
  it("keeps conditions visible and preserves them when the sidebar is hidden and reopened", async () => {
    const user = await openResults();
    const sidebar = screen.getByRole("complementary", { name: "検索条件" });
    expect(
      within(sidebar).getByRole("combobox", { name: "目的地" }),
    ).toHaveValue(DEMO_PLACES[1].label);
    expect(
      within(sidebar).getByRole("radio", { name: "徒歩" }),
    ).toHaveAttribute("aria-checked", "true");
    expect(
      within(sidebar).queryByRole("group", { name: "表示する経路" }),
    ).not.toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "条件サイドバーを閉じる" }),
    );
    expect(sidebar).not.toBeVisible();
    expect(screen.getByRole("group", { name: "表示する経路" })).toBeVisible();
    await user.click(
      screen.getByRole("button", { name: "条件サイドバーを開く" }),
    );
    expect(sidebar).toBeVisible();
    expect(
      within(sidebar).getByRole("combobox", { name: "目的地" }),
    ).toHaveValue(DEMO_PLACES[1].label);
    expect(api.searchRoutes).toHaveBeenCalledTimes(1);
  });

  it("preserves confirmed addresses and manual choices when going back and forward", async () => {
    const user = await openConditions();
    await user.click(screen.getByRole("radio", { name: "自転車" }));
    await user.click(screen.getByRole("radio", { name: /夕 16:00/ }));
    await user.click(screen.getByRole("button", { name: "戻る" }));
    expect(screen.getByRole("combobox", { name: "出発地" })).toHaveValue(
      "東京駅（現在地デモ）",
    );
    expect(screen.getByRole("combobox", { name: "目的地" })).toHaveValue(
      DEMO_PLACES[1].label,
    );
    expect(screen.getByRole("button", { name: "条件を選ぶ" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "条件を選ぶ" }));
    expect(screen.getByRole("radio", { name: "自転車" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByRole("radio", { name: /夕 16:00/ })).toHaveAttribute(
      "aria-checked",
      "true",
    );
  });

  it("keeps a manual weather choice made while the automatic request is pending", async () => {
    const user = await openConditions();
    const pending = deferred<ConditionsResponse>();
    api.getConditions.mockImplementationOnce(() => pending.promise);
    await user.click(screen.getByRole("radio", { name: "日時を指定" }));
    await screen.findByText("条件を取得しています…");
    await user.click(screen.getByRole("radio", { name: "雨" }));
    await act(async () =>
      pending.resolve(
        createDemoConditions("2026-10-24T12:00+09:00", DEMO_PLACES[0]),
      ),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "ルートを比較する" }),
      ).toBeEnabled(),
    );
    expect(screen.getByRole("radio", { name: "雨" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByRole("radio", { name: "晴" })).toHaveAttribute(
      "aria-checked",
      "false",
    );
    await user.click(screen.getByRole("button", { name: "ルートを比較する" }));
    await screen.findByRole("group", { name: "表示する経路" });
    expect(api.searchRoutes.mock.calls.at(-1)?.[0]).toMatchObject({
      weather: "雨",
    });
  });

  it("clears old routes immediately on condition edits and sends the new values only on re-search", async () => {
    const user = await openResults();
    expect(
      screen.getByRole("button", { name: "最短時間を選択" }),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("radio", { name: "雨" }));
    expect(
      screen.queryByRole("group", { name: "表示する経路" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "最短時間を選択" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByText("変更した条件で再検索してください。"),
    ).toBeInTheDocument();
    expect(api.searchRoutes).toHaveBeenCalledTimes(1);
    await user.click(
      screen.getByRole("button", { name: "変更した条件で再検索" }),
    );
    await screen.findByRole("group", { name: "表示する経路" });
    expect(api.searchRoutes).toHaveBeenCalledTimes(2);
    expect(api.searchRoutes.mock.calls.at(-1)?.[0]).toMatchObject({
      weather: "雨",
    });
  });

  it("locks inputs without a cancel action while the 15-minute refresh and search are pending", async () => {
    const user = await openConditions();
    const later = Date.now() + 16 * 60_000;
    vi.spyOn(Date, "now").mockReturnValue(later);
    const pending = deferred<ConditionsResponse>();
    api.getConditions.mockImplementationOnce(() => pending.promise);
    await user.click(screen.getByRole("button", { name: "ルートを比較する" }));
    await screen.findByRole("heading", { name: "ルートを検索しています" });
    expect(api.searchRoutes).not.toHaveBeenCalled();
    expect(
      screen.queryByRole("button", { name: /キャンセル/ }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "条件選択に戻る" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "再検索" })).toBeDisabled();
    expect(
      screen.getByRole("button", { name: "AcciLessNavi 地点入力へ" }),
    ).toBeDisabled();
    for (const input of screen.getAllByRole("combobox"))
      expect(input).toBeDisabled();
    await act(async () =>
      pending.resolve(
        createDemoConditions("2026-10-24T12:00+09:00", DEMO_PLACES[0]),
      ),
    );
    await screen.findByRole("group", { name: "表示する経路" });
    expect(api.searchRoutes).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "条件選択に戻る" })).toBeEnabled();
  });

  it("shows why automatic day type and weather were chosen", async () => {
    await openConditions();
    expect(
      await screen.findByText(/自動：(平日|休日)（.+）/),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/自動：晴（Open-Meteo、.+の予報・デモの固定値）/),
    ).toBeInTheDocument();
    expect(screen.getByText(/祝日表の収録範囲外/)).toBeInTheDocument();
  });

  it("waits for recovered automatic conditions after leaving the conditions-error scenario", async () => {
    const user = await openResults();
    await switchScenario(user, "条件取得不可");
    await screen.findByText(/自動項目は指定なしに戻しました/);
    expect(
      screen.queryByRole("group", { name: "表示する経路" }),
    ).not.toBeInTheDocument();
    const pending = deferred<ConditionsResponse>();
    api.getConditions.mockImplementationOnce(() => pending.promise);
    await switchScenario(user, "通常");
    await screen.findByText("条件を取得しています…");
    expect(api.searchRoutes).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "再検索" })).toBeDisabled();
    await act(async () =>
      pending.resolve(
        createDemoConditions("2026-10-24T12:00+09:00", DEMO_PLACES[0]),
      ),
    );
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "再検索" })).toBeEnabled(),
    );
    expect(api.searchRoutes).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole("button", { name: "再検索" }));
    await screen.findByRole("group", { name: "表示する経路" });
    expect(api.searchRoutes.mock.calls.at(-1)?.[0]).toMatchObject({
      period: "昼",
      weather: "晴",
      day_type: "休日",
    });
  });

  it("keeps zero-accident results explicit without presenting an accident-considered route", async () => {
    const user = await openResults();
    await switchScenario(user, "事故0件");
    const group = await screen.findByRole("group", { name: "表示する経路" });
    await waitFor(() =>
      expect(
        within(group).queryByRole("button", { name: /事故履歴を考慮/ }),
      ).not.toBeInTheDocument(),
    );
    expect(
      within(group).getByRole("button", { name: /最短時間/ }),
    ).toBeInTheDocument();
    expect(
      within(group).getByRole("button", { name: /最短時間.*過去事故 0 件/ }),
    ).toBeInTheDocument();
  });

  it("ignores a pending GPS result that arrives after a route search has started", async () => {
    mode.demo = false;
    vi.stubGlobal("isSecureContext", true);
    let onPosition!: PositionCallback;
    Object.defineProperty(navigator, "geolocation", {
      configurable: true,
      value: {
        getCurrentPosition: vi.fn((success: PositionCallback) => {
          onPosition = success;
        }),
      },
    });
    const user = userEvent.setup();
    render(<App />);
    await user.click(
      screen.getByRole("button", { name: "動画をスキップして開始" }),
    );
    for (const [name, query, index] of [
      ["出発地", "東京", 0],
      ["目的地", "上野", 1],
    ] as const) {
      fireEvent.change(screen.getByRole("combobox", { name }), {
        target: { value: query },
      });
      await user.click(screen.getByRole("button", { name: `${name}の候補を検索` }));
      await user.click(
        await screen.findByRole("option", { name: DEMO_PLACES[index].label }),
      );
    }
    await user.click(screen.getByRole("button", { name: "現在地を使う" }));
    expect(onPosition).toBeDefined();
    await user.click(screen.getByRole("button", { name: "条件を選ぶ" }));
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "ルートを比較する" }),
      ).toBeEnabled(),
    );
    const pending = deferred<RouteResponse>();
    api.searchRoutes.mockImplementationOnce(() => pending.promise);
    await user.click(screen.getByRole("button", { name: "ルートを比較する" }));
    await screen.findByRole("heading", { name: "ルートを検索しています" });
    const signal = api.searchRoutes.mock.calls[0][1] as AbortSignal;

    act(() =>
      onPosition({
        coords: { latitude: 35.0, longitude: 139.0, accuracy: 10 },
      } as GeolocationPosition),
    );
    expect(signal.aborted).toBe(false);
    expect(
      screen.getByRole("heading", { name: "ルートを検索しています" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "出発地" })).toHaveValue(
      DEMO_PLACES[0].label,
    );
    expect(screen.queryByText(/現在地を出発地に設定しました/)).toBeNull();
    await act(async () =>
      pending.resolve(
        createDemoRouteResponse(api.searchRoutes.mock.calls[0][0]),
      ),
    );
    await screen.findByRole("group", { name: "表示する経路" });
    expect(api.searchRoutes).toHaveBeenCalledTimes(1);
  });

  it("re-checks health once when the route result arrives before the first health response", async () => {
    const health = deferred<typeof demoHealth>();
    api.getHealth.mockImplementationOnce(() => health.promise);
    api.searchRoutes.mockImplementation((request: RouteRequest) =>
      Promise.resolve({
        ...createDemoRouteResponse(request),
        dataset_id: "route-dataset",
      }),
    );
    await openResults();
    expect(api.getHealth).toHaveBeenCalledTimes(1);
    await act(async () => health.resolve(demoHealth));
    await waitFor(() => expect(api.getHealth).toHaveBeenCalledTimes(2));
    // The refreshed health still differs, but the same mismatch is not checked again.
    await act(async () => {});
    expect(api.getHealth).toHaveBeenCalledTimes(2);
  });

  it("re-checks health once when the route result differs from an already fetched health", async () => {
    const user = await openConditions();
    await waitFor(() => expect(api.getHealth).toHaveBeenCalledTimes(1));
    api.searchRoutes.mockImplementation((request: RouteRequest) =>
      Promise.resolve({
        ...createDemoRouteResponse(request),
        dataset_id: "route-dataset",
      }),
    );
    await user.click(screen.getByRole("button", { name: "ルートを比較する" }));
    await screen.findByRole("group", { name: "表示する経路" });
    await waitFor(() => expect(api.getHealth).toHaveBeenCalledTimes(2));
    await act(async () => {});
    expect(api.getHealth).toHaveBeenCalledTimes(2);
  });
});
