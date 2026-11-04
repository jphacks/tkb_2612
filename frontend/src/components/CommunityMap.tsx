import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { LocateFixed, Minus, Plus } from "lucide-react";
import type { Map as MapLibreMap } from "maplibre-gl";
import maplibreWorkerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?url";
import { isDemo } from "../api/client";
import type { LatLon } from "../lib/types";
import type { SafetyReport } from "../community/reports";
import {
  mapLocation,
  mapPoint,
  type MapSize,
  type MapView,
} from "../community/coordinates";
import { SAFETY_COLORS, SAFETY_LABELS } from "./icons/SafetyIcon";
import "./map.css";

type Props = {
  center: LatLon;
  currentPosition: LatLon | null;
  reports: SafetyReport[];
  selected: LatLon | null;
  onPick: (point: LatLon) => void;
  onReport: (report: SafetyReport) => void;
  onLocate: () => void;
  children?: ReactNode;
  popupWidth: number;
  popupHeight: number;
  bottomInset?: number;
};

export function CommunityMap(props: Props) {
  const shell = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLDivElement>(null);
  const popupElement = useRef<HTMLDivElement>(null);
  const map = useRef<MapLibreMap | null>(null);
  const current = useRef(props);
  current.current = props;
  const [size, setSize] = useState<MapSize>({ width: 960, height: 720 });
  const [view, setView] = useState<MapView>({ ...props.center, zoom: 14 });
  const [failed, setFailed] = useState(false);
  const [popupHeight, setPopupHeight] = useState(props.popupHeight);
  const styleUrl = !isDemo ? import.meta.env.VITE_MAP_STYLE_URL : undefined;
  const live = !!styleUrl && !failed;
  const drag = useRef<{ x: number; y: number; view: MapView } | null>(null);
  const dragged = useRef(false);

  useLayoutEffect(() => {
    const element = shell.current;
    if (!element) return;
    const measure = () => {
      const { width, height } = element.getBoundingClientRect();
      if (width && height) setSize({ width, height });
      map.current?.resize();
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (!live || !canvas.current) return;
    let disposed = false;
    let instance: MapLibreMap | undefined;
    void import("maplibre-gl")
      .then((maplibre) => {
        if (disposed || !canvas.current) return;
        maplibre.setWorkerUrl(maplibreWorkerUrl);
        instance = new maplibre.Map({
          container: canvas.current,
          style: styleUrl!,
          center: [current.current.center.lon, current.current.center.lat],
          zoom: 14,
        });
        map.current = instance;
        const update = () => {
          if (!instance || disposed) return;
          const center = instance.getCenter();
          setView({
            lat: center.lat,
            lon: center.lng,
            zoom: instance.getZoom(),
          });
        };
        instance.on("move", update);
        instance.on("load", update);
        instance.on("click", (event) =>
          current.current.onPick({
            lat: event.lngLat.lat,
            lon: event.lngLat.lng,
          }),
        );
        instance.on("error", () => {
          if (!disposed) setFailed(true);
        });
      })
      .catch(() => {
        if (!disposed) setFailed(true);
      });
    return () => {
      disposed = true;
      instance?.remove();
      map.current = null;
    };
  }, [live, styleUrl]);

  useEffect(() => {
    setView((previous) => ({ ...previous, ...props.center }));
    map.current?.jumpTo({ center: [props.center.lon, props.center.lat] });
  }, [props.center]);

  useLayoutEffect(() => {
    const element = popupElement.current;
    if (!element) return;
    const measure = () =>
      setPopupHeight(
        element.getBoundingClientRect().height || props.popupHeight,
      );
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [props.selected, props.popupHeight]);

  const zoom = (delta: number) => {
    const next = Math.min(19, Math.max(4, view.zoom + delta));
    if (map.current) map.current.jumpTo({ zoom: next });
    else setView((previous) => ({ ...previous, zoom: next }));
  };
  const project = (point: LatLon) =>
    map.current
      ? map.current.project([point.lon, point.lat])
      : mapPoint(point, view, size);
  const popup = props.selected ? project(props.selected) : null;
  const popupWidth = Math.min(props.popupWidth, size.width - 24);
  const popupX = popup
    ? Math.max(
        12,
        Math.min(size.width - popupWidth - 12, popup.x - popupWidth / 2),
      )
    : 0;
  const popupY = popup
    ? Math.max(
        80,
        Math.min(
          size.height - popupHeight - (props.bottomInset ?? 16),
          popup.y + 24,
        ),
      )
    : 0;

  return (
    <div className="route-map-shell community-map" ref={shell}>
      {live ? (
        <div ref={canvas} className="route-live-map" />
      ) : (
        <svg
          className="route-schematic community-canvas"
          viewBox={`0 0 ${size.width} ${size.height}`}
          role="application"
          aria-label="安全危険情報共有モードの地図"
          tabIndex={0}
          onPointerDown={(event) => {
            if (event.button !== 0) return;
            dragged.current = false;
            drag.current = { x: event.clientX, y: event.clientY, view };
            event.currentTarget.setPointerCapture?.(event.pointerId);
          }}
          onPointerMove={(event) => {
            if (!drag.current) return;
            const dx = event.clientX - drag.current.x;
            const dy = event.clientY - drag.current.y;
            if (Math.hypot(dx, dy) < 4 && !dragged.current) return;
            dragged.current = true;
            setView({
              ...mapLocation(
                size.width / 2 - dx,
                size.height / 2 - dy,
                drag.current.view,
                size,
              ),
              zoom: drag.current.view.zoom,
            });
          }}
          onPointerUp={() => {
            drag.current = null;
          }}
          onPointerCancel={() => {
            drag.current = null;
          }}
          onClick={(event) => {
            if (dragged.current) return;
            const rect = event.currentTarget.getBoundingClientRect();
            props.onPick(
              mapLocation(
                event.clientX - rect.left,
                event.clientY - rect.top,
                view,
                size,
              ),
            );
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              props.onPick({ lat: view.lat, lon: view.lon });
            }
            const offset = {
              ArrowLeft: [-80, 0],
              ArrowRight: [80, 0],
              ArrowUp: [0, -80],
              ArrowDown: [0, 80],
            }[event.key];
            if (offset) {
              event.preventDefault();
              setView({
                ...mapLocation(
                  size.width / 2 + offset[0],
                  size.height / 2 + offset[1],
                  view,
                  size,
                ),
                zoom: view.zoom,
              });
            }
          }}
        >
          <defs>
            <pattern
              id="community-grid"
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
            width={size.width}
            height={size.height}
            fill="url(#community-grid)"
          />
        </svg>
      )}
      {props.currentPosition &&
        (() => {
          const point = project(props.currentPosition);
          return (
            <span
              className="community-current"
              style={{ left: point.x, top: point.y }}
              role="img"
              aria-label="現在地"
            />
          );
        })()}
      {props.reports.map((report) => {
        const point = project(report);
        if (
          point.x < -22 ||
          point.x > size.width + 22 ||
          point.y < -22 ||
          point.y > size.height + 22
        )
          return null;
        return (
          <button
            key={report.id}
            type="button"
            className="community-marker"
            style={{
              left: point.x,
              top: point.y,
              color: SAFETY_COLORS[report.kind],
            }}
            aria-label={`${SAFETY_LABELS[report.kind]}の口コミ${report.text ? `：${report.text}` : ""}`}
            onClick={() => props.onReport(report)}
          >
            <span />
            {report.supportCount > 1 && (
              <span className="community-marker-count" aria-hidden="true">
                {report.supportCount}
              </span>
            )}
          </button>
        );
      })}
      {popup && (
        <span
          className="community-picked"
          style={{ left: popup.x, top: popup.y }}
          aria-hidden="true"
        />
      )}
      {popup && (
        <div
          ref={popupElement}
          className="community-popup-anchor"
          style={{ left: popupX, top: popupY, width: popupWidth }}
        >
          {props.children}
        </div>
      )}
      <div className="map-controls" aria-label="地図の表示操作">
        <button
          type="button"
          aria-label="地図を拡大"
          onClick={() => zoom(1)}
          disabled={view.zoom >= 19}
        >
          <Plus size={20} />
        </button>
        <button
          type="button"
          aria-label="地図を縮小"
          onClick={() => zoom(-1)}
          disabled={view.zoom <= 4}
        >
          <Minus size={20} />
        </button>
        <button
          type="button"
          aria-label="現在地を表示"
          onClick={props.onLocate}
        >
          <LocateFixed size={20} />
        </button>
      </div>
    </div>
  );
}
