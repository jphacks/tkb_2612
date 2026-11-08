import { act, fireEvent, screen, within } from "@testing-library/react";
import type { ComponentProps } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RouteCandidate } from "./lib/types";
import type { GuidanceStep } from "./navigation/guidance";
import { api, resetApi } from "./test/apiMock";
import { advance, beginNavigation, installGeolocation, lastRoute, openNavigationSidebar, openResults, positionAt, startGroup } from "./test/navigationApp";

const harness = vi.hoisted(() => ({
  /** 指定すると、デモの案内地点をこの関数の結果へ置き換える。 */
  guidance: null as null | ((route: RouteCandidate) => GuidanceStep[]),
  failMap: null as null | (() => void),
}));

vi.mock("./api/client", async () => (await import("./test/apiMock")).clientMock());
vi.mock("./components/LogoIntro", () => ({
  LogoIntro: ({ onComplete }: { onComplete: () => void }) => (
    <button onClick={onComplete}>動画をスキップして開始</button>
  ),
}));
vi.mock("./navigation/guidance", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./navigation/guidance")>();
  return {
    ...actual,
    guidanceStepsFromRoute: (route: RouteCandidate) => harness.guidance?.(route) ?? actual.guidanceStepsFromRoute(route),
  };
});
vi.mock("./components/RouteMap", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./components/RouteMap")>();
  const { useState } = await import("react");
  return {
    ...actual,
    // 実際の地図の障害と同じく、RouteMap内部からonUnavailableChangeが呼ばれる経路を通す。
    RouteMap: (props: ComponentProps<typeof actual.RouteMap>) => {
      const [failed, setFailed] = useState(false);
      harness.failMap = () => setFailed(true);
      return <actual.RouteMap {...props} unavailable={props.unavailable || failed} />;
    },
  };
});

beforeEach(() => {
  resetApi();
  harness.guidance = null;
  harness.failMap = null;
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
const bar = () => within(screen.getByRole("region", { name: "ナビ案内" }));
const action = () => document.querySelector(".nav-guidance-action")?.textContent ?? "";

/** 上部バーの距離表示をmへ戻す。距離が出ていなければnull。 */
function displayedDistance(): number | null {
  const text = document.querySelector(".nav-guidance-distance")?.textContent;
  if (!text) return null;
  if (text === "まもなく") return 0;
  return text.endsWith("km先") ? Number.parseFloat(text) * 1000 : Number.parseFloat(text);
}

function simulationControls() {
  openNavigationSidebar();
  return within(screen.getByLabelText("模擬走行の操作"));
}

function beginWithoutJitter(route: string) {
  startGroup(route);
  fireEvent.click(screen.getByText("模擬走行の設定"));
  fireEvent.click(screen.getByRole("checkbox", { name: "位置の揺らぎ（半径8m）" }));
  beginNavigation(route, "模擬走行で開始");
}

describe("上部バーの案内と通過状態", () => {
  it("揺らぎのある模擬走行で曲がり角を通過した後、通過した案内へ戻らない", async () => {
    await openResults();
    vi.useFakeTimers();
    beginNavigation("最短ルート", "模擬走行で開始");
    expect(screen.getByRole("region", { name: "ナビ案内" })).toBeVisible();
    fireEvent.click(simulationControls().getByRole("radio", { name: "8倍" }));

    const approach = 100;
    for (let i = 0; i < 400 && !(displayedDistance() !== null && displayedDistance()! <= approach); i++) advance(1000);
    expect(displayedDistance()).toBeLessThanOrEqual(approach);
    expect(action()).toBe("右折");

    // 揺らぎによる後退を見逃さないよう、等倍にして1測位ずつ表示を確認する。
    fireEvent.click(simulationControls().getByRole("radio", { name: "1倍" }));
    const history: Array<number | null> = [];
    for (let i = 0; i < 180; i++) {
      advance(1000);
      history.push(displayedDistance());
    }
    const passedAt = history.findIndex((distance) => distance !== null && distance > approach);
    expect(passedAt).toBeGreaterThan(0);
    const afterPassing = history.slice(passedAt);
    expect(afterPassing.filter((distance) => distance === null || distance <= approach)).toEqual([]);
  });

  it.each(["最短ルート", "安全ルート"])("停止後に%sで開始し直すと、前のセッションの通過状態を持ち越さない", async (nextRoute) => {
    // 模擬走行の最初の進捗が後退許容幅に入る位置へ案内地点を置き、持ち越しを検出できるようにする。
    harness.guidance = (route) => [
      { id: "turn", maneuver: "right", atM: 15 },
      { id: "arrival", maneuver: "arrive", atM: route.navigation!.length_m },
    ];
    await openResults();
    vi.useFakeTimers();
    beginWithoutJitter("最短ルート");
    advance(30_000);
    expect(action()).toBe("目的地");

    openNavigationSidebar();
    fireEvent.click(screen.getByRole("button", { name: "ナビを停止" }));
    expect(screen.queryByRole("region", { name: "ナビ案内" })).not.toBeInTheDocument();

    beginWithoutJitter(nextRoute);
    advance(6000);
    expect(bar().getByText("右折")).toBeVisible();
  });
});

describe("APIの案内地点の表示", () => {
  it("実機GPSのナビでもAPIのguidanceから距離と方向を表示する", async () => {
    const gps = installGeolocation();
    await openResults();
    const route = await lastRoute("fastest");
    const turn = route.navigation!.guidance.find((step) => step.maneuver !== "arrive")!;
    const start = Math.max(0, turn.at_m - 40);
    vi.useFakeTimers();
    beginNavigation("最短ルート", "実機GPSで開始");
    openNavigationSidebar();
    for (let i = 0; i < 7; i++) {
      advance(1000);
      gps.emit(positionAt(route, start + i * 1.4));
    }
    expect(bar().getByText("右折")).toBeVisible();
    expect(displayedDistance()).toBeGreaterThan(0);
  });

  it("同じroute-1で再検索した後、前の経路で通過した案内を新しい経路で表示する", async () => {
    harness.guidance = (route) => [
      { id: "turn", maneuver: "right", atM: 15 },
      { id: "arrival", maneuver: "arrive", atM: route.navigation!.length_m },
    ];
    await openResults();
    vi.useFakeTimers();
    startGroup("最短ルート");
    fireEvent.click(screen.getByText("模擬走行の設定"));
    fireEvent.click(screen.getByRole("radio", { name: "4倍" }));
    beginNavigation("最短ルート", "模擬走行で開始");
    openNavigationSidebar();
    advance(10_000);
    expect(action()).toBe("目的地");
    fireEvent.click(screen.getByRole("button", { name: "逸脱を試す" }));
    advance(3_000);
    expect(within(screen.getByLabelText("ナビの状態")).getByRole("status")).toHaveTextContent("経路から外れています");
    fireEvent.click(screen.getByRole("button", { name: "安全な場所で停止してから現在地から再検索" }));
    await act(async () => { await vi.advanceTimersByTimeAsync(10); });
    const response = await api.searchRoutes.mock.results.at(-1)!.value;
    expect(response.fastest_id).toBe("route-1");
    fireEvent.click(simulationControls().getByRole("radio", { name: "1倍" }));
    advance(6000);
    expect(bar().getByText("右折")).toBeVisible();
  });

  it.each([
    ["slight_left", "斜め左"],
    ["slight_right", "斜め右"],
    ["uturn", "Uターン"],
  ] as const)("%sの案内を表示する", async (maneuver, label) => {
    harness.guidance = (route) => [
      { id: "turn", maneuver, atM: route.navigation!.length_m / 2 },
      { id: "arrival", maneuver: "arrive", atM: route.navigation!.length_m },
    ];
    await openResults();
    vi.useFakeTimers();
    beginNavigation("最短ルート", "模擬走行で開始");
    advance(6000);
    expect(action()).toBe(label);
  });

  it("案内がarriveだけの経路では経路に沿って進むと表示し、右左折を出さない", async () => {
    harness.guidance = (route) => [
      { id: "arrival", maneuver: "arrive", atM: route.navigation!.length_m },
    ];
    await openResults();
    vi.useFakeTimers();
    beginNavigation("最短ルート", "模擬走行で開始");
    advance(6000);
    expect(action()).toBe("経路に沿って進む");
    expect(displayedDistance()).toBeNull();
    expect(bar().queryByText("右折")).not.toBeInTheDocument();
  });
});

describe("地図障害時の模擬走行バナー", () => {
  it("地図が使えなくなっても、一時停止した模擬走行のバナーを残し、案内バーは出さない", async () => {
    await openResults();
    vi.useFakeTimers();
    beginNavigation("最短ルート", "模擬走行で開始");
    advance(10_000);
    const overlay = () => within(document.querySelector(".nav-guidance-overlay") as HTMLElement);
    expect(overlay().getByText(NOTICE)).toBeVisible();
    expect(screen.getByRole("region", { name: "ナビ案内" })).toBeVisible();

    act(() => harness.failMap!());
    expect(screen.getByText("地図を利用できません")).toBeVisible();
    expect(overlay().getByText(NOTICE)).toBeVisible();
    expect(overlay().getByText("倍率 1倍・一時停止")).toBeVisible();
    expect(screen.queryByRole("region", { name: "ナビ案内" })).not.toBeInTheDocument();

    openNavigationSidebar();
    fireEvent.click(screen.getByRole("button", { name: "ナビを終了" }));
    expect(overlay().queryByText(NOTICE)).not.toBeInTheDocument();
  });

  it("実機GPSのナビでは、地図が使えなくなっても模擬走行のバナーを出さない", async () => {
    installGeolocation();
    await openResults();
    vi.useFakeTimers();
    beginNavigation("最短ルート", "実機GPSで開始");
    act(() => harness.failMap!());
    expect(screen.getByText("地図を利用できません")).toBeVisible();
    expect(screen.queryByText(NOTICE)).not.toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "ナビ案内" })).not.toBeInTheDocument();
  });
});
