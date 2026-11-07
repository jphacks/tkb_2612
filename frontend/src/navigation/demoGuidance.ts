import type { RouteCandidate } from "../lib/types";
import type { GuidanceStep } from "./guidance";

/**
 * デモの模式図の折れ曲がりから作る表示確認用の案内。
 * 実道路の交差点情報ではないため、デモの模擬走行にだけ使う。
 */
export function createDemoGuidance(route: RouteCandidate): GuidanceStep[] {
  const navigation = route.navigation;
  if (!navigation) return [];
  const points = route.geometry.coordinates;
  const cos = Math.cos(navigation.projection_lat_deg * Math.PI / 180);
  const steps: GuidanceStep[] = [];
  for (let i = 1; i < points.length - 1; i++) {
    const incoming = [(points[i][0] - points[i - 1][0]) * cos, points[i][1] - points[i - 1][1]];
    const outgoing = [(points[i + 1][0] - points[i][0]) * cos, points[i + 1][1] - points[i][1]];
    if (!Math.hypot(...incoming) || !Math.hypot(...outgoing)) continue;
    const angle = Math.atan2(
      incoming[0] * outgoing[1] - incoming[1] * outgoing[0],
      incoming[0] * outgoing[0] + incoming[1] * outgoing[1],
    ) * 180 / Math.PI;
    if (Math.abs(angle) < 10) continue;
    steps.push({ id: `${route.id}:demo-turn-${i}`, maneuver: angle > 0 ? "left" : "right", atM: navigation.vertex_distances_m[i] });
  }
  steps.push({ id: `${route.id}:demo-arrival`, maneuver: "arrive", atM: navigation.length_m });
  return steps;
}
