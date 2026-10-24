import Ajv2020 from "ajv/dist/2020";
import addFormats from "ajv-formats";
import contract from "./generated/contract.json";
import { ApiError } from "./errors";
import type {
  ConditionsResponse,
  GeocodeResponse,
  HealthResponse,
  RouteRequest,
  RouteResponse,
  ErrorResponse,
} from "../lib/types";

const ajv = new Ajv2020({ strict: false, allErrors: false });
addFormats(ajv);
ajv.addSchema(contract);
const validators = Object.fromEntries(
  [
    "RouteRequest",
    "RouteResponse",
    "ConditionsResponse",
    "GeocodeResponse",
    "HealthResponse",
    "ErrorResponse",
  ].map((name) => [
    name,
    ajv.compile({ $ref: `${contract.$id}#/$defs/${name}` }),
  ]),
);

function invalid(): never {
  throw new ApiError(
    "受信データの形式がAPI契約と一致しません。時間をおいて再試行してください。",
    "invalid_response",
  );
}

function schema<T>(name: string, value: unknown): T {
  if (!validators[name](value)) invalid();
  return value as T;
}

const close = (a: number, b: number, tolerance = 0.0011) =>
  Math.abs(a - b) < tolerance;
const ascending = (values: number[]) =>
  values.every((value, i) => !i || values[i - 1] <= value);

export function validateRouteRequest(value: unknown): RouteRequest {
  if (!validators.RouteRequest(value))
    throw new ApiError(
      "地点や検索条件を確認してください。",
      "invalid_request",
      400,
    );
  const request = value as RouteRequest;
  const { origin, dest } = request;
  const radians = Math.PI / 180;
  const distance =
    radians *
    6_371_008.8 *
    Math.hypot(
      origin.lat - dest.lat,
      (origin.lon - dest.lon) *
        Math.cos(((origin.lat + dest.lat) / 2) * radians),
    );
  if (distance < 20 || distance > 30_000)
    throw new ApiError(
      "出発地と目的地の直線距離は20m以上、30km以内にしてください。",
      "invalid_request",
      400,
    );
  return request;
}

export function validateRouteResponse(
  value: unknown,
  request?: RouteRequest,
): RouteResponse {
  const response = schema<RouteResponse>("RouteResponse", value);
  const { candidates } = response;
  if (
    !ascending(candidates.map((route) => route.duration)) ||
    !ascending(response.data_years)
  )
    invalid();
  if (new Set(candidates.map((route) => route.id)).size !== candidates.length)
    invalid();
  if (candidates.some((route, i) => route.id !== `route-${i + 1}`)) invalid();
  const fastest = candidates.find((route) => route.id === response.fastest_id);
  const safest = candidates.find((route) => route.id === response.safest_id);
  if (!fastest || (response.safest_id !== null && !safest)) invalid();
  // Route selection rules belong to the backend (still under verification).
  // Check only references and invariants that hold for any selection rule.
  if (safest && !safest.within_detour) invalid();
  const allEmpty = candidates.every((route) => route.accident_count === 0);
  if (allEmpty !== (response.safest_id === null)) invalid();
  if (
    !safest &&
    (response.extra_minutes !== null ||
      response.risk_reduction_percent !== null)
  )
    invalid();
  if (
    safest &&
    (response.extra_minutes === null ||
      !close(response.extra_minutes, safest.extra_minutes))
  )
    invalid();
  if (fastest.weighted_risk === 0 && response.risk_reduction_percent !== null)
    invalid();
  // The value is computed by the backend; only its null/non-null invariant is checked (behavior.md 8).
  if (
    safest &&
    fastest.weighted_risk > 0 &&
    response.risk_reduction_percent === null
  )
    invalid();
  let navigationBytes = 0;
  for (const route of candidates) {
    const selected = route.id === fastest.id || route.id === safest?.id;
    if (selected !== (route.navigation !== null)) invalid();
    if (route.navigation) {
      validateNavigation(route);
      navigationBytes += utf8Length(JSON.stringify(route.navigation));
    }
    if (route.fatal_accidents + route.injury_accidents !== route.accident_count)
      invalid();
    if (
      route.points_shown !== route.accident_points.features.length ||
      route.points_shown !== Math.min(route.accident_count, 250)
    )
      invalid();
    if (
      route.high_risk_sections.features.length !==
      Math.min(route.high_risk_total, 100)
    )
      invalid();
    if (!close(route.extra_minutes, (route.duration - fastest.duration) / 60))
      invalid();
    if (
      !close(
        route.risk_per_km,
        route.weighted_risk / Math.max(route.distance / 1000, 0.001),
      )
    )
      invalid();
    if (
      request &&
      route.within_detour !==
        route.duration <=
          fastest.duration + (request.max_detour_min ?? 10) * 60 + 1e-6
    )
      invalid();
    if (
      (response.effective_conditions.weather === "全天候") !==
      (route.weather_match === null)
    )
      invalid();
    if (route.weather_match) {
      const match = route.weather_match;
      if (
        match.fatal_accidents + match.injury_accidents !==
        match.accident_count
      )
        invalid();
      for (const key of [
        "accident_count",
        "fatal_accidents",
        "injury_accidents",
        "deaths",
        "injuries",
      ] as const)
        if (match[key] > route[key]) invalid();
    }
    if (
      route.exact_condition_count !==
      (route.weather_match?.accident_count ?? route.accident_count)
    )
      invalid();
    if (
      new Set(route.accident_points.features.map((point) => point.id)).size !==
      route.points_shown
    )
      invalid();
    if (
      new Set(route.high_risk_sections.features.map((section) => section.id))
        .size !== route.high_risk_sections.features.length
    )
      invalid();
    route.high_risk_sections.features.forEach((section, index, sections) => {
      const data = section.properties;
      if (
        data.route_id !== route.id ||
        section.id !== `${route.id}:segment-${data.segment_index}`
      )
        invalid();
      if (
        data.source_vertex_end_index !== data.source_vertex_start_index + 1 ||
        data.source_vertex_end_index >= route.geometry.coordinates.length
      )
        invalid();
      if (
        index &&
        (sections[index - 1].properties.severity_weight <
          data.severity_weight ||
          (sections[index - 1].properties.severity_weight ===
            data.severity_weight &&
            sections[index - 1].properties.segment_index > data.segment_index))
      )
        invalid();
    });
    if (
      !route.accident_count &&
      (route.weighted_risk ||
        route.high_risk_total ||
        route.deaths ||
        route.injuries)
    )
      invalid();
  }
  if (navigationBytes > MAX_NAVIGATION_BYTES) invalid();
  return response;
}

/** behavior.md 7: selected routes' navigation must be at most 5,000,000 bytes. */
const MAX_NAVIGATION_BYTES = 5_000_000;
const encoder = new TextEncoder();
const utf8Length = (text: string) => encoder.encode(text).length;

function validateNavigation(route: RouteResponse["candidates"][number]) {
  const navigation = route.navigation!;
  const distances = navigation.vertex_distances_m;
  const length = navigation.length_m;
  if (
    distances.length !== route.geometry.coordinates.length ||
    distances[0] !== 0 ||
    !ascending(distances) ||
    distances[distances.length - 1] !== length
  )
    invalid();
  let previousEnd = -2;
  let previousEndM = 0;
  let total = 0;
  for (const section of navigation.sections) {
    const start = section.start_segment_index;
    const end = section.end_segment_index;
    if (
      section.id !== `${route.id}:nav-${start}-${end}` ||
      end < start ||
      start <= previousEnd + 1 ||
      section.start_m < previousEndM ||
      section.start_m > section.end_m ||
      section.end_m > length
    )
      invalid();
    previousEnd = end;
    previousEndM = section.end_m;
    total += end - start + 1;
  }
  if (total !== route.high_risk_total) invalid();
}

export function validateConditions(value: unknown): ConditionsResponse {
  const result = schema<ConditionsResponse>("ConditionsResponse", value);
  if (
    (result.weather_status === "ok") !==
    (result.weather !== null && result.weather_source !== null)
  )
    invalid();
  if (
    result.weather_status !== "ok" &&
    (result.weather !== null || result.weather_source !== null)
  )
    invalid();
  if (!result.datetime.endsWith(":00+09:00")) invalid();
  return result;
}

export const validatePlaces = (value: unknown): GeocodeResponse =>
  schema<GeocodeResponse>("GeocodeResponse", value);
export const validateError = (value: unknown): ErrorResponse =>
  schema<ErrorResponse>("ErrorResponse", value);
export function validateHealth(value: unknown): HealthResponse {
  const response = schema<HealthResponse>("HealthResponse", value);
  if (!ascending(response.data_years)) invalid();
  return response;
}
