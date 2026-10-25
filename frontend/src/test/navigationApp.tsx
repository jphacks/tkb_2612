import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, vi } from "vitest";
import App from "../App";
import { DEMO_PLACES } from "../api/demo";
import type { RouteCandidate } from "../lib/types";
import {
  createRouteGeometry,
  pointAtProgress,
  unprojectPoint,
} from "../navigation/geometry";
import { api } from "./apiMock";

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

export function startGroup(name: string) {
  if (!screen.queryByRole("group", { name: name })) {
    const close = screen.queryByRole("button", { name: "閉じる" });
    if (close) fireEvent.click(close);
    fireEvent.click(screen.getByRole("button", { name: new RegExp(`${name}.*でナビを開始`) }));
  }
  return within(screen.getByRole("group", { name: name }));
}

/** 開始操作と、注意ダイアログでの確認までを行う。 */
export function beginNavigation(group: string, button: "実機GPSで開始" | "模擬走行で開始") {
  fireEvent.click(startGroup(group).getByRole("button", { name: button }));
  fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "確認して開始" }));
}

/** 仮想時刻を進める。 */
export function advance(ms: number) {
  act(() => {
    vi.advanceTimersByTime(ms);
  });
}

/** 最後のroute応答から、選定経路を取り出す。 */
export async function lastRoute(which: "fastest" | "safest"): Promise<RouteCandidate> {
  const response = await api.searchRoutes.mock.results.at(-1)!.value;
  return response.candidates.find(
    (route: RouteCandidate) => route.id === (which === "fastest" ? response.fastest_id : response.safest_id),
  );
}

/** 経路上のp m地点から横に lateral m ずれた位置。 */
export function positionAt(route: RouteCandidate, progress: number, lateral = 0) {
  const geometry = createRouteGeometry(route.geometry.coordinates, route.navigation!);
  const at = pointAtProgress(geometry, progress);
  const a = geometry.points[at.segmentIndex];
  const b = geometry.points[at.segmentIndex + 1];
  const length = Math.hypot(b.x - a.x, b.y - a.y);
  const normal = { x: -(b.y - a.y) / length, y: (b.x - a.x) / length };
  return unprojectPoint(geometry, {
    x: at.point.x + normal.x * lateral,
    y: at.point.y + normal.y * lateral,
  });
}

type Success = (position: GeolocationPosition) => void;
type Failure = (error: GeolocationPositionError) => void;

/** navigator.geolocationのフェイク。watchの成功/失敗コールバックを外から呼ぶ。 */
export function installGeolocation() {
  let id = 0;
  const watches = new Map<number, { success: Success; failure: Failure }>();
  const watchPosition = vi.fn((success: Success, failure: Failure, ..._rest: unknown[]) => {
    watches.set(++id, { success, failure });
    return id;
  });
  const clearWatch = vi.fn((watch: number) => {
    watches.delete(watch);
  });
  Object.defineProperty(navigator, "geolocation", {
    configurable: true,
    value: { watchPosition, clearWatch, getCurrentPosition: vi.fn() },
  });
  return {
    watchPosition,
    clearWatch,
    active: () => watches.size,
    /** 現在のwatchすべてへ測位を渡す。timestampは仮想の現在時刻。 */
    emit(position: { lon: number; lat: number }, accuracy = 10, timestamp = Date.now()) {
      act(() => {
        for (const watch of watches.values())
          watch.success({
            coords: { longitude: position.lon, latitude: position.lat, accuracy },
            timestamp,
          } as GeolocationPosition);
      });
    },
    fail(code: 1 | 2 | 3) {
      act(() => {
        for (const watch of watches.values())
          watch.failure({ code, PERMISSION_DENIED: 1, POSITION_UNAVAILABLE: 2, TIMEOUT: 3, message: "" });
      });
    },
  };
}
