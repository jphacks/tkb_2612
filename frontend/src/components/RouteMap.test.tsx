import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createDemoRouteResponse, DEMO_PLACES } from "../api/demo";
import { FASTEST_LABEL, SAFE_LABEL } from "./RouteCallouts";
import { RouteMap, type RouteMapProps } from "./RouteMap";
import type { RouteResponse } from "../lib/types";
import { emptyNavigationLayers, setNavigationLayers } from "../map/layers";
import { distanceToLineM, NEAR_ROUTE_M } from "../community/reports";

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
    showReports: true,
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
        card.textContent?.includes(SAFE_LABEL),
      )!;
      const fastest = cards.find((card) =>
        card.textContent?.includes(FASTEST_LABEL),
      )!;
      expect(considered).toHaveClass("route-considered");
      expect(fastest).toHaveClass("route-fastest");
      // Each card offers selection and a navigation start button.
      expect(
        within(fastest).getByRole("button", {
          name: `${FASTEST_LABEL}でナビを開始`,
        }),
      ).toBeInTheDocument();
      const line = screen.getByRole("button", {
        name: `A ${FASTEST_LABEL}を選択`,
      });
      expect(line).toHaveClass("route-fastest");
      fireEvent.pointerEnter(fastest);
      expect(line).toHaveClass("is-hovered");
      fireEvent.click(
        within(fastest).getByRole("button", {
          name: new RegExp(`^${FASTEST_LABEL}`),
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
    fireEvent.click(
      screen.getByRole("button", { name: `A ${FASTEST_LABEL}を選択` }),
    );
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

  it("shows the three attribution values in the accident popup", () => {
    const result = createDemoRouteResponse(request);
    const props = { ...mapProps(result), selectedRouteId: result.fastest_id };
    render(<RouteMap {...props} />);
    const route = result.candidates.find(
      (candidate) => candidate.id === result.fastest_id,
    )!;
    const expected = {
      edge: "道路",
      intersection: "交差点",
      unattributed: "経路の近く",
    };
    expect(
      route.accident_points.features.map(
        (point) => point.properties.attribution,
      ),
    ).toEqual(["edge", "intersection", "unattributed"]);
    for (const accident of route.accident_points.features) {
      fireEvent.click(
        screen.getByRole("button", {
          name: `事故記録 ${accident.id + 1} を表示`,
        }),
      );
      const popup = screen.getByRole("region", { name: "過去の事故記録" });
      const term = within(popup).getByText("発生場所");
      expect(term.nextElementSibling).toHaveTextContent(
        expected[accident.properties.attribution],
      );
    }
  });

  it("shows both accident legend entries and hides them with accident points", () => {
    const props = mapProps(createDemoRouteResponse(request));
    const { rerender } = render(<RouteMap {...props} />);
    const legend = () => screen.getByLabelText("地図の凡例");
    expect(within(legend()).getByText("経路上の事故")).toBeVisible();
    expect(within(legend()).getByText("経路の近くの事故")).toBeVisible();
    expect(within(legend()).queryByText("事故記録")).not.toBeInTheDocument();
    rerender(<RouteMap {...props} showAccidents={false} />);
    expect(
      within(legend()).queryByText("経路上の事故"),
    ).not.toBeInTheDocument();
    expect(
      within(legend()).queryByText("経路の近くの事故"),
    ).not.toBeInTheDocument();
  });

  it("draws only unattributed accident points smaller in the preview", () => {
    const result = createDemoRouteResponse(request);
    const props = { ...mapProps(result), selectedRouteId: result.fastest_id };
    const { container } = render(<RouteMap {...props} />);
    const route = result.candidates.find(
      (candidate) => candidate.id === result.fastest_id,
    )!;
    const dots = [
      ...container.querySelectorAll<SVGCircleElement>(".map-accident-dot"),
    ];
    expect(dots).toHaveLength(route.accident_points.features.length);
    route.accident_points.features.forEach((accident, index) => {
      const dot = dots[index];
      if (accident.properties.attribution === "unattributed") {
        expect(dot).toHaveAttribute("r", "5");
        expect(dot).toHaveClass("is-unattributed");
      } else {
        expect(dot).toHaveAttribute("r", "7");
        expect(dot).not.toHaveClass("is-unattributed");
      }
    });
    expect(
      route.accident_points.features.some(
        (accident) => accident.properties.attribution === "unattributed",
      ),
    ).toBe(true);
    expect(
      dots.filter((dot) => dot.classList.contains("is-unattributed")),
    ).toHaveLength(1);
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
        name: `A ${FASTEST_LABEL}・${SAFE_LABEL}（同一経路）を選択`,
      }),
    ).toHaveLength(1);
    rerender(<RouteMap {...props} unavailable />);
    expect(screen.getByText("地図を利用できません")).toBeVisible();
    expect(screen.getByRole("group", { name: "表示する経路" })).toBeVisible();
  });

  it("keeps connector holes outside enlarged accident pins, including their focus border", () => {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({ width: 960, height: 720 } as DOMRect);
    const result = createDemoRouteResponse(request);
    const { container, rerender } = render(<RouteMap {...mapProps(result)} selectedRouteId={result.fastest_id} />);
    for (let i = 0; i < 4; i++) fireEvent.click(screen.getByRole("button", { name: "地図を拡大" }));
    const hole = container.querySelector("mask circle")!;
    // At 3x zoom: radius 7 plus half of the focused 5px stroke => 28.5px.
    expect(Number(hole.getAttribute("r"))).toBeGreaterThanOrEqual(28.5);
    rerender(<RouteMap {...mapProps(result)} selectedRouteId={result.fastest_id} showAccidents={false} />);
    expect(container.querySelectorAll("mask circle")).toHaveLength(0);
  });

  it("tells that navigation is not available yet instead of doing nothing", () => {
    const props = mapProps(createDemoRouteResponse(request));
    render(<RouteMap {...props} />);
    fireEvent.click(
      screen.getByRole("button", { name: `${SAFE_LABEL}でナビを開始` }),
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
      "current-location", "current-accuracy", "navigation-route", "navigation-alert", "community-reports",
    ].sort());
    expect(map.layers.map((layer) => layer.id)).toEqual([
      "route-fastest-line", "route-safest-line", "route-hit", "navigation-route-line",
      "route-high-risk-line", "section-hit", "navigation-alert-line", "navigation-alert-point", "route-accidents-point",
      "accident-hit", "community-reports-point", "community-hit", "current-accuracy-fill", "current-location-point",
    ]);
    for (const id of ["route-safest", "current-location", "current-accuracy", "navigation-route", "navigation-alert", "community-reports"]) {
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
      paint: {
        "circle-radius": ["case", ["==", ["get", "attribution"], "unattributed"], 5, 7],
        "circle-color": "#fff",
        "circle-stroke-color": "#d97706",
        "circle-stroke-width": ["case", ["==", ["get", "attribution"], "unattributed"], 2, 3],
        "circle-stroke-opacity": ["case", ["==", ["get", "attribution"], "unattributed"], 0.55, 1],
      },
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
    expect(map.sources.size).toBe(9);
    expect(map.layers).toHaveLength(14);
    expect(map.getSource("current-location")!.data).toEqual(navigation.location);
    expect(map.getSource("current-location")!.data.features[0].geometry).toEqual({ type: "Point", coordinates: [141, 35] });
    expect(map.getSource("route-accidents")!.data.features).toEqual([]);
    expect(map.getSource("route-fastest")!.data.features.at(-1)?.id).toBe(nextProps.selectedRouteId);
    map.loaded = true;
    const calls = map.getSource("route-fastest")!.setData.mock.calls.length;
    act(() => { map.emit("styledata"); map.emit("styledata"); });
    expect(map.getSource("route-fastest")!.setData).toHaveBeenCalledTimes(calls);
    expect(map.layers).toHaveLength(14);
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

describe("RouteMap community reports", () => {
  afterEach(() => localStorage.clear());

  const [origin] = DEMO_PLACES;
  type StoredComment = { kind: "safe" | "danger"; text: string; created_at: string };
  const stored = (
    id: string,
    point: { lat: number; lon: number },
    kinds: ("safe" | "danger")[],
    comments: StoredComment[] = [],
  ) => ({
    id: id.repeat(32),
    lat: point.lat,
    lon: point.lon,
    created_at: "2026-11-04T09:15:00.000Z",
    votes: kinds.map((kind, index) => ({ voter: `voter-${id}-${index}-000000000`, kind })),
    comments: comments.map((comment, index) => ({ id: `${id}${index}`.padEnd(32, "0"), ...comment })),
  });
  const seed = (...reports: ReturnType<typeof stored>[]) =>
    localStorage.setItem("accilessnavi.safety-reports.v2", JSON.stringify(reports));
  const selectedLine = (props: RouteMapProps) =>
    props.result!.candidates.find((route) => route.id === props.selectedRouteId)!.geometry.coordinates;
  // 外接矩形の中（読み込む範囲）だが、選択中の経路からは72mより遠い地点
  const far = { lat: origin.lat - 0.0025, lon: origin.lon - 0.0025 };

  it("pins only reports near the selected route, in the majority color", async () => {
    const props = mapProps(createDemoRouteResponse(request));
    expect(distanceToLineM(far, selectedLine(props))).toBeGreaterThan(NEAR_ROUTE_M);
    seed(
      stored("a", origin, ["danger"]),
      stored("b", { lat: origin.lat + 0.0005, lon: origin.lon }, ["safe", "safe", "danger"]),
      stored("c", far, ["danger"]),
      stored("d", { lat: origin.lat + 1, lon: origin.lon }, ["danger"]),
    );
    render(<RouteMap {...props} />);
    const danger = await screen.findByRole("button", { name: "口コミ（安全0票・危険1票）を表示" });
    expect(danger.querySelector(".map-community-pin")).toHaveAttribute("fill", "#DC2626");
    expect(
      screen.getByRole("button", { name: "口コミ（安全2票・危険1票）を表示" }).querySelector(".map-community-pin"),
    ).toHaveAttribute("fill", "#16A34A");
    expect(screen.getAllByRole("button", { name: /^口コミ（/ })).toHaveLength(2);
    const legend = screen.getByLabelText("地図の凡例");
    expect(legend).toHaveTextContent("安全の口コミ");
    expect(legend).toHaveTextContent("危険の口コミ");
  });

  it("shows no community legend when there are no reports", async () => {
    render(<RouteMap {...mapProps(createDemoRouteResponse(request))} />);
    await act(async () => {});
    expect(screen.getByLabelText("地図の凡例")).not.toHaveTextContent("口コミ");
  });

  it("shows no community legend when the only reports are far from the selected route", async () => {
    seed(stored("c", far, ["danger"]));
    render(<RouteMap {...mapProps(createDemoRouteResponse(request))} />);
    await act(async () => {});
    expect(screen.queryByRole("button", { name: /^口コミ（/ })).not.toBeInTheDocument();
    expect(screen.getByLabelText("地図の凡例")).not.toHaveTextContent("口コミ");
  });

  it("opens the tally, post date and newest three comments when a pin is pressed", async () => {
    seed(
      stored("a", origin, ["safe", "danger", "danger"], [
        { kind: "safe", text: "いちばん古い", created_at: "2026-11-05T01:00:00.000Z" },
        { kind: "safe", text: "歩道が広い", created_at: "2026-11-06T01:00:00.000Z" },
        { kind: "danger", text: "朝は自転車が多い", created_at: "2026-11-07T01:00:00.000Z" },
        { kind: "danger", text: "見通しが悪い", created_at: "2026-11-07T16:00:00.000Z" },
      ]),
    );
    render(<RouteMap {...mapProps(createDemoRouteResponse(request))} />);
    fireEvent.click(await screen.findByRole("button", { name: /^口コミ（/ }));
    const popup = screen.getByRole("region", { name: "口コミ" });
    expect(popup).toHaveTextContent("2026年11月4日");
    expect(within(popup).getByLabelText("安全1票・危険2票")).toHaveTextContent("安全 1票危険 2票");
    const items = within(popup).getAllByRole("listitem");
    // 新しい順。日付は日本時間（16:00Zは翌日）
    expect(items.map((item) => item.textContent)).toEqual([
      "危険11/8見通しが悪い",
      "危険11/7朝は自転車が多い",
      "安全11/6歩道が広い",
    ]);
    expect(popup).toHaveTextContent("ほか 1件");
    expect(popup).not.toHaveTextContent("いちばん古い");
    fireEvent.click(within(popup).getByRole("button", { name: "口コミを閉じる" }));
    expect(screen.queryByRole("region", { name: "口コミ" })).not.toBeInTheDocument();
  });

  it("says so when a report has no comment text", async () => {
    seed(stored("a", origin, ["danger"]));
    render(<RouteMap {...mapProps(createDemoRouteResponse(request))} />);
    fireEvent.click(await screen.findByRole("button", { name: /^口コミ（/ }));
    const popup = screen.getByRole("region", { name: "口コミ" });
    expect(popup).toHaveTextContent("口コミの本文はありません");
    expect(popup).not.toHaveTextContent("ほか");
  });

  it("never shows the accident and report popups together", async () => {
    seed(stored("a", origin, ["danger"]));
    render(<RouteMap {...mapProps(createDemoRouteResponse(request))} />);
    fireEvent.click(await screen.findByRole("button", { name: /^口コミ（/ }));
    fireEvent.click(screen.getAllByRole("button", { name: /事故記録 .* を表示/ })[0]);
    expect(screen.getByRole("region", { name: "過去の事故記録" })).toBeVisible();
    expect(screen.queryByRole("region", { name: "口コミ" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /^口コミ（/ }));
    expect(screen.getByRole("region", { name: "口コミ" })).toBeVisible();
    expect(screen.queryByRole("region", { name: "過去の事故記録" })).not.toBeInTheDocument();
  });

  it("toggles the reports, and hiding them closes the popup and the legend", async () => {
    seed(stored("a", origin, ["danger"]));
    const onShowReports = vi.fn();
    const props = { ...mapProps(createDemoRouteResponse(request)), onShowReports };
    const { rerender } = render(<RouteMap {...props} />);
    fireEvent.click(await screen.findByRole("button", { name: /^口コミ（/ }));
    const toggle = screen.getByRole("button", { name: "口コミを地図に表示" });
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(toggle);
    expect(onShowReports).toHaveBeenCalledWith(false);
    rerender(<RouteMap {...props} showReports={false} />);
    expect(screen.getByRole("button", { name: "口コミを地図に表示" })).toHaveAttribute("aria-pressed", "false");
    expect(screen.queryByRole("button", { name: /^口コミ（/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "口コミ" })).not.toBeInTheDocument();
    expect(screen.getByLabelText("地図の凡例")).not.toHaveTextContent("口コミ");
    // 表示に戻しても、閉じたポップアップは開き直さない
    rerender(<RouteMap {...props} />);
    expect(await screen.findByRole("button", { name: /^口コミ（/ })).toBeVisible();
    expect(screen.queryByRole("region", { name: "口コミ" })).not.toBeInTheDocument();
  });

  it("closes the report popup when another route is selected", async () => {
    seed(stored("a", origin, ["danger"]));
    const props = mapProps(createDemoRouteResponse(request));
    const { rerender } = render(<RouteMap {...props} />);
    fireEvent.click(await screen.findByRole("button", { name: /^口コミ（/ }));
    rerender(<RouteMap {...props} selectedRouteId={props.result!.fastest_id} />);
    expect(screen.queryByRole("region", { name: "口コミ" })).not.toBeInTheDocument();
  });

  it("paints navigation lines below accident and report pins, with location on top", async () => {
    seed(stored("a", origin, ["danger"]));
    const props = mapProps(createDemoRouteResponse(request));
    const layers = emptyNavigationLayers();
    layers.route.features.push({ type: "Feature", geometry: props.result!.candidates[0].geometry, properties: {} });
    layers.alert.features.push({ type: "Feature", geometry: props.result!.candidates[0].geometry, properties: {} });
    layers.location.features.push({ type: "Feature", geometry: { type: "Point", coordinates: [origin.lon, origin.lat] }, properties: {} });
    const { container } = render(<RouteMap {...props} navigation={{ layers, follow: true, onFollowChange: vi.fn() }} />);
    await screen.findByRole("button", { name: /^口コミ（/ });
    const before = (lower: string, upper: string) => {
      expect(container.querySelector(lower)!.compareDocumentPosition(container.querySelector(upper)!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    };
    for (const line of [".map-nav-route", ".map-nav-alert"]) {
      before(line, ".map-accident-dot");
      before(line, ".map-community-pin");
    }
    before(".map-accident-dot", ".map-community-pin");
    before(".map-community-pin", ".map-nav-location");
  });

  it("keeps the same reports and popup during navigation", async () => {
    seed(stored("a", origin, ["danger"], [
      { kind: "danger", text: "見通しが悪い", created_at: "2026-11-07T01:00:00.000Z" },
    ]));
    const props = mapProps(createDemoRouteResponse(request));
    render(
      <RouteMap
        {...props}
        onShowReports={vi.fn()}
        navigation={{ layers: emptyNavigationLayers(), follow: true, onFollowChange: vi.fn() }}
      />,
    );
    fireEvent.click(await screen.findByRole("button", { name: /^口コミ（/ }));
    expect(screen.getByRole("region", { name: "口コミ" })).toHaveTextContent("見通しが悪い");
    expect(screen.getByRole("button", { name: "口コミを地図に表示" })).toBeVisible();
  });

  describe("on the live map", () => {
    beforeEach(() => {
      live.maps.length = 0;
      vi.stubEnv("VITE_API_MODE", "live");
      vi.stubEnv("VITE_MAP_STYLE_URL", "https://example.invalid/style.json");
    });
    afterEach(() => { vi.unstubAllEnvs(); });

    it("draws only nearby reports with their id, opens one on click, and shows both toggles", async () => {
      const props = {
        ...mapProps(createDemoRouteResponse(request)),
        onShowAccidents: vi.fn(),
        onShowReports: vi.fn(),
      };
      seed(stored("a", origin, ["danger"]), stored("c", far, ["danger"]));
      const { map } = await renderLive(props);
      await waitFor(() =>
        expect(map.getSource("community-reports")!.data.features).toHaveLength(1),
      );
      expect(map.getSource("community-reports")!.data.features[0]).toEqual({
        type: "Feature",
        geometry: { type: "Point", coordinates: [origin.lon, origin.lat] },
        properties: { reportId: "a".repeat(32), color: "#DC2626" },
      });
      const toggles = screen.getByRole("group", { name: "地図の表示切り替え" });
      expect(within(toggles).getByRole("button", { name: "事故点を地図に表示" })).toBeVisible();
      expect(within(toggles).getByRole("button", { name: "口コミを地図に表示" })).toBeVisible();
      // 事故点と重なっていても口コミを優先する
      map.queryRenderedFeatures.mockReturnValue([
        { id: 0, layer: { id: "accident-hit" }, properties: {} },
        { layer: { id: "community-hit" }, properties: { reportId: "a".repeat(32) } },
      ]);
      act(() => map.emit("click", { point: { x: 0, y: 0 } }));
      expect(screen.getByRole("region", { name: "口コミ" })).toBeVisible();
      expect(screen.queryByRole("region", { name: "過去の事故記録" })).not.toBeInTheDocument();
    });

    it("moves connector holes with live map pins and removes holes for hidden pins", async () => {
      seed(stored("a", origin, ["danger"]));
      const props = mapProps(createDemoRouteResponse(request));
      const { map, container, rerender } = await renderLive(props);
      await waitFor(() => expect(container.querySelectorAll('mask circle[r="10"]')).toHaveLength(1));
      const reportHole = () => container.querySelector('mask circle[r="10"]')!;
      expect(reportHole()).toHaveAttribute("cx", "100");
      vi.spyOn(mapInstance(map), "project").mockReturnValue({ x: 250, y: 300 } as import("maplibre-gl").Point);
      act(() => map.emit("move"));
      expect(reportHole()).toHaveAttribute("cx", "250");
      expect(reportHole()).toHaveAttribute("cy", "300");
      rerender(<RouteMap {...props} showReports={false} showAccidents={false} />);
      expect(container.querySelectorAll("mask circle")).toHaveLength(0);
    });

    it("clears the layer when the reports are hidden", async () => {
      seed(stored("a", origin, ["danger"]));
      const props = mapProps(createDemoRouteResponse(request));
      const { map, rerender } = await renderLive(props);
      await waitFor(() =>
        expect(map.getSource("community-reports")!.data.features).toHaveLength(1),
      );
      rerender(<RouteMap {...props} showReports={false} />);
      expect(map.getSource("community-reports")!.data.features).toEqual([]);
    });
  });
});
