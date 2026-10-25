import type { FeatureCollection, LineString, Point, Polygon } from "geojson";
import type {
  GeoJSONSource,
  LayerSpecification,
  LineLayerSpecification,
  Map,
} from "maplibre-gl";
import type { RouteResponse } from "../lib/types";
import { routeColors, routeTone } from "../components/RouteCallouts";

export type NavigationLayers = {
  location: FeatureCollection<Point>;
  accuracy: FeatureCollection<Polygon>;
  route: FeatureCollection<LineString>;
  alert: FeatureCollection<LineString | Point>;
};

export function emptyNavigationLayers(): NavigationLayers {
  return {
    location: empty(),
    accuracy: empty(),
    route: empty(),
    alert: empty(),
  };
}

function empty(): FeatureCollection<never> {
  return { type: "FeatureCollection", features: [] };
}

const linePaint: LineLayerSpecification["paint"] = {
  "line-color": ["get", "color"],
  "line-width": ["case", ["get", "selected"], 7, 4],
  "line-opacity": 1,
};

// 見た目維持の暫定として全候補をroute-fastestに入れ、route-safestは空にする（6章と異なる）。
// 選定2経路化は利用者判断待ち。合意後はfastest_id/safest_idのgeometryを各sourceへ分割する。
export const mapLayers: LayerSpecification[] = [
  {
    id: "route-fastest-line",
    type: "line",
    source: "route-fastest",
    paint: { ...linePaint },
    layout: { "line-cap": "round", "line-join": "round" },
  },
  {
    id: "route-safest-line",
    type: "line",
    source: "route-safest",
    paint: { ...linePaint },
    layout: { "line-cap": "round", "line-join": "round" },
  },
  {
    id: "route-hit",
    type: "line",
    source: "route-fastest",
    paint: { "line-width": 22, "line-opacity": 0 },
  },
  {
    id: "navigation-route-line",
    type: "line",
    source: "navigation-route",
    paint: { "line-color": "#2563eb", "line-width": 7 },
  },
  {
    id: "route-high-risk-line",
    type: "line",
    source: "route-high-risk",
    paint: { "line-color": "#d97706", "line-width": ["case", ["get", "selected"], 12, 7] },
    layout: { "line-cap": "round" },
  },
  {
    id: "section-hit",
    type: "line",
    source: "route-high-risk",
    paint: { "line-width": 28, "line-opacity": 0 },
  },
  {
    id: "navigation-alert-line",
    type: "line",
    source: "navigation-alert",
    filter: ["==", ["geometry-type"], "LineString"],
    paint: { "line-color": "#d97706", "line-width": 7 },
  },
  {
    id: "navigation-alert-point",
    type: "circle",
    source: "navigation-alert",
    filter: ["==", ["geometry-type"], "Point"],
    paint: { "circle-color": "#d97706", "circle-radius": 7 },
  },
  {
    id: "route-accidents-point",
    type: "circle",
    source: "route-accidents",
    paint: {
      "circle-radius": 7,
      "circle-color": "#fff",
      "circle-stroke-color": "#d97706",
      "circle-stroke-width": 3,
    },
  },
  {
    id: "accident-hit",
    type: "circle",
    source: "route-accidents",
    paint: { "circle-radius": 22, "circle-opacity": 0 },
  },
  {
    id: "current-accuracy-fill",
    type: "fill",
    source: "current-accuracy",
    paint: { "fill-color": "#2563eb", "fill-opacity": 0.15 },
  },
  {
    id: "current-location-point",
    type: "circle",
    source: "current-location",
    paint: { "circle-color": "#2563eb", "circle-radius": 7 },
  },
];

type LayerState = {
  data: Record<string, FeatureCollection>;
  revisions: Record<string, number>;
  applied: globalThis.Map<
    string,
    { source: GeoJSONSource; revision: number }
  >;
  restoring: boolean;
  styleReady: boolean;
};
const states = new WeakMap<Map, LayerState>();

function stateFor(map: Map): LayerState {
  let state = states.get(map);
  if (!state) {
    state = {
      data: Object.fromEntries(
        mapLayers.map((layer) => [
          "source" in layer ? layer.source as string : "",
          empty(),
        ]),
      ),
      revisions: {},
      applied: new globalThis.Map(),
      restoring: false,
      styleReady: false,
    };
    states.set(map, state);
  }
  return state;
}

export function markMapStyleLoading(map: Map): void {
  stateFor(map).styleReady = false;
}

// isStyleLoadedはタイル待ちでもfalseになるため、style.load以降は定義の準備完了を保持する。
export function restoreMapLayers(map: Map, styleReady = false): void {
  const state = stateFor(map);
  if (styleReady) state.styleReady = true;
  if (state.restoring || (!state.styleReady && !map.isStyleLoaded())) return;
  state.restoring = true;
  try {
    for (const [id, data] of Object.entries(state.data)) {
      if (!map.getSource(id)) {
        map.addSource(id, { type: "geojson", data: empty() });
      }
      const source = map.getSource(id) as GeoJSONSource;
      const applied = state.applied.get(id);
      const revision = state.revisions[id] ?? 0;
      if (applied?.source !== source || applied?.revision !== revision) {
        // 明示更新の世代を記録し、setData由来のstyledataでは再投入しない。
        state.applied.set(id, { source, revision });
        source.setData(data);
      }
    }
    for (let index = mapLayers.length - 1; index >= 0; index--) {
      const layer = mapLayers[index];
      if (!map.getLayer(layer.id)) {
        const before = mapLayers
          .slice(index + 1)
          .find((next) => map.getLayer(next.id));
        map.addLayer(layer, before?.id);
      }
    }
  } finally {
    state.restoring = false;
  }
}

function updateData(state: LayerState, id: string, data: FeatureCollection): void {
  state.data[id] = data;
  state.revisions[id] = (state.revisions[id] ?? 0) + 1;
}

export function setRouteLayers(map: Map, props: {
  result: RouteResponse | null;
  selectedRouteId: string | null;
  selectedSectionId: string | null;
  showAccidents: boolean;
}): void {
  const { result, selectedRouteId, selectedSectionId, showAccidents } = props;
  const selected = result?.candidates.find((route) => route.id === selectedRouteId);
  const routes = [...(result?.candidates ?? [])].sort(
    (a, b) => Number(a.id === selectedRouteId) - Number(b.id === selectedRouteId),
  );
  const data: Record<string, FeatureCollection> = {
    "route-fastest": {
      type: "FeatureCollection",
      features: routes.map((route) => ({
        type: "Feature",
        id: route.id,
        geometry: route.geometry,
        properties: {
          selected: route.id === selectedRouteId,
          color: routeColors[routeTone(route.id, result!)],
        },
      })),
    },
    "route-high-risk": {
      type: "FeatureCollection",
      features: (selected?.high_risk_sections.features ?? []).map((section) => ({
        ...section,
        properties: {
          ...section.properties,
          selected: section.id === selectedSectionId,
        },
      })),
    },
    // 候補横断の重複排除は表示対象を変えるため、選択中の事故点をそのまま維持する。
    "route-accidents": {
      type: "FeatureCollection",
      features: showAccidents ? selected?.accident_points.features ?? [] : [],
    },
  };
  const state = stateFor(map);
  for (const [id, collection] of Object.entries(data)) updateData(state, id, collection);
  restoreMapLayers(map);
}

// 差分指定。空のFeatureCollectionを渡すと消去でき、style再読込中も最新値を保持する。
export function setNavigationLayers(map: Map, data: Partial<NavigationLayers>): void {
  const state = stateFor(map);
  if (data.location) updateData(state, "current-location", data.location);
  if (data.accuracy) updateData(state, "current-accuracy", data.accuracy);
  if (data.route) updateData(state, "navigation-route", data.route);
  if (data.alert) updateData(state, "navigation-alert", data.alert);
  restoreMapLayers(map);
}

export function disposeMapLayers(map: Map): void {
  states.delete(map);
}
