import type { LatLon } from "../lib/types";

export type MapView = LatLon & { zoom: number };
export type MapSize = { width: number; height: number };
export const INITIAL_CENTER: LatLon = { lat: 35.6812, lon: 139.7671 };

function world(point: LatLon, zoom: number) {
  const scale = 256 * 2 ** zoom;
  const sin = Math.sin(
    (Math.max(-85, Math.min(85, point.lat)) * Math.PI) / 180,
  );
  return {
    x: ((point.lon + 180) / 360) * scale,
    y: (0.5 - Math.log((1 + sin) / (1 - sin)) / (4 * Math.PI)) * scale,
  };
}

export function mapPoint(point: LatLon, view: MapView, size: MapSize) {
  const p = world(point, view.zoom);
  const c = world(view, view.zoom);
  return { x: p.x - c.x + size.width / 2, y: p.y - c.y + size.height / 2 };
}

export function mapLocation(
  x: number,
  y: number,
  view: MapView,
  size: MapSize,
): LatLon {
  const c = world(view, view.zoom);
  const scale = 256 * 2 ** view.zoom;
  const longitude = ((c.x + x - size.width / 2) / scale) * 360 - 180;
  const n = Math.PI - (2 * Math.PI * (c.y + y - size.height / 2)) / scale;
  return {
    lat: Math.max(-85, Math.min(85, (Math.atan(Math.sinh(n)) * 180) / Math.PI)),
    lon: ((((longitude + 180) % 360) + 360) % 360) - 180,
  };
}
