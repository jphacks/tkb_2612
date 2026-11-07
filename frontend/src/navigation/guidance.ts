/** フロントの案内表示モデル。将来のAPIの案内地点も、この距離軸で受け取る。 */
export type Maneuver = "left" | "right" | "straight" | "arrive";

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

/** 確定した進捗から次の案内を選ぶ。通過した案内は表示しない。 */
export function nextGuidance(
  steps: readonly GuidanceStep[],
  progressM: number | null,
): NavigationGuidance | null {
  if (progressM === null || !Number.isFinite(progressM) || progressM < 0) return null;
  const step = steps.find((candidate) => candidate.atM > progressM)
    ?? (steps.at(-1)?.maneuver === "arrive" ? steps.at(-1) : undefined);
  return step ? { id: step.id, maneuver: step.maneuver, distanceM: Math.max(0, step.atM - progressM) } : null;
}
