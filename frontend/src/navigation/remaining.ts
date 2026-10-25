import { clamp } from "./geometry";
export interface Remaining { distanceM: number; durationS: number; displayDistanceM: number; displayMinutes: number; }
export function calculateRemaining(route: { distance: number; duration: number; navigation: { length_m: number; }; }, p: number | null): Remaining | null {
  if (p === null || !Number.isFinite(p) || route.navigation.length_m <= 0) return null;
  const r = 1 - clamp(p, 0, route.navigation.length_m) / route.navigation.length_m;
  const distanceM = route.distance * r, durationS = route.duration * r;
  return { distanceM, durationS, displayDistanceM: Math.ceil(distanceM / 10) * 10, displayMinutes: Math.ceil(durationS / 60) };
}
