import { act, fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { registerServiceWorker } from "./pwa/register";
import { resetApi } from "./test/apiMock";
import {
  advance,
  beginNavigation,
  installGeolocation,
  lastRoute,
  openResults,
  positionAt,
} from "./test/navigationApp";

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
  Reflect.deleteProperty(document, "visibilityState");
});

function setVisibility(state: "hidden" | "visible") {
  Object.defineProperty(document, "visibilityState", { configurable: true, value: state });
  document.dispatchEvent(new Event("visibilitychange"));
}

async function startDevice() {
  const gps = installGeolocation();
  await openResults();
  const route = await lastRoute("fastest");
  vi.useFakeTimers();
  beginNavigation("最短ルート", "実機GPSで開始");
  return { gps, route };
}

describe("実機ナビのボタン・案内（手順11・12）", () => {
  it("Wake Lockが使えない環境では、画面が消えると一時停止する旨を案内する", async () => {
    await startDevice();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10);
    });
    expect(screen.getByText("画面が消えるとナビは一時停止します")).toBeVisible();
  });

  it("一時停止と明示再開。再開すると全経路で確認し直す", async () => {
    const { gps, route } = await startDevice();
    for (let i = 0; i < 7; i++) {
      advance(1000);
      gps.emit(positionAt(route, 100 + i * 1.4));
    }
    fireEvent.click(screen.getByRole("button", { name: "一時停止" }));
    expect(screen.getByText("一時停止時の位置")).toBeVisible();
    expect(gps.active()).toBe(0);
    fireEvent.click(screen.getByRole("button", { name: "再開" }));
    expect(gps.active()).toBe(1);
    expect(screen.getByText("測位待ち")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "ナビを停止" }));
    expect(gps.active()).toBe(0);
  });

  it("権限拒否では理由と、設定変更後の再開または模擬走行を案内し、許可要求を繰り返さない", async () => {
    const { gps } = await startDevice();
    gps.fail(1);
    expect(screen.getByText(/位置情報の利用が許可されていません。設定を変更して再開するか、模擬走行を使ってください/)).toBeVisible();
    advance(60_000);
    expect(gps.watchPosition).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "再開" })).toBeVisible();
  });

  it("非表示で一時停止し、表示に戻っても自動再開せず、明示再開を案内する", async () => {
    const { gps } = await startDevice();
    act(() => setVisibility("hidden"));
    expect(screen.getByText(/画面が非表示になったため一時停止しました/)).toBeVisible();
    act(() => setVisibility("visible"));
    advance(30_000);
    expect(gps.watchPosition).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "再開" })).toBeVisible();
  });
});

describe("PWAの更新待機の通知（8章）", () => {
  it("検索・ナビ中は再読込を提示せず待機を伝え、終了後に再読込を案内する。強制切替はしない", async () => {
    const registration = Object.assign(new EventTarget(), {
      waiting: new EventTarget() as unknown as ServiceWorker,
      installing: null,
    });
    const register = vi.fn().mockResolvedValue(registration);
    await registerServiceWorker(true, { register } as unknown as ServiceWorkerContainer);
    await openResults();
    expect(screen.getByText("新しいバージョンがあります。再読込できます。")).toBeVisible();
    expect(screen.getByText("再読込すると、入力した地点・検索結果・経路が消えます。")).toBeVisible();
    expect(screen.getByRole("button", { name: "再読込" })).toBeVisible();
    vi.useFakeTimers();
    beginNavigation("最短ルート", "模擬走行で開始");
    expect(screen.getByText("新しいバージョンがあります。検索またはナビの終了後に再読込できます。")).toBeVisible();
    expect(screen.queryByRole("button", { name: "再読込" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "一時停止" }));
    expect(screen.queryByRole("button", { name: "再読込" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "ナビを終了" }));
    expect(screen.getByRole("button", { name: "再読込" })).toBeVisible();
  });
});
