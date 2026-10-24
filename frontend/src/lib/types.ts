import type { components } from "../api/generated/schema";

type Schema = components["schemas"];
export type Place = Schema["GeocodeResult"];
export type LatLon = Schema["LatLon"];
export type Mode = Schema["Mode"];
export type Period = Schema["Period"];
export type Weather = Schema["Weather"];
export type DayType = Schema["DayType"];
export type RouteRequest = Schema["RouteRequest"];
export type RouteResponse = Schema["RouteResponse"];
export type RouteCandidate = Schema["RouteCandidate"];
export type ConditionsResponse = Schema["ConditionsResponse"];
export type HealthResponse = Schema["HealthResponse"];
export type GeocodeResponse = Schema["GeocodeResponse"];
export type AccidentFeature = Schema["AccidentFeature"];
export type HighRiskFeature = Schema["HighRiskFeature"];
export type NavigationData = Schema["NavigationData"];
export type ErrorResponse = Schema["ErrorResponse"];

export type Scenario =
  | "normal"
  | "empty"
  | "loading"
  | "error"
  | "same"
  | "zero"
  | "map-error"
  | "conditions-error";

export interface ConditionValues {
  mode: Mode;
  period: Period | "all";
  day_type: DayType | "all";
  weather: Weather | "all";
  max_detour_min: number;
}
