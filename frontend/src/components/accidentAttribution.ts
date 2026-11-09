import type { AccidentFeature } from "../lib/types";

export function attributionLabel(
  attribution: AccidentFeature["properties"]["attribution"],
  radiusM: number,
): string {
  if (attribution === "edge") return "経路上の道路";
  if (attribution === "intersection") return "経路上の交差点";
  return `経路の近く（${radiusM}m以内）`;
}
