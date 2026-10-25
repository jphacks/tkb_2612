import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createDemoRouteResponse, DEMO_PLACES } from "../api/demo";
import { installGeolocation, positionAt } from "../test/navigationApp";
import { useNavigation } from "./useNavigation";

const request = {
  origin: DEMO_PLACES[0],
  dest: DEMO_PLACES[1],
  mode: "walk" as const,
  period: "昼" as const,
  weather: "晴" as const,
  day_type: "平日" as const,
  max_detour_min: 10,
};
const result = createDemoRouteResponse(request);
const fastest = result.candidates.find((route) => route.id === result.fastest_id)!;

function setVisibility(state: "hidden" | "visible") {
  Object.defineProperty(document, "visibilityState", { configurable: true, value: state });
  document.dispatchEvent(new Event("visibilitychange"));
}

function wakeLockApi(outcome: "granted" | "denied" = "granted") {
  const listeners: (() => void)[] = [];
  const release = vi.fn().mockResolvedValue(undefined);
  const request = vi.fn(async () => {
    if (outcome === "denied") throw new Error("NotAllowedError");
    return { release, addEventListener: (_: string, listener: () => void) => listeners.push(listener) };
  });
  Object.defineProperty(navigator, "wakeLock", { configurable: true, value: { request } });
  return { request, release, osRelease: () => listeners.forEach((listener) => listener()) };
}

function setup(mapAvailable = true) {
  return renderHook((props: { mapAvailable: boolean }) => useNavigation({ result, ...props }), {
    initialProps: { mapAvailable },
  });
}

const startDevice = (control: ReturnType<typeof setup>["result"]) =>
  act(() => control.current.start({ forResult: result, selection: "fastest", kind: "device" }));

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-10-25T03:00:00Z"));
});
afterEach(() => {
  vi.useRealTimers();
  Reflect.deleteProperty(navigator, "geolocation");
  Reflect.deleteProperty(navigator, "wakeLock");
  Reflect.deleteProperty(document, "visibilityState");
  vi.restoreAllMocks();
});

describe("useNavigation: 位置源の排他・ライフサイクル（手順11）", () => {
  it("実機と模擬は同時に起動しない。2回目の開始は無視する", async () => {
    const gps = installGeolocation();
    const { result: control } = setup();
    act(() =>
      control.current.start({
        forResult: result,
        selection: "fastest",
        kind: "simulation",
        simulation: { seed: 1, multiplier: 1, jitter: true },
      }),
    );
    startDevice(control);
    expect(control.current.snapshot?.kind).toBe("simulation");
    expect(gps.watchPosition).not.toHaveBeenCalled();
  });

  it("hidden・pagehideでclearWatch・音声取消・Wake Lock解放を行い、visible/pageshowでは自動再開しない", async () => {
    const gps = installGeolocation();
    const wake = wakeLockApi();
    const { result: control } = setup();
    startDevice(control);
    await act(async () => {
      await Promise.resolve();
    });
    expect(control.current.snapshot?.wakeLock).toBe("held");
    expect(wake.request).toHaveBeenCalledTimes(1);
    act(() => setVisibility("hidden"));
    expect(gps.clearWatch).toHaveBeenCalledTimes(1);
    expect(wake.release).toHaveBeenCalledTimes(1);
    expect(control.current.snapshot).toMatchObject({ phase: "paused", pauseReason: "hidden" });
    act(() => {
      setVisibility("visible");
      window.dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true }));
    });
    vi.advanceTimersByTime(60_000);
    expect(gps.watchPosition).toHaveBeenCalledTimes(1);
    expect(wake.request).toHaveBeenCalledTimes(1);
    expect(control.current.snapshot?.phase).toBe("paused");
    // 明示再開だけがwatchとWake Lockを取り直す。
    act(() => control.current.resume());
    await act(async () => {
      await Promise.resolve();
    });
    expect(gps.watchPosition).toHaveBeenCalledTimes(2);
    expect(wake.request).toHaveBeenCalledTimes(2);
    expect(control.current.snapshot?.phase).toBe("waiting");
  });

  it("pagehide後に届いた古い測位は捨てる", () => {
    const gps = installGeolocation();
    const { result: control } = setup();
    startDevice(control);
    gps.emit(positionAt(fastest, 100));
    const before = control.current.snapshot?.lastFix;
    act(() => {
      window.dispatchEvent(new Event("pagehide"));
    });
    expect(control.current.snapshot?.pauseReason).toBe("hidden");
    gps.emit(positionAt(fastest, 300));
    expect(control.current.snapshot?.lastFix).toEqual(before);
  });

  it("権限拒否では実機ナビを停止し、watchを再登録せず、明示再開でだけ登録し直す", () => {
    const gps = installGeolocation();
    const { result: control } = setup();
    startDevice(control);
    gps.fail(1);
    expect(control.current.snapshot).toMatchObject({ phase: "paused", pauseReason: "permission-denied" });
    expect(gps.clearWatch).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(60_000);
    expect(gps.watchPosition).toHaveBeenCalledTimes(1);
    act(() => control.current.resume());
    expect(gps.watchPosition).toHaveBeenCalledTimes(2);
  });

  it("POSITION_UNAVAILABLE・timeoutは判定保留のまま続け、watchの再登録ループを作らない", () => {
    const gps = installGeolocation();
    const { result: control } = setup();
    startDevice(control);
    for (const code of [2, 3, 2, 3] as const) {
      gps.fail(code);
      vi.advanceTimersByTime(1000);
    }
    expect(gps.watchPosition).toHaveBeenCalledTimes(1);
    expect(control.current.snapshot?.sourceError).toBe("timeout");
    act(() => {
      vi.advanceTimersByTime(7000);
    });
    expect(control.current.snapshot?.phase).toBe("poor-accuracy");
    // 既存のwatchで良好測位が戻れば確認から復帰できる。
    for (let i = 0; i < 8; i++) {
      vi.advanceTimersByTime(1000);
      gps.emit(positionAt(fastest, 100 + i));
    }
    expect(control.current.snapshot?.phase).toBe("tracking");
    expect(control.current.snapshot?.sourceError).toBeNull();
  });

  it("Wake Lockが拒否されても利用でき、解除されても自動で取り直さない", async () => {
    installGeolocation();
    const wake = wakeLockApi("denied");
    const { result: control } = setup();
    startDevice(control);
    await act(async () => {
      await Promise.resolve();
    });
    expect(control.current.snapshot?.wakeLock).toBe("unavailable");
    expect(control.current.snapshot?.phase).toBe("waiting");
    vi.advanceTimersByTime(60_000);
    expect(wake.request).toHaveBeenCalledTimes(1);
  });

  it("OSがWake Lockを解除したら画面消灯の案内に切り替え、再取得は明示再開まで行わない", async () => {
    installGeolocation();
    const wake = wakeLockApi();
    const { result: control } = setup();
    startDevice(control);
    await act(async () => {
      await Promise.resolve();
    });
    act(() => wake.osRelease());
    expect(control.current.snapshot?.wakeLock).toBe("unavailable");
    vi.advanceTimersByTime(30_000);
    expect(wake.request).toHaveBeenCalledTimes(1);
  });

  it("地図が使えなくなったらナビを一時停止し、回復しても自動再開せず、使えない間は再開できない", () => {
    const gps = installGeolocation();
    const { result: control, rerender } = setup();
    startDevice(control);
    rerender({ mapAvailable: false });
    expect(control.current.snapshot).toMatchObject({ phase: "paused", pauseReason: "map-unavailable" });
    act(() => control.current.resume());
    expect(control.current.snapshot?.phase).toBe("paused");
    rerender({ mapAvailable: true });
    expect(control.current.snapshot?.phase).toBe("paused");
    act(() => control.current.resume());
    expect(control.current.snapshot?.phase).toBe("waiting");
    expect(gps.watchPosition).toHaveBeenCalledTimes(2);
  });

  it("地図が使えない状態では新規に開始できない", () => {
    const gps = installGeolocation();
    const { result: control } = setup(false);
    startDevice(control);
    expect(control.current.snapshot).toBeNull();
    expect(control.current.startError).toBe("地図を利用できません");
    expect(gps.watchPosition).not.toHaveBeenCalled();
  });

  it("unmountで位置源を止め、旧結果へ差し替わったら終了する", () => {
    const gps = installGeolocation();
    const { result: control, unmount } = setup();
    startDevice(control);
    unmount();
    expect(gps.clearWatch).toHaveBeenCalledTimes(1);
    expect(gps.active()).toBe(0);
  });
});
