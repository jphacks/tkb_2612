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

async function openSidebar(user: ReturnType<typeof userEvent.setup>) {
  await user.click(
    screen.getByRole("button", { name: "条件サイドバーを開く" }),
  );
}

async function openResults() {
  const user = await openConditions();
  await user.click(screen.getByRole("button", { name: "ルートを比較する" }));
  await screen.findByRole("group", { name: "表示する経路" });
  // The result page opens with the sidebar closed.
  expect(
    screen.getByRole("button", { name: "条件サイドバーを開く" }),
  ).toHaveAttribute("aria-expanded", "false");
  await openSidebar(user);
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
    const sidebar = screen.getByRole("complementary", {
      name: "検索結果と条件",
    });
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
      screen.getByRole("button", { name: "最短ルートを選択" }),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("radio", { name: "雨" }));
    expect(
      screen.queryByRole("group", { name: "表示する経路" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "最短ルートを選択" }),
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
    await openSidebar(user);
    expect(api.searchRoutes).not.toHaveBeenCalled();
    expect(
      screen.queryByRole("button", { name: /キャンセル/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "条件選択に戻る" }),
    ).toBeDisabled();
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
    expect(
      screen.getByRole("button", { name: "条件選択に戻る" }),
    ).toBeEnabled();
  });

  it("shows why the automatic day type was chosen", async () => {
    await openConditions();
    expect(
      await screen.findByText(/自動：(平日|休日)（.+）/),
    ).toBeInTheDocument();
    // The weather supplement (source and model note) is no longer shown.
    expect(screen.queryByText(/自動：晴（Open-Meteo/)).not.toBeInTheDocument();
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
        within(group).queryByRole("button", { name: /^安全ルート(?!でナビ)/ }),
      ).not.toBeInTheDocument(),
    );
    expect(
      within(group).getByRole("button", { name: /^最短ルート(?!でナビ)/ }),
    ).toBeInTheDocument();
    expect(
      within(group).getByRole("button", { name: /最短ルート.*過去事故 0 件/ }),
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
      await user.click(
        screen.getByRole("button", { name: `${name}の候補を検索` }),
      );
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
    await openSidebar(user);
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

  it("keeps the health warning and manual re-check on the result page when the dataset re-check fails", async () => {
    const user = await openConditions();
    await waitFor(() => expect(api.getHealth).toHaveBeenCalledTimes(1));
    api.getHealth
      .mockRejectedValueOnce(
        new ApiError("サーバーの説明文", "data_unavailable", 503),
      )
      .mockResolvedValueOnce(demoHealth);
    api.searchRoutes.mockImplementation((request: RouteRequest) =>
      Promise.resolve({
        ...createDemoRouteResponse(request),
        dataset_id: "route-dataset",
      }),
    );
    await user.click(screen.getByRole("button", { name: "ルートを比較する" }));
    await screen.findByRole("group", { name: "表示する経路" });
    await openSidebar(user);
    expect(
      await screen.findByText(
        "事故データを準備しています。時間をおいて再試行してください。",
      ),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "再検索" })).toBeDisabled();

    await user.click(screen.getByRole("radio", { name: "雨" }));
    expect(
      screen.queryByRole("group", { name: "表示する経路" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByText(
        "事故データを準備しています。時間をおいて再試行してください。",
      ),
    ).toBeVisible();
    expect(
      screen.getByRole("button", { name: "変更した条件で再検索" }),
    ).toBeDisabled();

    await user.click(screen.getByRole("button", { name: "準備状況を再確認" }));
    await waitFor(() =>
      expect(
        screen.queryByText(
          "事故データを準備しています。時間をおいて再試行してください。",
        ),
      ).toBeNull(),
    );
    expect(api.getHealth).toHaveBeenCalledTimes(3);
    expect(screen.getByRole("button", { name: "再検索" })).toBeEnabled();
    await user.click(
      screen.getByRole("button", { name: "変更した条件で再検索" }),
    );
    await screen.findByRole("group", { name: "表示する経路" });
    expect(api.searchRoutes).toHaveBeenCalledTimes(2);
  });
});

describe("App result comparison and failures (frontend-spec 5, 10)", () => {
  it("shows the aggregation settings and all candidates in the sidebar and syncs the selection with the map", async () => {
    const user = await openResults();
    const sidebar = screen.getByRole("complementary", {
      name: "検索結果と条件",
    });
    const response: RouteResponse =
      await api.searchRoutes.mock.results.at(-1)?.value;
    await user.click(within(sidebar).getByText("集計条件と全候補"));
    expect(
      within(sidebar).getByText(`${response.data_years.join("・")}年`),
    ).toBeVisible();
    const fastestIndex = response.candidates.findIndex(
      (route) => route.id === response.fastest_id,
    );
    await user.click(
      within(sidebar).getByRole("button", { name: `候補${fastestIndex + 1}` }),
    );
    expect(
      screen.getByRole("button", { name: "最短ルートを選択" }),
    ).toHaveAttribute("aria-pressed", "true");

    // Editing a condition clears the table together with the result.
    await user.click(within(sidebar).getByRole("radio", { name: "雨" }));
    expect(
      within(sidebar).queryByText("集計条件と全候補"),
    ).not.toBeInTheDocument();
  });

  it.each([
    ["no_route", 502, /道路に近い地点/],
    ["route_too_long", 422, /80km.*近づけて/],
    ["service_busy", 503, /混み合って/],
    ["invalid_response", undefined, /再読み込み/],
    [
      "brand_new_code",
      500,
      /^未知の失敗です。 時間をおいて再試行してください。$/,
    ],
  ])(
    "shows the %s failure by code with a manual retry and keeps the inputs",
    async (code, status, expected) => {
      const user = await openConditions();
      api.searchRoutes.mockRejectedValueOnce(
        new ApiError("未知の失敗です。", code, status),
      );
      await user.click(
        screen.getByRole("button", { name: "ルートを比較する" }),
      );
      const alert = await screen.findByRole("alert");
      expect(within(alert).getByText(expected)).toBeInTheDocument();
      expect(
        within(alert).getByRole("button", { name: "再試行" }),
      ).toBeEnabled();
      await openSidebar(user);
      expect(screen.getByRole("combobox", { name: "目的地" })).toHaveValue(
        DEMO_PLACES[1].label,
      );
    },
  );

  it("always shows live data attribution, including the geocode providers", async () => {
    mode.demo = false;
    api.searchPlaces.mockImplementation((query: string) =>
      Promise.resolve({
        ...findDemoPlaces(query),
        attribution: [
          { text: "Powered by Geoapify", url: "https://www.geoapify.com/" },
        ],
      }),
    );
    const user = userEvent.setup();
    render(<App />);
    await user.click(
      screen.getByRole("button", { name: "動画をスキップして開始" }),
    );
    const footer = screen.getByRole("contentinfo", { name: "データの出典" });
    await waitFor(() =>
      expect(
        within(footer).getByText(
          new RegExp(`（${demoHealth.data_years.join("・")}年）`),
        ),
      ).toBeInTheDocument(),
    );
    expect(
      within(footer).getByRole("link", {
        name: "Weather data by Open-Meteo.com",
      }),
    ).toBeInTheDocument();
    const input = screen.getByRole("combobox", { name: "目的地" });
    fireEvent.change(input, { target: { value: "上野" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() =>
      expect(
        within(footer).getByRole("link", { name: "Powered by Geoapify" }),
      ).toHaveAttribute("href", "https://www.geoapify.com/"),
    );
  });

  it("keeps live attribution visible on the result page when the sidebar is closed", async () => {
    const user = await openResults();
    // The helper uses the demo origin; the next render reads live mode.
    mode.demo = false;
    await user.click(
      screen.getByRole("button", { name: "条件サイドバーを閉じる" }),
    );
    const footer = screen.getByRole("contentinfo", { name: "データの出典" });
    expect(footer).toBeVisible();
    expect(
      within(footer).getByText(/過去の事故記録に基づく参考指標/),
    ).toBeVisible();
    expect(
      within(footer).getByRole("link", {
        name: "Weather data by Open-Meteo.com",
      }),
    ).toBeVisible();
  });

  it("explains a health network failure with the possible expired login", async () => {
    api.getHealth
      .mockReset()
      .mockRejectedValue(
        new ApiError("サーバーに接続できません。", "network_error"),
      );
    await openConditions();
    expect(
      await screen.findByText(/ログインの有効期限が切れた可能性があります/),
    ).toBeInTheDocument();
  });
});
