import { act, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CommunityMap } from "./CommunityMap";

vi.hoisted(() => {
  // isDemoはimport時に確定するので、実地図の判定より先にliveへ切り替える。
  vi.stubEnv("VITE_API_MODE", "live");
});

type MockMap = {
  styleAvailable: boolean;
  listeners: Map<string, Set<(event: unknown) => void>>;
  emit: (name: string, event?: unknown) => void;
  on: ReturnType<typeof vi.fn>;
  getStyle: () => { version: 8; sources: object; layers: unknown[] } | undefined;
  getCenter: () => { lat: number; lng: number };
  getZoom: () => number;
  remove: ReturnType<typeof vi.fn>;
};

const live = vi.hoisted(() => ({ maps: [] as MockMap[] }));

vi.mock("maplibre-gl", () => ({
  setWorkerUrl: vi.fn(),
  Map: class {
    styleAvailable = true;
    listeners = new Map<string, Set<(event: unknown) => void>>();
    constructor() {
      live.maps.push(this);
    }
    emit(name: string, event: unknown = {}) {
      this.listeners.get(name)?.forEach((listener) => listener(event));
    }
    on = vi.fn((name: string, listener: (event: unknown) => void) => {
      if (!this.listeners.has(name)) this.listeners.set(name, new Set());
      this.listeners.get(name)!.add(listener);
    });
    getStyle = () =>
      this.styleAvailable
        ? { version: 8 as const, sources: {}, layers: [] }
        : undefined;
    getCenter = () => ({ lat: 35.6812, lng: 139.7671 });
    getZoom = () => 14;
    jumpTo = vi.fn();
    resize = vi.fn();
    project = () => ({ x: 100, y: 100 });
    remove = vi.fn();
  },
}));

const props = {
  center: { lat: 35.6812, lon: 139.7671 },
  currentPosition: null,
  reports: [],
  selected: null,
  onPick: vi.fn(),
  onReport: vi.fn(),
  onLocate: vi.fn(),
  popupWidth: 146,
  popupHeight: 82,
};

async function renderLive() {
  const view = render(<CommunityMap {...props} />);
  await waitFor(() => expect(live.maps).toHaveLength(1));
  return { ...view, map: live.maps[0] };
}

describe("CommunityMapの実地図エラー判定", () => {
  beforeEach(() => {
    live.maps.length = 0;
    vi.stubEnv("VITE_MAP_STYLE_URL", "https://example.invalid/style.json");
  });
  afterEach(() => {
    vi.unstubAllEnvs();
  });

  it("keeps the live map when a tile fails to load", async () => {
    const { map } = await renderLive();
    act(() =>
      map.emit("error", {
        error: new Error("tile failed"),
        tile: {},
        style: {},
      }),
    );
    expect(
      screen.queryByRole("application", {
        name: "安全危険情報共有モードの地図",
      }),
    ).not.toBeInTheDocument();
    expect(document.querySelector(".route-live-map")).toBeInTheDocument();
    expect(map.remove).not.toHaveBeenCalled();
  });

  it("falls back to the schematic map for a fatal style error", async () => {
    const { map } = await renderLive();
    map.styleAvailable = false;
    act(() =>
      map.emit("error", {
        error: new Error("Failed to fetch"),
        style: {},
      }),
    );
    expect(
      screen.getByRole("application", {
        name: "安全危険情報共有モードの地図",
      }),
    ).toBeVisible();
    expect(map.remove).toHaveBeenCalledTimes(1);
  });
});
