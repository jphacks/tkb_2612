import { fireEvent, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resetApi } from "./test/apiMock";
import { advance, beginNavigation, installGeolocation, openResults, startGroup } from "./test/navigationApp";

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

const NOTICE = "模擬走行中（実際の現在地ではありません）";
const phase = () => within(screen.getByLabelText("ナビの状態")).getByRole("status");
const marker = () => document.querySelector(".map-nav-location")!;
const position = () => `${marker().getAttribute("cx")},${marker().getAttribute("cy")}`;

async function startSimulation() {
  await openResults();
  vi.useFakeTimers();
  beginNavigation("最短ルート", "模擬走行で開始");
}

describe("模擬走行の区別とSimulationControls（手順10・7.7）", () => {
  it("模擬走行中の旨・倍率・状態を常設し、現在地マーカーも実機と区別する。サイドバーを閉じても地図の上に残る", async () => {
    await startSimulation();
    expect(screen.getByText(NOTICE)).toBeVisible();
    expect(screen.getByText("倍率 1倍・進行中")).toBeVisible();
    advance(2000);
    expect(marker()).toHaveClass("is-simulation");
    fireEvent.click(screen.getByRole("button", { name: "条件サイドバーを閉じる" }));
    expect(document.querySelector(".map-stage .nav-sim-banner")).not.toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "条件サイドバーを開く" }));
    expect(document.querySelector(".map-stage .nav-sim-banner")).toBeNull();
  });

  it("倍率1/2/4/8を選べ、seedと揺らぎを開始前に設定でき、走行中のseedは表示のみ", async () => {
    await openResults();
    const settings = startGroup("最短ルート");
    fireEvent.click(screen.getByText("模擬走行の設定"));
    expect(settings.getByRole("button", { name: "模擬走行で開始" })).toBeVisible();
    const seed = screen.getByLabelText(/seed/);
    fireEvent.change(seed, { target: { value: "-1" } });
    expect(seed).toHaveAttribute("aria-invalid", "true");
    expect(startGroup("最短ルート").getByRole("button", { name: "模擬走行で開始" })).toBeDisabled();
    fireEvent.change(seed, { target: { value: "42" } });
    fireEvent.click(screen.getByRole("radio", { name: "4倍" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "位置の揺らぎ（半径8m）" }));
    vi.useFakeTimers();
    beginNavigation("最短ルート", "模擬走行で開始");
    expect(screen.getByText("倍率 4倍・進行中")).toBeVisible();
    const controls = within(screen.getByLabelText("模擬走行の操作"));
    expect(controls.getByText("42")).toBeVisible();
    expect(controls.getByRole("checkbox", { name: "位置の揺らぎ（半径8m）" })).not.toBeChecked();
    expect(controls.queryByLabelText(/seed/)).not.toBeInTheDocument();
    fireEvent.click(controls.getByRole("radio", { name: "8倍" }));
    expect(screen.getByText("倍率 8倍・進行中")).toBeVisible();
  });

  it("一時停止中は位置も残りも進まず、明示再開で全経路を確認し直して進む", async () => {
    await startSimulation();
    advance(15_000);
    const controls = within(screen.getByLabelText("模擬走行の操作"));
    fireEvent.click(controls.getByRole("button", { name: "一時停止" }));
    expect(screen.getByText("倍率 1倍・一時停止")).toBeVisible();
    expect(phase()).toHaveTextContent("一時停止");
    expect(screen.getByText("一時停止時の位置")).toBeVisible();
    const frozen = position();
    const remaining = screen.getByText("残り距離（概算）").closest("div")!.textContent;
    advance(60_000);
    expect(position()).toBe(frozen);
    expect(screen.getByText("残り距離（概算）").closest("div")!.textContent).toBe(remaining);
    fireEvent.click(controls.getByRole("button", { name: "再開" }));
    expect(phase()).toHaveTextContent("測位待ち");
    advance(30_000);
    expect(phase()).toHaveTextContent("進行中");
    expect(position()).not.toBe(frozen);
  });

  it("逸脱を試すと逸脱表示になり、自動の再検索はせず、基準の進行が戻ると復帰する", async () => {
    await startSimulation();
    advance(10_000);
    expect(phase()).toHaveTextContent("進行中");
    const searches = (await import("./test/apiMock")).api.searchRoutes.mock.calls.length;
    fireEvent.click(screen.getByRole("button", { name: "逸脱を試す" }));
    advance(16_000);
    expect(phase()).toHaveTextContent("経路から外れています");
    expect(screen.getByText("安全な場所で停止してから、現在地から再検索できます")).toBeVisible();
    advance(40_000);
    expect(phase()).toHaveTextContent("進行中");
    expect((await import("./test/apiMock")).api.searchRoutes.mock.calls.length).toBe(searches);
  });

  it("模擬走行では実機の位置情報を使わない", async () => {
    const gps = installGeolocation();
    await startSimulation();
    advance(20_000);
    expect(gps.watchPosition).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "実機GPSで開始" })).not.toBeInTheDocument();
  });
});
