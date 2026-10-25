import { describe, expect, it } from "vitest";
import { localPosition, testRoute } from "../navigation/test-utils";
import type { NavigationSnapshot } from "../navigation/session";
import { accuracyCircle, buildNavigationLayers, sectionFeature } from "./navigationData";

function snapshot(patch: Partial<NavigationSnapshot>): NavigationSnapshot {
  return { kind: "simulation", location: null, lastFix: null, alertIds: [], ...patch } as NavigationSnapshot;
}

describe("ナビ用レイヤーのデータ", () => {
  it("停止後（snapshotなし）は全レイヤーが空になる", () => {
    const layers = buildNavigationLayers(null, null);
    for (const collection of Object.values(layers)) expect(collection.features).toEqual([]);
  });

  it("選んだ1本の経路、現在地、未補正の測位を中心とした精度円、通知中区間だけを作る", () => {
    const route = testRoute([0, 100, 400]);
    route.navigation!.sections = [
      { id: "route-1:nav-0-0", start_segment_index: 0, end_segment_index: 0, start_m: 0, end_m: 75, severity_weight: 3 },
    ];
    const raw = { ...localPosition(10, 5), accuracy: 12, timestamp: 1000 };
    const layers = buildNavigationLayers(route, snapshot({
      location: { ...localPosition(10), onRoute: true },
      lastFix: raw,
      alertIds: ["route-1:nav-0-0"],
    }));
    expect(layers.route.features).toHaveLength(1);
    expect(layers.route.features[0].geometry).toEqual(route.geometry);
    expect(layers.location.features[0].properties).toMatchObject({ onRoute: true, kind: "simulation" });
    expect(layers.location.features[0].geometry.coordinates).toEqual([localPosition(10).lon, localPosition(10).lat]);
    const ring = layers.accuracy.features[0].geometry.coordinates[0];
    expect(ring[0]).toEqual(ring.at(-1));
    const centre = ring.slice(0, -1).reduce((sum, [x, y]) => [sum[0] + x / (ring.length - 1), sum[1] + y / (ring.length - 1)], [0, 0]);
    expect(centre[0]).toBeCloseTo(raw.lon, 7);
    expect(centre[1]).toBeCloseTo(raw.lat, 7);
    expect(layers.alert.features).toHaveLength(1);
    const line = layers.alert.features[0].geometry;
    expect(line.type).toBe("LineString");
  });

  it("通知中でない区間は描かず、0長区間は点で代替し、半径はaccuracyに一致する", () => {
    const route = testRoute([0, 200]);
    route.navigation!.sections = [
      { id: "a", start_segment_index: 0, end_segment_index: 0, start_m: 50, end_m: 50, severity_weight: 3 },
    ];
    expect(sectionFeature(route, route.navigation!.sections[0]).geometry.type).toBe("Point");
    expect(buildNavigationLayers(route, snapshot({ alertIds: [] })).alert.features).toEqual([]);
    const circle = accuracyCircle(139, 35, 30).geometry.coordinates[0][0];
    const metres = (circle[0] - 139) * (Math.PI * 6371008.8 / 180) * Math.cos(35 * Math.PI / 180);
    expect(metres).toBeCloseTo(30, 3);
  });
});
