import { fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resetApi } from "./test/apiMock";
import {
  advance,
  startGroup,
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

const phase = () => within(screen.getByLabelText("ナビの状態")).getByRole("status");
const metrics = () => screen.getByText("残り距離（概算）").closest("div")!;

/** 実機GPSで開始し、1秒ごとに進捗pの位置を渡す。 */
async function startDevice() {
  const gps = installGeolocation();
  await openResults();
  const route = await lastRoute("fastest");
  vi.useFakeTimers();
  beginNavigation("最短ルート", "実機GPSで開始");
  const feed = (progress: number, lateral = 0, accuracy = 10) => {
    advance(1000);
    gps.emit(positionAt(route, progress, lateral), accuracy);
  };
  return { gps, route, feed };
}

describe("ナビの状態表示（手順10・5章）", () => {
  it("測位待ちから進行中になり、残り距離・残り時間（概算）と選んだ種別、検索時点の旨を表示する", async () => {
    const { feed } = await startDevice();
    expect(phase()).toHaveTextContent("測位待ち");
    expect(metrics()).toHaveTextContent("—");
    for (let i = 0; i < 7; i++) feed(100 + i * 1.4);
    expect(phase()).toHaveTextContent("進行中");
    expect(screen.getByText("残り距離（概算）")).toBeVisible();
    expect(screen.getByText("残り時間（概算）")).toBeVisible();
    expect(metrics()).toHaveTextContent(/km|m/);
    expect(screen.getByText("最短ルート", { selector: "dd" })).toBeVisible();
    expect(screen.getByText("条件と事故データは検索時点のものです")).toBeVisible();
  });

  it("精度不足では原因・精度・最後の位置時点を示し、進捗の更新を止める", async () => {
    const { feed } = await startDevice();
    for (let i = 0; i < 7; i++) feed(100 + i * 1.4);
    const before = metrics().textContent;
    feed(120, 0, 50);
    expect(phase()).toHaveTextContent("精度不足");
    expect(screen.getByText("位置精度が低いため判定を停止中")).toBeVisible();
    expect(screen.getByText("約50m")).toBeVisible();
    expect(screen.getByText("最後の位置")).toBeVisible();
    expect(screen.getByText("更新停止")).toBeVisible();
    feed(400, 0, 50);
    expect(metrics().textContent).toBe(before);
  });

  it("5秒を超えて測位がなければ精度不足の原因を受信なしとして表示する", async () => {
    const { feed } = await startDevice();
    for (let i = 0; i < 7; i++) feed(100 + i * 1.4);
    advance(6000);
    expect(phase()).toHaveTextContent("精度不足");
    expect(screen.getByText("位置を受信できないため判定を停止中")).toBeVisible();
  });

  it("経路から離れた位置は照合保留と表示し、進捗を更新しない。逸脱は10秒・3測位で確定し、復帰まで保持する", async () => {
    const { feed } = await startDevice();
    for (let i = 0; i < 7; i++) feed(100 + i * 1.4);
    const before = metrics().textContent;
    feed(110, 25);
    expect(phase()).toHaveTextContent("照合保留");
    expect(screen.getByText("経路上の位置を確認中")).toBeVisible();
    expect(metrics().textContent).toBe(before);
    for (let i = 0; i < 12; i++) feed(110, 120);
    expect(phase()).toHaveTextContent("経路から外れています");
    expect(screen.getByText("安全な場所で停止してから、現在地から再検索できます")).toBeVisible();
    for (let i = 0; i < 4; i++) feed(110, 0);
    expect(phase()).toHaveTextContent("経路から外れています");
    for (let i = 0; i < 8; i++) feed(112 + i, 0);
    expect(phase()).not.toHaveTextContent("経路から外れています");
  });

  it("模擬走行は到着まで進み、到着では残りを0にして位置源を止める", async () => {
    await openResults();
    startGroup("最短ルート");
    fireEvent.click(screen.getByText("模擬走行の設定"));
    fireEvent.click(screen.getByRole("radio", { name: "8倍" }));
    vi.useFakeTimers();
    beginNavigation("最短ルート", "模擬走行で開始");
    advance(1000 * 20);
    expect(phase()).toHaveTextContent("進行中");
    advance(1000 * 600);
    expect(phase()).toHaveTextContent("到着");
    expect(metrics()).toHaveTextContent("0 m");
    expect(screen.getByText("0分")).toBeVisible();
    expect(screen.getByText(/経路の終点に到着しました/)).toBeVisible();
    const text = document.body.textContent;
    advance(10_000);
    expect(document.body.textContent).toBe(text);
  }, 30000);
});
