import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createDemoRouteResponse, DEMO_PLACES } from "../api/demo";
import { RouteMap, type RouteMapProps } from "./RouteMap";
import type { RouteResponse } from "../lib/types";
import { emptyNavigationLayers, setNavigationLayers } from "../map/layers";

const request = {
  origin: DEMO_PLACES[0],
  dest: DEMO_PLACES[1],
  mode: "walk" as const,
  period: "昼" as const,
  weather: "晴" as const,
  day_type: "平日" as const,
  max_detour_min: 10,
};
function mapProps(result: RouteResponse): RouteMapProps {
  return {
    result,
    selectedRouteId: result.safest_id ?? result.fastest_id,
    selectedSectionId: null,
    onSelectRoute: vi.fn(),
    onSelectSection: vi.fn(),
    showAccidents: true,
    unavailable: false,
  };
}

describe("RouteMap", () => {
  it.each([
    [390, 720],
    [390, 400],
    [1048, 720],
  ])(
    "pairs callouts with paths and keeps cards within a %i by %i map",
    (width, height) => {
      const bounds = vi
        .spyOn(HTMLElement.prototype, "getBoundingClientRect")
        .mockReturnValue({
          width,
          height,
          top: 0,
          left: 0,
          bottom: height,
          right: width,
          x: 0,
          y: 0,
          toJSON: () => ({}),
        });
      const props = mapProps(createDemoRouteResponse(request));
      render(<RouteMap {...props} />);
      const group = screen.getByRole("group", { name: "表示する経路" });
      const cards = [...group.querySelectorAll<HTMLElement>(".map-route-card")];
      expect(cards).toHaveLength(2);
      const considered = cards.find((card) =>
        card.textContent?.includes("安全ルート"),
      )!;
      const fastest = cards.find((card) =>
        card.textContent?.includes("最短ルート"),
      )!;
      expect(considered).toHaveClass("route-considered");
      expect(fastest).toHaveClass("route-fastest");
      // Each card offers selection and a navigation start button.
      expect(
        within(fastest).getByRole("button", { name: "最短ルートでナビを開始" }),
      ).toBeInTheDocument();
      const line = screen.getByRole("button", { name: "最短ルートを選択" });
      expect(line).toHaveClass("route-fastest");
      fireEvent.pointerEnter(fastest);
      expect(line).toHaveClass("is-hovered");
      fireEvent.click(
        within(fastest).getByRole("button", {
          name: /^最短ルート/,
          pressed: false,
        }),
      );
      expect(props.onSelectRoute).toHaveBeenCalledWith(
        props.result!.fastest_id,
      );
      for (const card of cards) {
        expect(parseFloat(card.style.left)).toBeGreaterThanOrEqual(0);
        expect(
          parseFloat(card.style.left) + parseFloat(card.style.width),
        ).toBeLessThanOrEqual(width);
        expect(
          parseFloat(card.style.top) + parseFloat(card.style.height),
        ).toBeLessThanOrEqual(height);
      }
      bounds.mockRestore();
    },
  );
  beforeEach(() => {
    vi.stubEnv("VITE_API_MODE", "demo");
  });
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.restoreAllMocks();
  });

  it("connects route and section selection, and only shows factual accident fields", () => {
    const props = mapProps(createDemoRouteResponse(request));
    render(<RouteMap {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "最短ルートを選択" }));
    expect(props.onSelectRoute).toHaveBeenCalledWith(props.result?.fastest_id);
    const selected = props.result!.candidates.find(
      (route) => route.id === props.selectedRouteId,
    )!;
    fireEvent.click(screen.getByRole("button", { name: "注意箇所 1 を選択" }));
    expect(props.onSelectSection).toHaveBeenCalledWith(
      selected.high_risk_sections.features[0].id,
    );
    const accident = selected.accident_points.features[0];
    fireEvent.click(
      screen.getByRole("button", {
        name: `事故記録 ${accident.id + 1} を表示`,
      }),
    );
    expect(
      screen.getByRole("region", { name: "過去の事故記録" }),
    ).toBeVisible();
    expect(screen.getByText("死傷区分")).toBeVisible();
    expect(screen.queryByText(/原因|見通し|交差点/)).not.toBeInTheDocument();
  });

  it("removes accident details when points are hidden and when a new result has no accidents", () => {
    const props = mapProps(createDemoRouteResponse(request));
    const { rerender } = render(<RouteMap {...props} />);
    fireEvent.click(
      screen.getAllByRole("button", { name: /事故記録 .* を表示/ })[0],
    );
    rerender(<RouteMap {...props} showAccidents={false} />);
    expect(
      screen.queryByRole("region", { name: "過去の事故記録" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /事故記録 .* を表示/ }),
    ).not.toBeInTheDocument();
    rerender(
      <RouteMap {...mapProps(createDemoRouteResponse(request, "zero"))} />,
    );
    expect(
      screen.queryByRole("button", { name: /事故記録 .* を表示/ }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /注意箇所/ }),
    ).not.toBeInTheDocument();
  });

  it("模式図でもナビ中は、ナビ経路・現在地（模擬は輪）・通知中区間を重ね、比較カードを隠す", () => {
    const props = mapProps(createDemoRouteResponse(request));
    const route = props.result!.candidates[0];
    const layers = emptyNavigationLayers();
    layers.route.features.push({ type: "Feature", geometry: route.geometry, properties: {} });
    layers.location.features.push({
      type: "Feature",
      geometry: { type: "Point", coordinates: route.geometry.coordinates[1] },
      properties: { kind: "simulation", onRoute: true },
    });
    layers.alert.features.push({
      type: "Feature",
      id: "a",
      geometry: { type: "LineString", coordinates: route.geometry.coordinates.slice(1, 3) },
      properties: {},
    });
    const { container } = render(
      <RouteMap {...props} navigation={{ layers, follow: true, onFollowChange: vi.fn() }} />,
    );
    expect(container.querySelectorAll(".map-nav-route")).toHaveLength(1);
    expect(container.querySelector(".map-nav-location")).toHaveClass("is-simulation");
    expect(container.querySelectorAll(".map-nav-alert")).toHaveLength(1);
    expect(screen.queryByRole("group", { name: "表示する経路" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "現在地に追従" })).not.toBeInTheDocument();
  });

  it("labels coincident recommendations as one route and preserves comparison when the map is unavailable", () => {
    const props = mapProps(createDemoRouteResponse(request, "same"));
    const { rerender } = render(<RouteMap {...props} />);
    expect(
      screen.getAllByRole("button", {
        name: "最短ルート・安全ルート（同一経路）を選択",
      }),
    ).toHaveLength(1);
    rerender(<RouteMap {...props} unavailable />);
    expect(screen.getByText("地図を利用できません")).toBeVisible();
    expect(screen.getByRole("group", { name: "表示する経路" })).toBeVisible();
  });

  it("tells that navigation is not available yet instead of doing nothing", () => {
    const props = mapProps(createDemoRouteResponse(request));
    render(<RouteMap {...props} />);
    fireEvent.click(
      screen.getByRole("button", { name: "安全ルートでナビを開始" }),
    );
    expect(screen.getByRole("status")).toHaveTextContent(
      "ナビの開始は準備中です。",
    );
    expect(props.onSelectRoute).not.toHaveBeenCalled();
  });
});

// WebGLやネットワークを使わず、MapLibreへの入力とライフサイクルを検査する。
const live = vi.hoisted(() => ({ maps: [] as MockMap[] }));

type MockSource = { setData: ReturnType<typeof vi.fn>; data: import("geojson").FeatureCollection };
type MockLayer = import("maplibre-gl").LayerSpecification;
type MockMap = {
  sources: Map<string, MockSource>;
  layers: MockLayer[];
  listeners: Map<string, Set<(event: never) => void>>;
  loaded: boolean;
  styleAvailable: boolean;
  emit: (name: string, event?: unknown) => void;
  on: ReturnType<typeof vi.fn>;
  off: ReturnType<typeof vi.fn>;
  getSource: (id: string) => MockSource | undefined;
  getLayer: (id: string) => MockLayer | undefined;
  addSource: ReturnType<typeof vi.fn>;
  addLayer: ReturnType<typeof vi.fn>;
  remove: ReturnType<typeof vi.fn>;
  fitBounds: ReturnType<typeof vi.fn>;
  queryRenderedFeatures: ReturnType<typeof vi.fn>;
};
vi.mock("maplibre-gl", () => ({
  setWorkerUrl: vi.fn(),
  NavigationControl: class {},
  AttributionControl: class {},
  Map: class {
    sources = new Map<string, MockSource>();
    layers: MockLayer[] = [];
    listeners = new Map<string, Set<(event: never) => void>>();
    loaded = true;
    styleAvailable = true;
    getStyle = () => this.styleAvailable ? { version: 8, sources: {}, layers: [] } : undefined;
    constructor() { live.maps.push(this); }
    emit(name: string, event: unknown = {}) {
      this.listeners.get(name)?.forEach((listener) => listener(event as never));
    }
    on = vi.fn((name: string, listener: (event: never) => void) => {
      if (!this.listeners.has(name)) this.listeners.set(name, new Set());
      this.listeners.get(name)!.add(listener);
    });
    off = vi.fn((name: string, listener: (event: never) => void) => {
      this.listeners.get(name)?.delete(listener);
    });
    isStyleLoaded = () => this.loaded;
    getSource = (id: string) => this.sources.get(id);
    getLayer = (id: string) => this.layers.find((layer) => layer.id === id);
    addSource = vi.fn((id: string, spec: { data: import("geojson").FeatureCollection }) => {
      const source: MockSource = {
        data: structuredClone(spec.data),
        setData: vi.fn((data: import("geojson").FeatureCollection) => {
          source.data = structuredClone(data);
          this.emit("styledata");
        }),
      };
      this.sources.set(id, source);
      this.emit("styledata");
    });
    addLayer = vi.fn((layer: MockLayer, before?: string) => {
      const index = this.layers.findIndex((item) => item.id === before);
      this.layers.splice(index < 0 ? this.layers.length : index, 0, layer);
      this.emit("styledata");
    });
    addControl = vi.fn();
    remove = vi.fn();
    fitBounds = vi.fn();
    easeTo = vi.fn();
    resize = vi.fn();
    project = () => ({ x: 100, y: 100 });
    queryRenderedFeatures = vi.fn(() => []);
  },
}));


async function renderLive(props = mapProps(createDemoRouteResponse(request))) {
  const view = render(<RouteMap {...props} />);
  await waitFor(() => expect(live.maps).toHaveLength(1));
  const map = live.maps[0];
  act(() => map.emit("load"));
  return { ...view, map, props };
}

function mapInstance(map: MockMap) {
  return map as unknown as import("maplibre-gl").Map;
}

describe("RouteMapの実地図内部契約", () => {
  beforeEach(() => {
    live.maps.length = 0;
    vi.stubEnv("VITE_API_MODE", "live");
    vi.stubEnv("VITE_MAP_STYLE_URL", "https://example.invalid/style.json");
  });
  afterEach(() => { vi.unstubAllEnvs(); });

  it("標準IDと空のナビsourceを用意し、従来のpaintと重なり順を維持する", async () => {
    const { map } = await renderLive();
    expect([...map.sources.keys()].sort()).toEqual([
      "route-fastest", "route-safest", "route-high-risk", "route-accidents",
      "current-location", "current-accuracy", "navigation-route", "navigation-alert",
    ].sort());
    expect(map.layers.map((layer) => layer.id)).toEqual([
      "route-fastest-line", "route-safest-line", "route-hit", "navigation-route-line",
      "route-high-risk-line", "section-hit", "navigation-alert-line", "navigation-alert-point", "route-accidents-point",
      "accident-hit", "current-accuracy-fill", "current-location-point",
    ]);
    for (const id of ["route-safest", "current-location", "current-accuracy", "navigation-route", "navigation-alert"]) {
      expect(map.getSource(id)?.data).toEqual({ type: "FeatureCollection", features: [] });
    }
    expect(map.getLayer("route-fastest-line")).toMatchObject({
      paint: { "line-color": ["get", "color"], "line-width": ["case", ["get", "selected"], 7, 4], "line-opacity": 1 },
      layout: { "line-cap": "round", "line-join": "round" },
    });
    expect(map.getLayer("route-high-risk-line")).toMatchObject({
      paint: { "line-color": "#d97706", "line-width": ["case", ["get", "selected"], 12, 7] },
    });
    expect(map.getLayer("route-accidents-point")).toMatchObject({
      paint: { "circle-radius": 7, "circle-color": "#fff", "circle-stroke-color": "#d97706", "circle-stroke-width": 3 },
    });
  });

  it("全候補のgeometry・順序・色と、選択候補だけの区間・事故点・boundsを維持する", async () => {
    const { map, props, rerender } = await renderLive();
    const result = props.result!;
    const selected = result.candidates.find((route) => route.id === props.selectedRouteId)!;
    const routes = map.getSource("route-fastest")!.data.features;
    expect(routes.map((route) => route.id)).toEqual([
      ...result.candidates.filter((route) => route !== selected).map((route) => route.id), selected.id,
    ]);
    routes.forEach((feature) => {
      expect(feature.geometry).toEqual(result.candidates.find((route) => route.id === feature.id)!.geometry);
      expect(feature.properties).toEqual({
        selected: feature.id === selected.id,
        color: feature.id === result.safest_id ? "#16a34a" : feature.id === result.fastest_id ? "#2563eb" : "#94a3b8",
      });
    });
    expect(map.getSource("route-accidents")!.data.features).toEqual(selected.accident_points.features);
    expect(map.getSource("route-high-risk")!.data.features.map((feature) => feature.geometry)).toEqual(selected.high_risk_sections.features.map((feature) => feature.geometry));
    const coordinates = result.candidates.flatMap((route) => route.geometry.coordinates);
    expect(map.fitBounds).toHaveBeenLastCalledWith([
      [Math.min(...coordinates.map(([x]) => x)), Math.min(...coordinates.map(([, y]) => y))],
      [Math.max(...coordinates.map(([x]) => x)), Math.max(...coordinates.map(([, y]) => y))],
    ], { padding: 64, duration: 0, maxZoom: 16 });
    rerender(<RouteMap {...props} showAccidents={false} />);
    expect(map.getSource("route-accidents")!.data.features).toEqual([]);
    rerender(<RouteMap {...props} result={null} />);
    for (const id of ["route-fastest", "route-safest", "route-high-risk", "route-accidents"]) {
      expect(map.getSource(id)!.data.features).toEqual([]);
    }
    expect(live.maps).toHaveLength(1);
  });

  it.each(["styledata", "style.load"])("%sで最新の選択・表示状態とナビデータを復元する", async (event) => {
    const { map, props, rerender } = await renderLive();
    map.loaded = false;
    act(() => map.emit("styledataloading"));
    map.sources.clear();
    map.layers.length = 0;
    const nextProps = { ...props, selectedRouteId: props.result!.fastest_id, showAccidents: false };
    rerender(<RouteMap {...nextProps} />);
    const navigation = emptyNavigationLayers();
    navigation.location.features.push({ type: "Feature", geometry: { type: "Point", coordinates: [139, 35] }, properties: {} });
    act(() => setNavigationLayers(mapInstance(map), navigation));
    navigation.location.features[0].geometry.coordinates[0] = 141;
    act(() => setNavigationLayers(mapInstance(map), { location: navigation.location }));
    expect(map.sources.size).toBe(0);
    map.loaded = event !== "style.load";
    act(() => map.emit(event));
    expect(map.sources.size).toBe(8);
    expect(map.layers).toHaveLength(12);
    expect(map.getSource("current-location")!.data).toEqual(navigation.location);
    expect(map.getSource("current-location")!.data.features[0].geometry).toEqual({ type: "Point", coordinates: [141, 35] });
    expect(map.getSource("route-accidents")!.data.features).toEqual([]);
    expect(map.getSource("route-fastest")!.data.features.at(-1)?.id).toBe(nextProps.selectedRouteId);
    map.loaded = true;
    const calls = map.getSource("route-fastest")!.setData.mock.calls.length;
    act(() => { map.emit("styledata"); map.emit("styledata"); });
    expect(map.getSource("route-fastest")!.setData).toHaveBeenCalledTimes(calls);
    expect(map.layers).toHaveLength(12);
    expect(map.fitBounds).toHaveBeenCalledTimes(1);
  });

  it("タイル待ちでも選択・事故表示の更新を保留しない", async () => {
    const { map, props, rerender } = await renderLive();
    map.loaded = false;
    rerender(<RouteMap {...props} selectedRouteId={props.result!.fastest_id} showAccidents={false} />);
    expect(map.getSource("route-fastest")!.data.features.at(-1)?.id).toBe(props.result!.fastest_id);
    expect(map.getSource("route-accidents")!.data.features).toEqual([]);
  });

  it("ナビの差分更新・全消去と、一部レイヤー欠落の復元に対応する", async () => {
    const { map } = await renderLive();
    const navigation = emptyNavigationLayers();
    navigation.route.features.push({ type: "Feature", geometry: { type: "LineString", coordinates: [[139, 35], [140, 36]] }, properties: {} });
    act(() => setNavigationLayers(mapInstance(map), { route: navigation.route }));
    expect(map.getSource("navigation-route")!.data).toEqual(navigation.route);
    expect(map.getSource("current-location")!.data.features).toEqual([]);
    map.layers = map.layers.filter((layer) => layer.id !== "route-fastest-line");
    act(() => map.emit("styledata"));
    expect(map.layers[0].id).toBe("route-fastest-line");
    act(() => setNavigationLayers(mapInstance(map), emptyNavigationLayers()));
    for (const id of ["current-location", "current-accuracy", "navigation-route", "navigation-alert"]) {
      expect(map.getSource(id)!.data.features).toEqual([]);
    }
  });

  it("同じFCを変更して再送しても全ナビsourceを更新し、styledataだけでは再投入しない", async () => {
    const { map } = await renderLive();
    const navigation = emptyNavigationLayers();
    navigation.location.features.push({ type: "Feature", geometry: { type: "Point", coordinates: [139, 35] }, properties: {} });
    navigation.accuracy.features.push({ type: "Feature", geometry: { type: "Polygon", coordinates: [[[139, 35], [140, 35], [140, 36], [139, 35]]] }, properties: {} });
    navigation.route.features.push({ type: "Feature", geometry: { type: "LineString", coordinates: [[139, 35], [140, 36]] }, properties: {} });
    navigation.alert.features.push({ type: "Feature", geometry: { type: "Point", coordinates: [139, 35] }, properties: {} });
    act(() => setNavigationLayers(mapInstance(map), navigation));
    const ids = ["current-location", "current-accuracy", "navigation-route", "navigation-alert"];
    const snapshots = ids.map((id) => map.getSource(id)!.data);
    const calls = ids.map((id) => map.getSource(id)!.setData.mock.calls.length);
    navigation.location.features[0].geometry.coordinates[0] = 141;
    navigation.accuracy.features[0].geometry.coordinates[0][0][0] = 141;
    navigation.route.features[0].geometry.coordinates[0][0] = 141;
    navigation.alert.features[0].geometry.coordinates[0] = 141;
    ids.forEach((id, index) => expect(map.getSource(id)!.data).toEqual(snapshots[index]));
    act(() => setNavigationLayers(mapInstance(map), navigation));
    ids.forEach((id, index) => {
      expect(map.getSource(id)!.data).not.toEqual(snapshots[index]);
      expect(map.getSource(id)!.setData).toHaveBeenCalledTimes(calls[index] + 1);
    });
    act(() => { map.emit("styledata"); map.emit("styledata"); });
    ids.forEach((id, index) => expect(map.getSource(id)!.setData).toHaveBeenCalledTimes(calls[index] + 1));
    // 未変更の同一FCでも明示setterは反映する。未指定のsourceは更新しない。
    act(() => setNavigationLayers(mapInstance(map), { location: navigation.location }));
    expect(map.getSource(ids[0])!.setData).toHaveBeenCalledTimes(calls[0] + 2);
    ids.slice(1).forEach((id, index) => expect(map.getSource(id)!.setData).toHaveBeenCalledTimes(calls[index + 1] + 1));
    expect(map.getLayer("navigation-alert-point")).toMatchObject({
      type: "circle", source: "navigation-alert", filter: ["==", ["geometry-type"], "Point"],
    });
    expect(map.getLayer("navigation-alert-line")).toMatchObject({
      filter: ["==", ["geometry-type"], "LineString"],
    });
  });

  it.each([
    { error: new Error("tile failed"), tile: {}, style: {} },
    { error: new Error("network failed"), sourceId: "basemap", style: {} },
    { error: new Error("source failed"), source: {}, style: {} },
    // sprite失敗もstyle由来のErrorEventとして伝播する。
    { error: new Error("HTTP 503"), style: {} },
    { error: new Error("Failed to fetch"), style: {} },
    { error: new Error("glyph failed"), tile: {}, sourceId: "basemap", style: {} },
    { error: new Error("unclassified resource failed") },
  ])("定義取得済みなら部分的な読込失敗で全面不可にしない: %o", async (event) => {
    const { map } = await renderLive();
    // isStyleLoadedは補助リソースや背景タイルの待機中にもfalseになる。
    map.loaded = false;
    act(() => map.emit("error", event));
    expect(screen.queryByText("地図を利用できません")).not.toBeInTheDocument();
    expect(map.remove).not.toHaveBeenCalled();
  });

  it("style.load前でも定義が取得済みならsprite失敗で全面不可にしない", async () => {
    render(<RouteMap {...mapProps(createDemoRouteResponse(request))} />);
    await waitFor(() => expect(live.maps).toHaveLength(1));
    const map = live.maps[0];
    map.loaded = false;
    act(() => map.emit("error", { error: new Error("HTTP 503"), style: {} }));
    expect(screen.queryByText("地図を利用できません")).not.toBeInTheDocument();
    expect(map.remove).not.toHaveBeenCalled();
  });

  it.each([
    "HTTP 503", "Failed to fetch", "Unexpected token '<'",
    'version: expected one of [8], 7 found',
    'layers[0].type: expected one of [fill, line], invalid found',
  ])("ルートstyleの取得・JSON解析・検証失敗で既存の利用不可表示にする: %s", async (message) => {
    const { map } = await renderLive();
    // 初回・完全再生成時のstyle取得・検証失敗では、定義が未取得になる。
    map.styleAvailable = false;
    map.loaded = false;
    act(() => map.emit("error", { error: new Error(message), style: {} }));
    expect(screen.getByText("地図を利用できません")).toBeVisible();
    expect(map.remove).toHaveBeenCalledTimes(1);
  });

  it.each([
    ["取得", { error: new Error("Failed to fetch") }],
    ["検証", { error: new Error('version: expected one of [8], 7 found'), style: {} }],
  ])("旧定義が残るstyle差分更新の%s失敗でも経路描画と選択を続ける", async (_name, event) => {
    const { map, props, rerender } = await renderLive();
    // 取得失敗のerrorはMapから、検証失敗は旧Styleから伝播しstyleが付く。
    // いずれも完全再生成は行われず、既存のsourceとlayerが残る。
    const source = map.getSource("route-fastest")!;
    const previous = structuredClone(source.data);
    const layer = map.getLayer("route-fastest-line");
    act(() => map.emit("error", event));
    expect(screen.queryByText("地図を利用できません")).not.toBeInTheDocument();
    expect(map.remove).not.toHaveBeenCalled();
    expect(map.getSource("route-fastest")).toBe(source);
    expect(source.data).toEqual(previous);
    expect(map.getLayer("route-fastest-line")).toBe(layer);
    expect(screen.getByRole("group", { name: "表示する経路" })).toBeVisible();

    // 旧スタイル上で選択変更後のデータも描画できる。
    rerender(<RouteMap {...props} selectedRouteId={props.result!.fastest_id} />);
    expect(source.data.features.find((feature) => feature.id === props.result!.fastest_id)?.properties?.selected).toBe(true);
    const selected = props.result!.candidates.find((route) => route.id === props.result!.fastest_id)!;
    expect(map.getSource("route-accidents")!.data.features).toEqual(selected.accident_points.features);
    map.queryRenderedFeatures.mockReturnValue([{ id: selected.id, layer: { id: "route-hit" } }]);
    act(() => map.emit("click", { point: { x: 0, y: 0 } }));
    expect(props.onSelectRoute).toHaveBeenCalledWith(selected.id);
    expect(map.remove).not.toHaveBeenCalled();
  });

  it("初回style取得失敗でも既存の利用不可表示と比較を残す", async () => {
    render(<RouteMap {...mapProps(createDemoRouteResponse(request))} />);
    await waitFor(() => expect(live.maps).toHaveLength(1));
    const map = live.maps[0];
    map.styleAvailable = false;
    map.loaded = false;
    act(() => map.emit("error", { error: new Error("Failed to fetch"), style: {} }));
    expect(screen.getByText("地図を利用できません")).toBeVisible();
    expect(screen.getByRole("group", { name: "表示する経路" })).toBeVisible();
    expect(map.remove).toHaveBeenCalledTimes(1);
  });

  it.each([
    ...["route-fastest", "route-safest", "route-high-risk", "route-accidents",
      "navigation-route", "navigation-alert", "current-location", "current-accuracy"]
      .map((sourceId) => ["error", { error: new Error("source failed"), sourceId, source: {}, style: {} }] as const),
    ["error", { error: new Error("source failed"), source: { id: "navigation-route" }, style: {} }],
    ["error", { error: new Error("tile failed"), sourceId: "current-location", tile: {}, style: {} }],
    ["error", { error: new Error("WebGL context lost"), sourceId: "basemap", tile: {}, style: {} }],
    ["error", { error: new Error("WebGL initialization failed") }],
    ["webglcontextlost", {}],
  ] as const)("必須source・WebGLの失敗で既存の利用不可表示にする: %s %o", async (name, event) => {
    const { map } = await renderLive();
    act(() => map.emit(name, event));
    expect(screen.getByText("地図を利用できません")).toBeVisible();
    expect(screen.getByRole("group", { name: "表示する経路" })).toBeVisible();
    expect(map.remove).toHaveBeenCalledTimes(1);
  });

  it("クリック処理を再登録せず、unmountで全イベントとMapを破棄する", async () => {
    const { map, unmount, props } = await renderLive();
    act(() => { map.emit("style.load"); map.emit("style.load"); });
    expect(map.listeners.get("click")!.size).toBe(1);
    map.queryRenderedFeatures.mockReturnValue([{ id: props.selectedRouteId, layer: { id: "route-hit" } }]);
    act(() => map.emit("click", { point: { x: 0, y: 0 } }));
    expect(props.onSelectRoute).toHaveBeenCalledTimes(1);
    unmount();
    expect(map.remove).toHaveBeenCalledTimes(1);
    for (const listeners of map.listeners.values()) expect(listeners.size).toBe(0);
    expect(map.off).toHaveBeenCalledTimes(map.on.mock.calls.length);
  });
});

describe("RouteMapのナビ表示（手順10・11）", () => {
  beforeEach(() => {
    live.maps.length = 0;
    vi.stubEnv("VITE_API_MODE", "live");
    vi.stubEnv("VITE_MAP_STYLE_URL", "https://example.invalid/style.json");
  });
  afterEach(() => { vi.unstubAllEnvs(); });

  function navLayers(lon: number, lat: number) {
    const layers = emptyNavigationLayers();
    layers.location.features.push({ type: "Feature", geometry: { type: "Point", coordinates: [lon, lat] }, properties: { kind: "simulation", onRoute: true } });
    return layers;
  }
  const easeTo = (map: MockMap) => (map as unknown as { easeTo: ReturnType<typeof vi.fn> }).easeTo;

  it("ナビ用4レイヤーのデータをそのままsourceへ流し込み、比較カードを隠し、ナビ終了で消去する", async () => {
    const { map, props, rerender } = await renderLive();
    const layers = navLayers(139.77, 35.69);
    layers.route.features.push({ type: "Feature", geometry: props.result!.candidates[0].geometry, properties: {} });
    const navigation = { layers, follow: true, onFollowChange: vi.fn() };
    rerender(<RouteMap {...props} navigation={navigation} />);
    expect(map.getSource("current-location")!.data).toEqual(layers.location);
    expect(map.getSource("navigation-route")!.data).toEqual(layers.route);
    expect(screen.queryByRole("group", { name: "表示する経路" })).not.toBeInTheDocument();
    expect(screen.getByLabelText("地図の凡例")).toHaveTextContent("現在地");
    rerender(<RouteMap {...props} />);
    for (const id of ["current-location", "current-accuracy", "navigation-route", "navigation-alert"])
      expect(map.getSource(id)!.data.features).toEqual([]);
    expect(screen.getByRole("group", { name: "表示する経路" })).toBeVisible();
  });

  it("追従ONでは現在地へ中心を寄せるだけで、fitBoundsを繰り返さない。手動パンで解除し、操作で復帰できる", async () => {
    const { map, props, rerender } = await renderLive();
    const onFollowChange = vi.fn();
    rerender(<RouteMap {...props} navigation={{ layers: navLayers(139.77, 35.69), follow: true, onFollowChange }} />);
    expect(easeTo(map)).toHaveBeenLastCalledWith({ center: [139.77, 35.69], duration: 300 });
    const fits = map.fitBounds.mock.calls.length;
    rerender(<RouteMap {...props} navigation={{ layers: navLayers(139.771, 35.691), follow: true, onFollowChange }} />);
    expect(easeTo(map)).toHaveBeenCalledTimes(2);
    expect(map.fitBounds).toHaveBeenCalledTimes(fits);
    act(() => map.emit("dragstart"));
    expect(onFollowChange).toHaveBeenCalledWith(false);
    rerender(<RouteMap {...props} navigation={{ layers: navLayers(139.772, 35.692), follow: false, onFollowChange }} />);
    expect(easeTo(map)).toHaveBeenCalledTimes(2);
    fireEvent.click(screen.getByRole("button", { name: "現在地に追従" }));
    expect(onFollowChange).toHaveBeenLastCalledWith(true);
  });

  it("経路と位置を描けない状態を上位へ伝える", async () => {
    const onUnavailableChange = vi.fn();
    const props = { ...mapProps(createDemoRouteResponse(request)), onUnavailableChange };
    const { map } = await renderLive(props);
    expect(onUnavailableChange).toHaveBeenLastCalledWith(false);
    act(() => map.emit("webglcontextlost", {}));
    expect(onUnavailableChange).toHaveBeenLastCalledWith(true);
  });
});
