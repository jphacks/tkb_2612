import { useLayoutEffect } from "react";
import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createDemoRouteResponse, DEMO_PLACES } from "../api/demo";
import { ApiError } from "../api/errors";
import type { ConditionValues, RouteResponse } from "../lib/types";
import { RESEARCH_INTERRUPTED, NO_SAFEST, TOO_CLOSE, useReSearch } from "./useReSearch";
import type { NavigationControl, ReSearchSource } from "./useNavigation";
import type { useConditions } from "./useConditions";
import { useRouteSearch } from "./useRouteSearch";

const client = vi.hoisted(() => ({ searchRoutes: vi.fn() }));
vi.mock("../api/client", () => client);

const dest = DEMO_PLACES[1];
const request = {
  origin: DEMO_PLACES[0],
  dest,
  mode: "bike" as const,
  period: "昼" as const,
  weather: "晴" as const,
  day_type: "平日" as const,
  max_detour_min: 10,
};
const values: ConditionValues = { mode: "bike", period: "夕", day_type: "休日", weather: "雨", max_detour_min: 6 };
const here = { lat: 35.69, lon: 139.74, accuracy: 12, timestamp: 1_000 };

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((accept, decline) => {
    resolve = accept;
    reject = decline;
  });
  return { promise, resolve, reject };
}

function geolocation() {
  const getCurrentPosition = vi.fn();
  Object.defineProperty(navigator, "geolocation", {
    configurable: true,
    value: { getCurrentPosition, watchPosition: vi.fn(), clearWatch: vi.fn() },
  });
  return {
    getCurrentPosition,
    succeed: (fix: { lat: number; lon: number; accuracy: number; timestamp?: number }) =>
      getCurrentPosition.mock.calls.at(-1)![0]({
        coords: { latitude: fix.lat, longitude: fix.lon, accuracy: fix.accuracy },
        timestamp: fix.timestamp ?? Date.now(),
      }),
    fail: (code: number) =>
      getCurrentPosition.mock.calls.at(-1)![1]({ code, PERMISSION_DENIED: 1, POSITION_UNAVAILABLE: 2, TIMEOUT: 3 }),
  };
}

function setup(source: Partial<ReSearchSource> = {}, beforeResume?: () => void) {
  const taken: ReSearchSource = {
    kind: "device",
    selection: "safest",
    simulation: null,
    fix: { lon: here.lon, lat: here.lat, accuracy: here.accuracy, timestamp: Date.now() },
    fixFresh: true,
    ...source,
  };
  const navigation = {
    takeForReSearch: vi.fn(() => taken),
    start: vi.fn(),
  } as unknown as NavigationControl;
  const refresh = vi.fn(async (..._args: unknown[]) => values);
  const conditions = { refreshForReSearch: refresh } as unknown as ReturnType<typeof useConditions>;
  const onAdoptOrigin = vi.fn();
  const onRouteSuccess = vi.fn();
  const view = renderHook(() => {
    const route = useRouteSearch();
    const reSearch = useReSearch({
      navigation,
      route,
      conditions,
      dest,
      scenario: "normal",
      onAdoptOrigin,
      onRouteSuccess,
    });
    useLayoutEffect(() => {
      if (route.result) beforeResume?.();
    }, [route.result]);
    return { route, reSearch };
  });
  return { ...view, navigation, refresh, onAdoptOrigin, onRouteSuccess, taken };
}

function setVisibility(state: "hidden" | "visible") {
  Object.defineProperty(document, "visibilityState", { configurable: true, value: state });
  document.dispatchEvent(new Event("visibilitychange"));
}

beforeEach(() => {
  vi.stubEnv("VITE_ENABLE_SIMULATION", "true");
  client.searchRoutes.mockReset().mockImplementation((req: typeof request) =>
    Promise.resolve(createDemoRouteResponse(req)),
  );
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllEnvs();
  Reflect.deleteProperty(navigator, "geolocation");
  Reflect.deleteProperty(document, "visibilityState");
  vi.restoreAllMocks();
});

const state = (view: ReturnType<typeof setup>) => view.result.current.reSearch.state;

describe("useReSearch: 現在地からの明示的再検索（7.6）", () => {
  it("新しい測位が不要なら、未補正の位置をoriginにして条件と経路を1回ずつ取得し、同種別のナビを継続する", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(here.timestamp + 2000);
    const geo = geolocation();
    const view = setup();
    await act(async () => view.result.current.reSearch.start());
    vi.useRealTimers();
    await waitFor(() => expect(view.navigation.start).toHaveBeenCalledTimes(1));
    expect(geo.getCurrentPosition).not.toHaveBeenCalled();
    expect(view.refresh).toHaveBeenCalledTimes(1);
    expect(view.refresh.mock.calls[0].slice(0, 2)).toEqual([
      { label: "現在地", lat: here.lat, lon: here.lon },
      dest,
    ]);
    expect(client.searchRoutes).toHaveBeenCalledTimes(1);
    expect(client.searchRoutes.mock.calls[0][0]).toEqual({
      origin: { lat: here.lat, lon: here.lon },
      dest: { lat: dest.lat, lon: dest.lon },
      ...values,
    });
    expect(view.onAdoptOrigin).toHaveBeenCalledWith({ label: "現在地", lat: here.lat, lon: here.lon });
    expect(view.navigation.start).toHaveBeenCalledWith(
      expect.objectContaining({ selection: "safest", kind: "device" }),
    );
    expect(vi.mocked(view.navigation.start).mock.calls[0][0].forResult).toBe(view.result.current.route.result);
    expect(state(view)).toEqual({ status: "idle" });
  });

  it("測位が古いと実機は新しく1回測位する（7.1のオプション）。経路上の位置は使わない", async () => {
    const geo = geolocation();
    const view = setup({ fixFresh: false });
    await act(async () => view.result.current.reSearch.start());
    expect(geo.getCurrentPosition).toHaveBeenCalledTimes(1);
    expect(geo.getCurrentPosition.mock.calls[0][2]).toEqual({
      enableHighAccuracy: true,
      timeout: 10000,
      maximumAge: 0,
    });
    expect(state(view)).toMatchObject({ status: "searching", stage: "locating" });
    expect(view.refresh).not.toHaveBeenCalled();
    await act(async () => geo.succeed({ lat: 35.7, lon: 139.75, accuracy: 9 }));
    await waitFor(() => expect(view.navigation.start).toHaveBeenCalled());
    expect(client.searchRoutes.mock.calls[0][0].origin).toEqual({ lat: 35.7, lon: 139.75 });
  });

  it.each([
    ["精度不足", (geo: ReturnType<typeof geolocation>) => geo.succeed({ lat: 35.7, lon: 139.75, accuracy: 80 }), "位置の精度が低い"],
    ["権限拒否", (geo: ReturnType<typeof geolocation>) => geo.fail(1), "位置情報の利用が許可されていません"],
    ["timeout", (geo: ReturnType<typeof geolocation>) => geo.fail(3), "タイムアウト"],
  ])("位置を取得できない（%s）ときは送信せず停止状態にする", async (_name, respond, text) => {
    const geo = geolocation();
    const view = setup({ fix: null, fixFresh: false });
    await act(async () => view.result.current.reSearch.start());
    await act(async () => respond(geo));
    expect(state(view)).toMatchObject({ status: "failed" });
    expect((state(view) as { message: string }).message).toContain(text);
    expect(view.refresh).not.toHaveBeenCalled();
    expect(client.searchRoutes).not.toHaveBeenCalled();
    expect(view.navigation.start).not.toHaveBeenCalled();
  });

  it.each(["古い", "未来", "NaN"])("再取得測位のtimestampが%sならconditions・routeを送信しない", async (kind) => {
    const geo = geolocation();
    const view = setup({ fixFresh: false });
    await act(async () => view.result.current.reSearch.start());
    const timestamp = kind === "古い" ? Date.now() - 60_000 : kind === "未来" ? Date.now() + 60_000 : NaN;
    await act(async () => geo.succeed({ ...here, timestamp }));
    expect(state(view).status).toBe("failed");
    expect(view.refresh).not.toHaveBeenCalled();
    expect(client.searchRoutes).not.toHaveBeenCalled();
    expect(view.navigation.start).not.toHaveBeenCalled();
    expect(view.onAdoptOrigin).not.toHaveBeenCalled();
  });

  it.each(["古い", "未来", "NaN"])("再試行の既存originのtimestampが%sなら再測位し、応答前に送信しない", async (kind) => {
    vi.useFakeTimers({ toFake: ["Date"] });
    const geo = geolocation();
    client.searchRoutes.mockRejectedValueOnce(new ApiError("混雑", "service_busy", 503));
    const fix = { ...here, timestamp: Date.now() };
    const view = setup({ fix });
    await act(async () => view.result.current.reSearch.start());
    expect(state(view).status).toBe("failed");
    if (kind === "古い") vi.setSystemTime(Date.now() + 5001);
    else if (kind === "未来") vi.setSystemTime(Date.now() - 1000);
    else fix.timestamp = NaN;
    await act(async () => view.result.current.reSearch.retry());
    expect(geo.getCurrentPosition).toHaveBeenCalledTimes(1);
    expect(view.refresh).toHaveBeenCalledTimes(1);
    expect(client.searchRoutes).toHaveBeenCalledTimes(1);
    await act(async () => geo.succeed({ ...here, timestamp: Date.now() }));
    expect(view.navigation.start).toHaveBeenCalledTimes(1);
    expect(view.refresh).toHaveBeenCalledTimes(2);
    expect(client.searchRoutes).toHaveBeenCalledTimes(2);
  });

  it("模擬は位置の鮮度に関わらず停止位置を使い、GPSを呼ばず、模擬位置としてoriginにする", async () => {
    const geo = geolocation();
    const view = setup({
      kind: "simulation",
      selection: "fastest",
      simulation: { seed: 7, multiplier: 4, jitter: false },
      fixFresh: false,
    });
    await act(async () => view.result.current.reSearch.start());
    await waitFor(() => expect(view.navigation.start).toHaveBeenCalled());
    expect(geo.getCurrentPosition).not.toHaveBeenCalled();
    expect(view.onAdoptOrigin).toHaveBeenCalledWith({ label: "模擬位置", lat: here.lat, lon: here.lon });
    expect(view.navigation.start).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "simulation", simulation: { seed: 7, multiplier: 4, jitter: false } }),
    );
  });

  it("目的地と20m未満なら検索できない旨を示し、conditionsもrouteも送らない", async () => {
    const view = setup({ fix: { lon: dest.lon + 0.0001, lat: dest.lat, accuracy: 10, timestamp: Date.now() }, fixFresh: true });
    await act(async () => view.result.current.reSearch.start());
    await waitFor(() => expect(state(view).status).toBe("failed"));
    expect((state(view) as { message: string }).message).toBe(TOO_CLOSE);
    expect(view.refresh).not.toHaveBeenCalled();
    expect(client.searchRoutes).not.toHaveBeenCalled();
  });

  it("事故考慮経路がなければ最短へ自動切替せず、ナビを再開しないで案内する", async () => {
    client.searchRoutes.mockImplementation((req: typeof request) =>
      Promise.resolve(createDemoRouteResponse(req, "zero")),
    );
    const view = setup();
    await act(async () => view.result.current.reSearch.start());
    await waitFor(() => expect(view.result.current.reSearch.notice).toBe(NO_SAFEST));
    expect(view.navigation.start).not.toHaveBeenCalled();
    expect(view.result.current.route.result?.safest_id).toBeNull();
  });

  it("503では停止したまま手動再試行を提示し、自動再送しない。再試行は条件取得から1回ずつやり直す", async () => {
    client.searchRoutes.mockRejectedValueOnce(new ApiError("混雑", "service_busy", 503));
    const view = setup({ kind: "simulation", simulation: { seed: 1, multiplier: 1, jitter: true } });
    await act(async () => view.result.current.reSearch.start());
    await waitFor(() => expect(state(view).status).toBe("failed"));
    expect((state(view) as { message: string }).message).toContain("サーバーが混み合っています");
    expect(view.result.current.route.error).toBeNull();
    expect(view.navigation.start).not.toHaveBeenCalled();
    await new Promise((resolve) => setTimeout(resolve, 60));
    expect(client.searchRoutes).toHaveBeenCalledTimes(1);
    expect(view.refresh).toHaveBeenCalledTimes(1);
    await act(async () => view.result.current.reSearch.retry());
    await waitFor(() => expect(view.navigation.start).toHaveBeenCalledTimes(1));
    expect(view.refresh).toHaveBeenCalledTimes(2);
    expect(client.searchRoutes).toHaveBeenCalledTimes(2);
  });

  it("非JSON・IAP失敗では再読込（再ログイン）を案内して停止する", async () => {
    client.searchRoutes.mockRejectedValueOnce(new ApiError("応答不正", "invalid_response", 200));
    const view = setup();
    await act(async () => view.result.current.reSearch.start());
    await waitFor(() => expect(state(view).status).toBe("failed"));
    expect((state(view) as { message: string }).message).toContain("再読み込み");
    expect(view.navigation.start).not.toHaveBeenCalled();
  });

  it("再試行で実機のoriginが古ければ、測位を取り直してから送信する", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    const geo = geolocation();
    client.searchRoutes.mockRejectedValueOnce(new ApiError("混雑", "service_busy", 503));
    const view = setup();
    await act(async () => view.result.current.reSearch.start());
    await waitFor(() => expect(state(view).status).toBe("failed"));
    vi.setSystemTime(Date.now() + 5001);
    await act(async () => view.result.current.reSearch.retry());
    expect(geo.getCurrentPosition).toHaveBeenCalledTimes(1);
    expect(client.searchRoutes).toHaveBeenCalledTimes(1);
    await act(async () => geo.succeed({ lat: 35.71, lon: 139.76, accuracy: 8 }));
    await waitFor(() => expect(view.navigation.start).toHaveBeenCalled());
    expect(client.searchRoutes.mock.calls[1][0].origin).toEqual({ lat: 35.71, lon: 139.76 });
  });

  it("失敗後の入力編集（discard）でsnapshotを破棄し、再試行しても旧origin・条件・種別は復活しない", async () => {
    client.searchRoutes.mockRejectedValueOnce(new ApiError("混雑", "service_busy", 503));
    const view = setup();
    await act(async () => view.result.current.reSearch.start());
    await waitFor(() => expect(state(view).status).toBe("failed"));
    act(() => view.result.current.reSearch.discard());
    expect(state(view)).toEqual({ status: "idle" });
    await act(async () => view.result.current.reSearch.retry());
    expect(client.searchRoutes).toHaveBeenCalledTimes(1);
    expect(view.refresh).toHaveBeenCalledTimes(1);
  });

  it("二重の開始操作では要求を重ねない", async () => {
    const pending = deferred<RouteResponse>();
    client.searchRoutes.mockReturnValue(pending.promise);
    const view = setup();
    await act(async () => {
      view.result.current.reSearch.start();
      view.result.current.reSearch.retry();
    });
    await waitFor(() => expect(client.searchRoutes).toHaveBeenCalledTimes(1));
    await act(async () => view.result.current.reSearch.retry());
    expect(client.searchRoutes).toHaveBeenCalledTimes(1);
    expect(view.refresh).toHaveBeenCalledTimes(1);
  });
});

describe("useReSearch: hidden/pagehide と unmount", () => {
  it.each(["hidden", "pagehide"] as const)("成功後の再開前に%sになると一度だけ中断し、復帰でも自動開始・追加送信しない", async (event) => {
    const view = setup({}, () => {
      if (event === "hidden") setVisibility("hidden");
      else window.dispatchEvent(new Event("pagehide"));
    });
    await act(async () => view.result.current.reSearch.start());
    expect(state(view)).toEqual({ status: "interrupted", kind: "device" });
    expect(view.navigation.start).not.toHaveBeenCalled();
    expect(view.result.current.route.result).toBeNull();
    act(() => window.dispatchEvent(new Event("pagehide")));
    act(() => setVisibility("visible"));
    act(() => window.dispatchEvent(new Event("pageshow")));
    expect(state(view).status).toBe("interrupted");
    expect(view.navigation.start).not.toHaveBeenCalled();
    expect(view.refresh).toHaveBeenCalledTimes(1);
    expect(client.searchRoutes).toHaveBeenCalledTimes(1);
  });

  it("位置取得中のhiddenで中断へ一度だけ遷移し、遅い成功を捨て、visibleの復帰だけでは送信しない", async () => {
    const geo = geolocation();
    const view = setup({ fixFresh: false });
    await act(async () => view.result.current.reSearch.start());
    act(() => setVisibility("hidden"));
    act(() => window.dispatchEvent(new Event("pagehide")));
    expect(state(view)).toEqual({ status: "interrupted", kind: "device" });
    await act(async () => geo.succeed({ lat: 35.7, lon: 139.75, accuracy: 8 }));
    act(() => setVisibility("visible"));
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(view.refresh).not.toHaveBeenCalled();
    expect(client.searchRoutes).not.toHaveBeenCalled();
    expect(view.navigation.start).not.toHaveBeenCalled();
    expect(state(view).status).toBe("interrupted");
    expect(RESEARCH_INTERRUPTED).toContain("再検索を中断しました");
  });

  it("conditions取得中のhiddenでabortし、遅い成功でrouteを送らない。確定済みの新originは保持する", async () => {
    const pending = deferred<ConditionValues>();
    const view = setup({ kind: "simulation", simulation: { seed: 1, multiplier: 1, jitter: true } });
    view.refresh.mockImplementation(() => pending.promise);
    await act(async () => view.result.current.reSearch.start());
    const signal = view.refresh.mock.calls[0][2] as AbortSignal;
    act(() => setVisibility("hidden"));
    expect(signal.aborted).toBe(true);
    expect(state(view).status).toBe("interrupted");
    expect(view.onAdoptOrigin).toHaveBeenCalledTimes(1);
    await act(async () => pending.resolve(values));
    expect(client.searchRoutes).not.toHaveBeenCalled();
    // 手動の再試行だけが新しい世代で送信する。
    await act(async () => {
      view.refresh.mockResolvedValue(values);
      view.result.current.reSearch.retry();
    });
    await waitFor(() => expect(client.searchRoutes).toHaveBeenCalledTimes(1));
  });

  it("route取得中のhiddenでabortし、遅い成功を反映せずナビも再開しない", async () => {
    const pending = deferred<RouteResponse>();
    let signal!: AbortSignal;
    client.searchRoutes.mockImplementation((_req: unknown, received: AbortSignal) => {
      signal = received;
      return pending.promise;
    });
    const view = setup();
    await act(async () => view.result.current.reSearch.start());
    await waitFor(() => expect(client.searchRoutes).toHaveBeenCalled());
    act(() => setVisibility("hidden"));
    expect(signal.aborted).toBe(true);
    expect(state(view).status).toBe("interrupted");
    await act(async () => pending.resolve(createDemoRouteResponse(request)));
    expect(view.result.current.route.result).toBeNull();
    expect(view.navigation.start).not.toHaveBeenCalled();
    expect(view.onRouteSuccess).not.toHaveBeenCalled();
  });

  it("unmountで要求をabortし、遅い旧応答でナビを開始しない", async () => {
    const pending = deferred<RouteResponse>();
    let signal!: AbortSignal;
    client.searchRoutes.mockImplementation((_req: unknown, received: AbortSignal) => {
      signal = received;
      return pending.promise;
    });
    const view = setup();
    await act(async () => view.result.current.reSearch.start());
    await waitFor(() => expect(client.searchRoutes).toHaveBeenCalled());
    view.unmount();
    expect(signal.aborted).toBe(true);
    await pending.resolve(createDemoRouteResponse(request));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(view.navigation.start).not.toHaveBeenCalled();
  });
});
