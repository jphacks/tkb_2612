import type { AccidentFeature } from "../lib/types";

export function attributionLabel(
  attribution: AccidentFeature["properties"]["attribution"],
): string {
  if (attribution === "edge") return "道路";
  if (attribution === "intersection") return "交差点";
  return "経路の近く";
}
