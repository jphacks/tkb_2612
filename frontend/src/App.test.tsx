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
  Scenario,
} from "./lib/types";

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
vi.mock("./components/RouteMap", () => ({
  RouteMap: ({ result }: { result: unknown }) => (
    <div aria-label="地図プレビュー">
      {result ? "地図に経路があります" : "地図に経路はありません"}
    </div>
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

afterEach(() => vi.restoreAllMocks());

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
    expect(
      screen.getByRole("radio", { name: "雨" }),
    ).toHaveAttribute("aria-checked", "true");
    expect(
      screen.getByRole("radio", { name: "晴" }),
    ).toHaveAttribute("aria-checked", "false");
    await user.click(screen.getByRole("button", { name: "ルートを比較する" }));
    await screen.findByRole("group", { name: "表示する経路" });
    expect(api.searchRoutes.mock.calls.at(-1)?.[0]).toMatchObject({
      weather: "雨",
    });
  });

  it("clears old routes immediately on condition edits and sends the new values only on re-search", async () => {
    const user = await openResults();
    expect(screen.getByText("地図に経路があります")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "条件を編集" }));
    await user.click(screen.getByRole("radio", { name: "雨" }));
    expect(
      screen.queryByRole("group", { name: "表示する経路" }),
    ).not.toBeInTheDocument();
    expect(screen.getByText("地図に経路はありません")).toBeInTheDocument();
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

  it("unblocks the condition form after canceling the 15-minute refresh by navigating back", async () => {
    const user = await openConditions();
    const later = Date.now() + 16 * 60_000;
    vi.spyOn(Date, "now").mockReturnValue(later);
    api.getConditions.mockImplementationOnce(
      (
        _datetime: string,
        _origin: LatLon,
        _dest: LatLon,
        signal: AbortSignal,
      ) =>
        new Promise((_resolve, reject) => {
          signal.addEventListener(
            "abort",
            () => reject(new DOMException("cancelled", "AbortError")),
            { once: true },
          );
        }),
    );
    await user.click(screen.getByRole("button", { name: "ルートを比較する" }));
    await screen.findByRole("heading", { name: "ルートを検索しています" });
    expect(api.searchRoutes).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "条件選択に戻る" }));
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "ルートを比較する" }),
      ).toBeEnabled(),
    );
    await user.click(screen.getByRole("button", { name: "ルートを比較する" }));
    await screen.findByRole("group", { name: "表示する経路" });
    expect(api.searchRoutes).toHaveBeenCalledTimes(1);
  });

  it("waits for recovered automatic conditions after leaving the conditions-error scenario", async () => {
    const user = await openResults();
    await user.click(screen.getByRole("button", { name: "条件を編集" }));
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
    expect(
      screen.getByRole("button", { name: "再検索" }),
    ).toBeDisabled();
    await act(async () =>
      pending.resolve(
        createDemoConditions("2026-10-24T12:00+09:00", DEMO_PLACES[0]),
      ),
    );
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "再検索" }),
      ).toBeEnabled(),
    );
    expect(api.searchRoutes).toHaveBeenCalledTimes(1);
    await user.click(
      screen.getByRole("button", { name: "再検索" }),
    );
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
      screen.getByText(/事故0件は安全の証明ではありません/),
    ).toBeInTheDocument();
  });
});
