import type { Feature, LineString, Point, Polygon } from "geojson";
import type { RouteCandidate } from "../lib/types";
import { createRouteGeometry, pointAtProgress, unprojectPoint, type RouteGeometry } from "../navigation/geometry";
import type { NavigationSnapshot } from "../navigation/session";
import { emptyNavigationLayers, type NavigationLayers } from "./layers";

const geometries = new WeakMap<RouteCandidate, RouteGeometry>();
function geometryOf(route: RouteCandidate): RouteGeometry {
  let geometry = geometries.get(route);
  if (!geometry) {
    geometry = createRouteGeometry(route.geometry.coordinates, route.navigation!);
    geometries.set(route, geometry);
  }
  return geometry;
}

const METRES_PER_DEGREE = (6371008.8 * Math.PI) / 180;

/** 未補正の測位を中心にしたaccuracy半径の円（局所的な正多角形）。 */
export function accuracyCircle(lon: number, lat: number, radiusM: number): Feature<Polygon> {
  const steps = 48;
  const dLat = radiusM / METRES_PER_DEGREE;
  const dLon = radiusM / (METRES_PER_DEGREE * Math.cos((lat * Math.PI) / 180));
  const ring = Array.from({ length: steps }, (_, index) => {
    const angle = (2 * Math.PI * index) / steps;
    return [lon + dLon * Math.cos(angle), lat + dLat * Math.sin(angle)];
  });
  return {
    type: "Feature",
    geometry: { type: "Polygon", coordinates: [[...ring, ring[0]]] },
    properties: {},
  };
}

/** ナビ用sectionsのstart_m〜end_mに対応する経路部分。0長は点で代替する。 */
export function sectionFeature(
  route: RouteCandidate,
  section: { id: string; start_m: number; end_m: number },
): Feature<LineString | Point> {
  const geometry = geometryOf(route);
  const at = (p: number) => {
    const point = unprojectPoint(geometry, pointAtProgress(geometry, p).point);
    return [point.lon, point.lat];
  };
  if (section.end_m <= section.start_m) {
    return {
      type: "Feature",
      id: section.id,
      geometry: { type: "Point", coordinates: at(section.start_m) },
      properties: { id: section.id },
    };
  }
  const inner = route.geometry.coordinates.filter(
    (_, index) =>
      geometry.distances[index] > section.start_m && geometry.distances[index] < section.end_m,
  );
  return {
    type: "Feature",
    id: section.id,
    geometry: {
      type: "LineString",
      coordinates: [at(section.start_m), ...inner, at(section.end_m)],
    },
    properties: { id: section.id },
  };
}

/** ナビの状態から地図のナビ用4レイヤーのデータを作る。nullなら全消去。 */
export function buildNavigationLayers(
  route: RouteCandidate | null,
  snapshot: NavigationSnapshot | null,
): NavigationLayers {
  const layers = emptyNavigationLayers();
  if (!route || !snapshot) return layers;
  layers.route.features.push({
    type: "Feature",
    geometry: route.geometry,
    properties: { kind: snapshot.kind },
  });
  if (snapshot.location) {
    layers.location.features.push({
      type: "Feature",
      geometry: { type: "Point", coordinates: [snapshot.location.lon, snapshot.location.lat] },
      properties: { onRoute: snapshot.location.onRoute, kind: snapshot.kind },
    });
  }
  if (snapshot.lastFix) {
    layers.accuracy.features.push(
      accuracyCircle(snapshot.lastFix.lon, snapshot.lastFix.lat, snapshot.lastFix.accuracy),
    );
  }
  const active = new Set(snapshot.alertIds);
  for (const section of route.navigation?.sections ?? []) {
    if (active.has(section.id)) layers.alert.features.push(sectionFeature(route, section));
  }
  return layers;
}
