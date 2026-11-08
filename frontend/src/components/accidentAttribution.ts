import type { AccidentFeature } from "../lib/types";

export function attributionLabel(
  attribution: AccidentFeature["properties"]["attribution"],
  radiusM: number,
): string {
  if (attribution === "edge") return "道路の区間";
  if (attribution === "intersection") return "交差点";
  return `道路を特定できず（経路から${radiusM}m以内）`;
}
