import type { RouteCandidate } from "../lib/types";
import { createRouteGeometry } from "./geometry";
export type NavigationSelection = "fastest" | "safest";
export interface NavigationStartInput {
  route: RouteCandidate;
  selection: NavigationSelection;
  fastestId: string;
  safestId: string | null;
  responseGeneration: number;
  currentGeneration: number;
  mapAvailable: boolean;
}
export type NavigationStartResult = { ok: true; route: RouteCandidate; selection: NavigationSelection; } | { ok: false; reason: string; };
export function checkNavigationStart(input: NavigationStartInput): NavigationStartResult {
  const fail = (reason: string): NavigationStartResult => ({ ok: false, reason });
  const { route, selection } = input;
  if (input.responseGeneration !== input.currentGeneration) return fail("検索結果が古くなっています");
  if (route.id !== (selection === "fastest" ? input.fastestId : input.safestId)) return fail("選定された経路から開始してください");
  if (!input.mapAvailable) return fail("地図を利用できません");
  const n = route.navigation;
  if (!n) return fail("ナビ用データがありません");
  const c = route.geometry.coordinates, d = n.vertex_distances_m;
  if (c.length < 2 || c.length > 15000 || c.some(p => p.length !== 2 || !p.every(Number.isFinite) || p[0] < 122 || p[0] > 154 || p[1] < 20 || p[1] > 46) || !Number.isFinite(n.projection_lat_deg) || n.projection_lat_deg < 20 || n.projection_lat_deg > 46 || !Number.isFinite(n.length_m) || n.length_m < 0 || n.length_m > 110000 || d.length !== c.length || d[0] !== 0 || d.at(-1) !== n.length_m || d.some((v, i) => !Number.isFinite(v) || v < 0 || (i > 0 && v < d[i - 1]))) return fail("累積距離配列が不正です");
  const meanLat = c.reduce((sum, p) => sum + p[1], 0) / c.length;
  const g = createRouteGeometry(c, n);
  if (Math.abs(meanLat - n.projection_lat_deg) > 1e-9 || g.computedDistances.some((v, i) => Math.abs(v - d[i]) > 1e-6)) return fail("局所平面の距離軸が一致しません");
  // 小線分の参照だけを検算し、リスクの再計算や再結合はしない。
  const starts: number[] = [], ends: number[] = [];
  for (let i = 1; i < d.length; i++) {
    const edgeLength = Math.hypot((c[i][0] - c[i - 1][0]) * g.kx, (c[i][1] - c[i - 1][1]) * g.ky);
    const count = Math.max(1, Math.ceil(edgeLength / 75));
    for (let j = 0; j < count; j++) { starts.push(d[i - 1] + (d[i] - d[i - 1]) * j / count); ends.push(d[i - 1] + (d[i] - d[i - 1]) * (j + 1) / count); }
  }
  let last = -2, total = 0;
  if (n.sections.length > 8233) return fail("ナビ区間の数が不正です");
  for (const s of n.sections) {
    const a = s.start_segment_index, b = s.end_segment_index;
    if (!Number.isInteger(a) || !Number.isInteger(b) || a <= last + 1 || a < 0 || b < a || b >= ends.length || s.id !== `${route.id}:nav-${a}-${b}` || !Number.isFinite(s.start_m) || !Number.isFinite(s.end_m) || Math.abs(s.start_m - starts[a]) > 1e-6 || Math.abs(s.end_m - ends[b]) > 1e-6 || !Number.isSafeInteger(s.severity_weight) || s.severity_weight < 3) return fail("ナビ区間の参照が不正です");
    last = b; total += b - a + 1;
  }
  if (total !== route.high_risk_total) return fail("ナビ区間の件数が一致しません");
  if (n.length_m === 0) return fail("長さ0の経路では開始できません");
  return { ok: true, route, selection };
}
