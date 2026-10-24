import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from "react";
import { Crosshair, Eye, EyeOff, MapPin, Minus, Plus, X } from "lucide-react";
import type { GeoJSONSource, Map as MapLibreMap } from "maplibre-gl";
import type {
  AccidentFeature,
  RouteCandidate,
  RouteResponse,
} from "../lib/types";
import "maplibre-gl/dist/maplibre-gl.css";
// v6のESM版はworkerを import.meta.url からの相対で探すので、ビルドに含めてURLを渡す
import maplibreWorkerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?url";
import "./map.css";
import {
  RouteCallouts,
  featuredRoutes,
  routeAnchor,
  routeColors,
  routeTone,
  type MapPoint,
  type MapSize,
} from "./RouteCallouts";

export type RouteMapProps = {
  result: RouteResponse | null;
  selectedRouteId: string | null;
  selectedSectionId: string | null;
  onSelectRoute: (id: string) => void;
  onSelectSection: (id: string | null) => void;
  showAccidents: boolean;
  onShowAccidents?: (show: boolean) => void;
  unavailable: boolean;
};

type Coordinate = number[];
const WIDTH = 960;
const HEIGHT = 720;

function coordinateBounds(coordinates: Coordinate[]) {
  if (!coordinates.length) return { minX: 0, maxX: 1, minY: 0, maxY: 1 };
  return coordinates.reduce(
    (bounds, [x, y]) => ({
      minX: Math.min(bounds.minX, x),
      maxX: Math.max(bounds.maxX, x),
      minY: Math.min(bounds.minY, y),
      maxY: Math.max(bounds.maxY, y),
    }),
    { minX: Infinity, maxX: -Infinity, minY: Infinity, maxY: -Infinity },
  );
}

/** The schematic preserves relative geometry, without inventing a street network. */
function makeProjection(routes: RouteCandidate[]) {
  const coordinates = routes.flatMap((route) => route.geometry.coordinates);
  const { minX, maxX, minY, maxY } = coordinateBounds(coordinates);
  const spanX = Math.max(maxX - minX, 0.0001);
  const spanY = Math.max(maxY - minY, 0.0001);
  const scale = Math.min((WIDTH - 160) / spanX, (HEIGHT - 180) / spanY);
  return ([x, y]: Coordinate) => [
    WIDTH / 2 + (x - (maxX + minX) / 2) * scale,
    HEIGHT / 2 - (y - (maxY + minY) / 2) * scale,
  ];
}

function keyActivate(event: KeyboardEvent<SVGGElement>, action: () => void) {
  if (event.key === "Enter" || event.key === " ") {
    event.preventDefault();
    action();
  }
}

function routeLabel(route: RouteCandidate, result: RouteResponse) {
  if (route.id === result.fastest_id && route.id === result.safest_id)
    return "最短時間・事故履歴を考慮（同一経路）";
  if (route.id === result.fastest_id) return "最短時間";
  if (route.id === result.safest_id) return "事故履歴を考慮";
  return `候補 ${route.id.replace("route-", "")}`;
}

function AccidentPopup({
  accident,
  onClose,
}: {
  accident: AccidentFeature;
  onClose: () => void;
}) {
  const fact = accident.properties;
  const date = String(fact.date);
  const dateLabel = `${date.slice(0, 4)}年${Number(date.slice(4, 6))}月${Number(date.slice(6, 8))}日`;
  return (
    <section className="map-accident-popup" aria-label="過去の事故記録">
      <div className="map-popup-heading">
        <strong>過去の事故記録</strong>
        <button type="button" onClick={onClose} aria-label="事故記録を閉じる">
          <X size={18} />
        </button>
      </div>
      <dl>
        <dt>日付</dt>
        <dd>{dateLabel}</dd>
        <dt>天候</dt>
        <dd>{fact.weather}</dd>
        <dt>時間帯</dt>
        <dd>{fact.period}</dd>
        <dt>死傷区分</dt>
        <dd>{fact.deaths > 0 ? "死亡事故" : "負傷事故"}</dd>
        <dt>死者数</dt>
        <dd>{fact.deaths}人</dd>
        <dt>負傷者数</dt>
        <dd>{fact.injuries}人</dd>
      </dl>
    </section>
  );
}

type LiveMapProps = RouteMapProps & {
  styleUrl: string;
  onAccident: (id: number) => void;
  onUnavailable: () => void;
  onPositions: (positions: Record<string, MapPoint>) => void;
};

function LiveMap(props: LiveMapProps) {
  const container = useRef<HTMLDivElement>(null);
  const mapRef = useRef<MapLibreMap | null>(null);
  const current = useRef(props);
  const [ready, setReady] = useState(false);
  current.current = props;

  useEffect(() => {
    let disposed = false;
    let map: MapLibreMap | undefined;
    void import("maplibre-gl")
      .then((maplibre) => {
        if (disposed || !container.current) return;
        maplibre.setWorkerUrl(maplibreWorkerUrl);
        map = new maplibre.Map({
          container: container.current,
          style: props.styleUrl,
          center: [139.7671, 35.6812],
          zoom: 12,
          attributionControl: false,
        });
        mapRef.current = map;
        const reportPositions = () => {
          const result = current.current.result;
          if (!map || !result) return;
          current.current.onPositions(
            Object.fromEntries(
              featuredRoutes(result).map((route) => {
                const point = map!.project(
                  routeAnchor(route, route.id === result.safest_id ? 0.6 : 0.4),
                );
                return [route.id, { x: point.x, y: point.y }];
              }),
            ),
          );
        };
        map.on("move", reportPositions);
        map.on("resize", reportPositions);
        map.addControl(
          new maplibre.NavigationControl({ showCompass: false }),
          "top-right",
        );
        map.addControl(
          new maplibre.AttributionControl({ compact: false }),
          "bottom-right",
        );
        map.on("load", () => {
          if (disposed || !map) return;
          const empty = { type: "FeatureCollection" as const, features: [] };
          map.addSource("routes", { type: "geojson", data: empty });
          map.addSource("sections", { type: "geojson", data: empty });
          map.addSource("accidents", { type: "geojson", data: empty });
          map.addLayer({
            id: "route-lines",
            type: "line",
            source: "routes",
            paint: {
              "line-color": ["get", "color"],
              "line-width": ["case", ["get", "selected"], 7, 4],
              "line-opacity": 1,
            },
            layout: { "line-cap": "round", "line-join": "round" },
          });
          map.addLayer({
            id: "route-hit",
            type: "line",
            source: "routes",
            paint: { "line-width": 22, "line-opacity": 0 },
          });
          map.addLayer({
            id: "section-lines",
            type: "line",
            source: "sections",
            paint: {
              "line-color": "#d97706",
              "line-width": ["case", ["get", "selected"], 12, 7],
            },
            layout: { "line-cap": "round" },
          });
          map.addLayer({
            id: "section-hit",
            type: "line",
            source: "sections",
            paint: { "line-width": 28, "line-opacity": 0 },
          });
          map.addLayer({
            id: "accident-points",
            type: "circle",
            source: "accidents",
            paint: {
              "circle-radius": 7,
              "circle-color": "#fff",
              "circle-stroke-color": "#d97706",
              "circle-stroke-width": 3,
            },
          });
          map.addLayer({
            id: "accident-hit",
            type: "circle",
            source: "accidents",
            paint: { "circle-radius": 22, "circle-opacity": 0 },
          });
          map.on("click", (event) => {
            if (!map) return;
            const features = map.queryRenderedFeatures(event.point, {
              layers: ["accident-hit", "section-hit", "route-hit"],
            });
            const accident = features.find(
              (feature) => feature.layer.id === "accident-hit",
            );
            if (accident) {
              current.current.onAccident(Number(accident.id));
              return;
            }
            const section = features.find(
              (feature) => feature.layer.id === "section-hit",
            );
            if (section) {
              current.current.onSelectSection(String(section.id));
              return;
            }
            const route = features.find(
              (feature) => feature.layer.id === "route-hit",
            );
            if (route) current.current.onSelectRoute(String(route.id));
          });
          setReady(true);
        });
        map.on("error", () => {
          if (!disposed) current.current.onUnavailable();
        });
      })
      .catch(() => {
        if (!disposed) current.current.onUnavailable();
      });
    return () => {
      disposed = true;
      map?.remove();
      mapRef.current = null;
    };
  }, [props.styleUrl]);

  useEffect(() => {
    if (
      !ready ||
      !mapRef.current ||
      !container.current ||
      typeof ResizeObserver === "undefined"
    )
      return;
    const observer = new ResizeObserver(() => mapRef.current?.resize());
    observer.observe(container.current);
    return () => observer.disconnect();
  }, [ready]);

  useEffect(() => {
    const map = mapRef.current;
    if (!ready || !map) return;
    const result = props.result;
    const selected = result?.candidates.find(
      (route) => route.id === props.selectedRouteId,
    );
    const routes = result?.candidates ?? [];
    const selectedLast = [...routes].sort(
      (a, b) =>
        Number(a.id === props.selectedRouteId) -
        Number(b.id === props.selectedRouteId),
    );
    (map.getSource("routes") as GeoJSONSource).setData({
      type: "FeatureCollection",
      features: selectedLast.map((route) => ({
        type: "Feature",
        id: route.id,
        geometry: route.geometry,
        properties: {
          selected: route.id === props.selectedRouteId,
          color: routeColors[routeTone(route.id, result!)],
        },
      })),
    });
    (map.getSource("sections") as GeoJSONSource).setData({
      type: "FeatureCollection",
      features: (selected?.high_risk_sections.features ?? []).map(
        (section) => ({
          ...section,
          properties: {
            ...section.properties,
            selected: section.id === props.selectedSectionId,
          },
        }),
      ),
    });
    (map.getSource("accidents") as GeoJSONSource).setData({
      type: "FeatureCollection",
      features: props.showAccidents
        ? (selected?.accident_points.features ?? [])
        : [],
    });
  }, [
    ready,
    props.result,
    props.selectedRouteId,
    props.selectedSectionId,
    props.showAccidents,
  ]);

  useEffect(() => {
    const map = mapRef.current;
    const points = props.result?.candidates.flatMap(
      (route) => route.geometry.coordinates,
    );
    if (!ready || !map || !points?.length) return;
    const { minX, maxX, minY, maxY } = coordinateBounds(points);
    map.fitBounds(
      [
        [minX, minY],
        [maxX, maxY],
      ],
      {
        padding: 64,
        duration: 0,
        maxZoom: 16,
      },
    );
  }, [ready, props.result]);

  useEffect(() => {
    const map = mapRef.current;
    const section = props.result?.candidates
      .find((route) => route.id === props.selectedRouteId)
      ?.high_risk_sections.features.find(
        (item) => item.id === props.selectedSectionId,
      );
    if (!ready || !map || !section) return;
    const [from, to] = section.geometry.coordinates;
    map.easeTo({
      center: [(from[0] + to[0]) / 2, (from[1] + to[1]) / 2],
      duration: 150,
    });
  }, [ready, props.result, props.selectedRouteId, props.selectedSectionId]);

  return (
    <div
      ref={container}
      className="route-live-map"
      role="region"
      aria-label="経路地図"
    />
  );
}

export function RouteMap(props: RouteMapProps) {
  const {
    result,
    selectedRouteId,
    selectedSectionId,
    onSelectRoute,
    onSelectSection,
    showAccidents,
    onShowAccidents,
    unavailable,
  } = props;
  const routes = result?.candidates ?? [];
  const selectedRoute = routes.find((route) => route.id === selectedRouteId);
  const [accidentId, setAccidentId] = useState<number | null>(null);
  const [liveFailed, setLiveFailed] = useState(false);
  const [zoom, setZoom] = useState(1);
  const [hoveredRouteId, setHoveredRouteId] = useState<string | null>(null);
  const [livePositions, setLivePositions] = useState<Record<string, MapPoint>>(
    {},
  );
  const shell = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState<MapSize>({ width: WIDTH, height: HEIGHT });
  useLayoutEffect(() => {
    const element = shell.current;
    if (!element) return;
    const measure = () => {
      const { width, height } = element.getBoundingClientRect();
      if (width && height) setSize({ width, height });
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  const project = useMemo(
    () => makeProjection(result?.candidates ?? []),
    [result],
  );
  const path = (coordinates: Coordinate[]) =>
    coordinates.map((point) => project(point).join(",")).join(" ");
  const selectedAccident = showAccidents
    ? selectedRoute?.accident_points.features.find(
        (point) => point.id === accidentId,
      )
    : undefined;
  const selectedSection = selectedRoute?.high_risk_sections.features.find(
    (section) => section.id === selectedSectionId,
  );
  const sectionCenter = selectedSection
    ? project([
        (selectedSection.geometry.coordinates[0][0] +
          selectedSection.geometry.coordinates[1][0]) /
          2,
        (selectedSection.geometry.coordinates[0][1] +
          selectedSection.geometry.coordinates[1][1]) /
          2,
      ])
    : null;
  const center = sectionCenter ?? [WIDTH / 2, HEIGHT / 2];
  const actualZoom = selectedSection ? Math.max(zoom, 1.3) : zoom;
  const viewBox = `${center[0] - WIDTH / actualZoom / 2} ${center[1] - HEIGHT / actualZoom / 2} ${WIDTH / actualZoom} ${HEIGHT / actualZoom}`;
  const styleUrl =
    import.meta.env.VITE_API_MODE === "live"
      ? import.meta.env.VITE_MAP_STYLE_URL
      : undefined;
  const disabled = unavailable || liveFailed;
  const scale = Math.min(
    size.width / (WIDTH / actualZoom),
    size.height / (HEIGHT / actualZoom),
  );
  const positions = result
    ? Object.fromEntries(
        featuredRoutes(result).map((route) => {
          const point = project(
            routeAnchor(route, route.id === result.safest_id ? 0.6 : 0.4),
          );
          return [
            route.id,
            {
              x: size.width / 2 + (point[0] - center[0]) * scale,
              y: size.height / 2 + (point[1] - center[1]) * scale,
            },
          ];
        }),
      )
    : {};

  useEffect(() => {
    setAccidentId(null);
  }, [result, selectedRouteId]);
  useEffect(() => {
    setZoom(1);
    setLiveFailed(false);
    setHoveredRouteId(null);
  }, [result]);

  // Identical geometries represent one visible line even if the API supplies duplicate candidates.
  const visibleRoutes = [...routes]
    .sort(
      (a, b) =>
        Number(a.id === selectedRouteId) - Number(b.id === selectedRouteId),
    )
    .filter(
      (route, index, all) =>
        !all
          .slice(index + 1)
          .some(
            (other) =>
              JSON.stringify(other.geometry) === JSON.stringify(route.geometry),
          ),
    );
  const referenceRoute = selectedRoute ?? routes[0];
  const origin = referenceRoute?.geometry.coordinates[0];
  const dest = referenceRoute?.geometry.coordinates.at(-1);
  const reset = () => {
    setZoom(1);
    onSelectSection(null);
  };

  return (
    <div
      ref={shell}
      className={`route-map-shell${styleUrl ? " is-live-map" : ""}`}
    >
      {disabled ? (
        <div className="map-unavailable" role="status">
          <MapPin size={28} aria-hidden="true" />
          <strong>地図を利用できません</strong>
        </div>
      ) : styleUrl ? (
        <LiveMap
          {...props}
          styleUrl={styleUrl}
          onAccident={setAccidentId}
          onUnavailable={() => setLiveFailed(true)}
          onPositions={setLivePositions}
        />
      ) : (
        <div className="route-schematic-canvas">
          <svg
            className="route-schematic"
            viewBox={viewBox}
            aria-label="地図プレビュー（模式図・実在道路ではありません）"
          >
            <defs>
              <pattern
                id="map-grid"
                width="80"
                height="80"
                patternUnits="userSpaceOnUse"
              >
                <path
                  d="M80 0H0V80"
                  fill="none"
                  stroke="#e2e8f0"
                  strokeWidth="1"
                />
              </pattern>
            </defs>
            <rect
              x="-10000"
              y="-10000"
              width="20000"
              height="20000"
              fill="url(#map-grid)"
            />
            {result &&
              visibleRoutes.map((route) => (
                <g
                  key={route.id}
                  className={`map-route-button route-${routeTone(route.id, result)}${route.id === selectedRouteId ? " is-selected" : ""}${route.id === hoveredRouteId ? " is-hovered" : ""}`}
                  role="button"
                  tabIndex={0}
                  aria-label={`${routeLabel(route, result)}を選択`}
                  aria-pressed={route.id === selectedRouteId}
                  onClick={() => onSelectRoute(route.id)}
                  onPointerEnter={() => setHoveredRouteId(route.id)}
                  onPointerLeave={() => setHoveredRouteId(null)}
                  onFocus={() => setHoveredRouteId(route.id)}
                  onBlur={() => setHoveredRouteId(null)}
                  onKeyDown={(event) =>
                    keyActivate(event, () => onSelectRoute(route.id))
                  }
                >
                  <title>{routeLabel(route, result)}</title>
                  <polyline
                    points={path(route.geometry.coordinates)}
                    className="map-route-line"
                    vectorEffect="non-scaling-stroke"
                  />
                  <polyline
                    points={path(route.geometry.coordinates)}
                    className="map-route-hit"
                    vectorEffect="non-scaling-stroke"
                  />
                </g>
              ))}
            {selectedRoute?.high_risk_sections.features.map(
              (section, index) => {
                const from = project(section.geometry.coordinates[0]);
                const to = project(section.geometry.coordinates[1]);
                const activate = () =>
                  onSelectSection(
                    section.id === selectedSectionId ? null : section.id,
                  );
                return (
                  <g
                    key={section.id}
                    className={`map-section-button${section.id === selectedSectionId ? " is-selected" : ""}`}
                    role="button"
                    tabIndex={0}
                    aria-label={`注意箇所 ${index + 1} を選択`}
                    aria-pressed={section.id === selectedSectionId}
                    onClick={activate}
                    onKeyDown={(event) => keyActivate(event, activate)}
                  >
                    <polyline
                      points={path(section.geometry.coordinates)}
                      className="map-section-line"
                      vectorEffect="non-scaling-stroke"
                    />
                    <polyline
                      points={path(section.geometry.coordinates)}
                      className="map-section-hit"
                      vectorEffect="non-scaling-stroke"
                    />
                    <circle
                      cx={(from[0] + to[0]) / 2}
                      cy={(from[1] + to[1]) / 2}
                      r="12"
                      className="map-section-marker"
                    />
                    <text
                      x={(from[0] + to[0]) / 2}
                      y={(from[1] + to[1]) / 2 + 4}
                      textAnchor="middle"
                      className="map-section-number"
                    >
                      {index + 1}
                    </text>
                  </g>
                );
              },
            )}
            {origin && (
              <g aria-label="出発地" className="map-endpoint">
                <circle
                  cx={project(origin)[0]}
                  cy={project(origin)[1]}
                  r="12"
                />
                <text
                  x={project(origin)[0]}
                  y={project(origin)[1] - 24}
                  textAnchor="middle"
                >
                  出発地
                </text>
              </g>
            )}
            {dest && (
              <g aria-label="目的地" className="map-endpoint is-destination">
                <circle cx={project(dest)[0]} cy={project(dest)[1]} r="12" />
                <text
                  x={project(dest)[0]}
                  y={project(dest)[1] - 24}
                  textAnchor="middle"
                >
                  目的地
                </text>
              </g>
            )}
            {showAccidents &&
              selectedRoute?.accident_points.features.map((point) => {
                const [x, y] = project(point.geometry.coordinates);
                const activate = () => setAccidentId(point.id);
                return (
                  <g
                    key={point.id}
                    className="map-accident-button"
                    role="button"
                    tabIndex={0}
                    aria-label={`事故記録 ${point.id + 1} を表示`}
                    onClick={activate}
                    onKeyDown={(event) => keyActivate(event, activate)}
                  >
                    <circle
                      cx={x}
                      cy={y}
                      r="7"
                      fill="transparent"
                      stroke="transparent"
                      strokeWidth="44"
                      vectorEffect="non-scaling-stroke"
                    />
                    <circle cx={x} cy={y} r="7" className="map-accident-dot" />
                  </g>
                );
              })}
          </svg>
        </div>
      )}
      {result && (
        <RouteCallouts
          result={result}
          positions={disabled ? {} : styleUrl ? livePositions : positions}
          size={size}
          selectedRouteId={selectedRouteId}
          hoveredRouteId={hoveredRouteId}
          onSelectRoute={onSelectRoute}
          onHoverRoute={setHoveredRouteId}
          detached={disabled}
        />
      )}
      {!disabled && !styleUrl && (
        <div className="map-preview-label">地図プレビュー</div>
      )}
      {!disabled && !styleUrl && (
        <div className="map-controls" aria-label="地図の表示操作">
          <button
            type="button"
            aria-label="地図を拡大"
            disabled={zoom >= 3}
            onClick={() => setZoom((value) => Math.min(3, value + 0.5))}
          >
            <Plus size={20} />
          </button>
          <button
            type="button"
            aria-label="地図を縮小"
            disabled={zoom <= 1}
            onClick={() => setZoom((value) => Math.max(1, value - 0.5))}
          >
            <Minus size={20} />
          </button>
          <button type="button" aria-label="経路全体を表示" onClick={reset}>
            <Crosshair size={20} />
          </button>
          {onShowAccidents && (
            <button
              type="button"
              aria-label="事故点を地図に表示"
              aria-pressed={showAccidents}
              onClick={() => onShowAccidents(!showAccidents)}
            >
              {showAccidents ? <Eye size={20} /> : <EyeOff size={20} />}
            </button>
          )}
        </div>
      )}
      {!disabled && (
        <div className="map-legend" aria-label="地図の凡例">
          <span>
            <i className="legend-considered" />
            事故履歴を考慮
          </span>
          <span>
            <i className="legend-fastest" />
            最短時間
          </span>
          <span>
            <i className="legend-section" />
            注意区間
          </span>
          {showAccidents && (
            <span>
              <i className="legend-accident" />
              事故記録
            </span>
          )}
        </div>
      )}
      {!disabled && selectedAccident && (
        <AccidentPopup
          accident={selectedAccident}
          onClose={() => setAccidentId(null)}
        />
      )}
    </div>
  );
}
