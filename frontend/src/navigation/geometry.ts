import type { NavigationData } from "../lib/types";

export interface Coordinate { lon: number; lat: number; }
export interface PlanePoint { x: number; y: number; }
export interface RouteGeometry {
  origin: Coordinate;
  kx: number;
  ky: number;
  points: PlanePoint[];
  distances: readonly number[];
  computedDistances: number[];
  lengthM: number;
}
export interface RouteProjection {
  p: number;
  distanceM: number;
  point: PlanePoint;
  segmentIndex: number;
}
const K = 6371008.8 * Math.PI / 180;
export const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

export function createRouteGeometry(coordinates: readonly (readonly number[])[], navigation: NavigationData): RouteGeometry {
  const origin = { lon: coordinates[0][0], lat: coordinates[0][1] };
  const kx = K * Math.cos(navigation.projection_lat_deg * Math.PI / 180);
  const points = coordinates.map(([lon, lat]) => ({ x: (lon - origin.lon) * kx, y: (lat - origin.lat) * K }));
  const computedDistances = [0];
  for (let i = 1; i < coordinates.length; i++) {
    // 元の隣接座標差で計算し、サーバと同じf64の距離軸を検算する。
    computedDistances.push(computedDistances[i - 1] + Math.hypot((coordinates[i][0] - coordinates[i - 1][0]) * kx, (coordinates[i][1] - coordinates[i - 1][1]) * K));
  }
  return { origin, kx, ky: K, points, distances: navigation.vertex_distances_m, computedDistances, lengthM: navigation.length_m };
}
export function projectPoint(g: RouteGeometry, p: Coordinate): PlanePoint {
  return { x: (p.lon - g.origin.lon) * g.kx, y: (p.lat - g.origin.lat) * g.ky };
}
export function unprojectPoint(g: RouteGeometry, p: PlanePoint): Coordinate {
  return { lon: g.origin.lon + p.x / g.kx, lat: g.origin.lat + p.y / g.ky };
}
export function projectOnSegment(g: RouteGeometry, p: PlanePoint, i: number): RouteProjection {
  const a = g.points[i], b = g.points[i + 1];
  const dx = b.x - a.x, dy = b.y - a.y;
  const length2 = dx * dx + dy * dy;
  const t = length2 ? clamp(((p.x - a.x) * dx + (p.y - a.y) * dy) / length2, 0, 1) : 0;
  const point = { x: a.x + t * dx, y: a.y + t * dy };
  return { p: g.distances[i] + t * (g.distances[i + 1] - g.distances[i]), distanceM: Math.hypot(p.x - point.x, p.y - point.y), point, segmentIndex: i };
}
export function routeProjections(g: RouteGeometry, p: Coordinate): RouteProjection[] {
  const plane = projectPoint(g, p);
  return g.points.slice(1).map((_, i) => projectOnSegment(g, plane, i));
}
export function nearestRoutePoint(g: RouteGeometry, p: Coordinate): RouteProjection {
  return routeProjections(g, p).reduce((a, b) => b.distanceM < a.distanceM ? b : a);
}
export function pointAtProgress(g: RouteGeometry, p: number): RouteProjection {
  p = clamp(p, 0, g.lengthM);
  let i = 0;
  while (i < g.points.length - 2 && (g.distances[i + 1] <= p || g.distances[i + 1] === g.distances[i])) i++;
  // 終点でも直前の正長辺を使う。
  while (i > 0 && g.distances[i + 1] === g.distances[i]) i--;
  const a = g.points[i], b = g.points[i + 1];
  const length = g.distances[i + 1] - g.distances[i];
  const t = length ? (p - g.distances[i]) / length : 0;
  return { p, segmentIndex: i, distanceM: 0, point: { x: a.x + t * (b.x - a.x), y: a.y + t * (b.y - a.y) } };
}
