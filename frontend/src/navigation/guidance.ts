import type { components } from "../api/generated/schema";
import type { RouteCandidate } from "../lib/types";

/** フロントの案内表示モデル。APIの案内地点をこの距離軸で受け取る。 */
export type Maneuver = components["schemas"]["GuidanceStep"]["maneuver"];

export interface GuidanceStep {
  id: string;
  maneuver: Maneuver;
  /** 経路始点から案内地点までの距離。navigation.vertex_distances_mと同じ軸。 */
  atM: number;
}

export interface NavigationGuidance {
  id: string;
  maneuver: Maneuver;
  distanceM: number;
}

/** 通過済み案内のID。ナビのセッションごとに作り直す。 */
export type GuidanceHistory = ReadonlySet<string>;

export const createGuidanceHistory = (): GuidanceHistory => new Set();

/** 経路応答の案内地点を、通過履歴で使うID付きの表示モデルへ変換する。 */
export function guidanceStepsFromRoute(route: RouteCandidate): GuidanceStep[] {
  if (!route.navigation) return [];
  return route.navigation.guidance.map((step, index) => ({
    id: `${route.id}:guide-${index}`,
    maneuver: step.maneuver,
    atM: step.at_m,
  }));
}

const EMPTY_GUIDANCE_HISTORY = createGuidanceHistory();

/**
 * 通過済み案内を再表示しない後退許容幅。
 * Simulatorの揺らぎは半径8mで、前後サンプルの経路方向の差は最大16m（実測例は6.6m）。
 * Matcherのaccuracyも10mあるため、20mなら揺らぎを吸収しつつ、
 * それを超える引き返しは本物の逆行として案内を戻せる。
 */
export const GUIDANCE_BACKTRACK_M = 20;

/** 確定した進捗から次の案内を選ぶ。historyにある案内は、大きく引き返すまで通過扱いを保つ。 */
export function nextGuidance(
  steps: readonly GuidanceStep[],
  progressM: number | null,
  history: GuidanceHistory = EMPTY_GUIDANCE_HISTORY,
): NavigationGuidance | null {
  if (progressM === null || !Number.isFinite(progressM) || progressM < 0) return null;
  const step = steps.find(
    (candidate) =>
      candidate.atM > progressM &&
      (!history.has(candidate.id) || progressM < candidate.atM - GUIDANCE_BACKTRACK_M),
  ) ?? (steps.at(-1)?.maneuver === "arrive" ? steps.at(-1) : undefined);
  return step ? { id: step.id, maneuver: step.maneuver, distanceM: Math.max(0, step.atM - progressM) } : null;
}

/** 確定進捗で通過状態を更新する。大きく引き返した案内と、その後ろの案内を未通過へ戻す。 */
export function updateGuidanceHistory(
  steps: readonly GuidanceStep[],
  progressM: number | null,
  history: GuidanceHistory,
): GuidanceHistory {
  if (progressM === null || !Number.isFinite(progressM) || progressM < 0) return history;
  let next: Set<string> | null = null;
  for (let index = 0; index < steps.length; index++) {
    const step = steps[index];
    if (progressM >= step.atM) {
      if (!history.has(step.id)) {
        next ??= new Set(history);
        next.add(step.id);
      }
    } else if (progressM < step.atM - GUIDANCE_BACKTRACK_M) {
      // この案内より後ろも、引き返した地点より先にあるため未通過へ戻す。
      for (let later = index; later < steps.length; later++) {
        if ((next ?? history).has(steps[later].id)) {
          next ??= new Set(history);
          next.delete(steps[later].id);
        }
      }
      break;
    }
  }
  return next ?? history;
}
