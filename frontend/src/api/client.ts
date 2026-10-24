import type {
  ConditionsResponse,
  GeocodeResponse,
  HealthResponse,
  LatLon,
  RouteRequest,
  RouteResponse,
  Scenario,
} from "../lib/types";
import { ApiError } from "./errors";
import {
  createDemoConditions,
  createDemoRouteResponse,
  demoHealth,
  findDemoPlaces,
} from "./demo";
import {
  validateConditions,
  validateError,
  validateHealth,
  validatePlaces,
  validateRouteRequest,
  validateRouteResponse,
} from "./validate";

export { ApiError } from "./errors";
export const isDemo = import.meta.env.VITE_API_MODE !== "live";

const abortError = () =>
  new DOMException("操作がキャンセルされました。", "AbortError");

/** The loading scenario deliberately stays pending until the caller cancels. */
export function demoDelay(
  milliseconds: number,
  signal?: AbortSignal,
  pending = false,
): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(abortError());
      return;
    }
    const cleanup = () => {
      if (timer !== undefined) clearTimeout(timer);
      signal?.removeEventListener("abort", abort);
    };
    const abort = () => {
      cleanup();
      reject(abortError());
    };
    const timer = pending
      ? undefined
      : setTimeout(() => {
          cleanup();
          resolve();
        }, milliseconds);
    signal?.addEventListener("abort", abort, { once: true });
  });
}

async function demoStep(
  signal: AbortSignal | undefined,
  scenario: Scenario,
  milliseconds = 350,
): Promise<void> {
  await demoDelay(milliseconds, signal, scenario === "loading");
  if (scenario === "error")
    throw new ApiError(
      "デモ：検索に失敗しました。通常状態に切り替えて再試行してください。",
      "external_api_error",
      502,
    );
}

/** Client deadlines (architecture.md 4): route covers body receipt, parsing and checks. */
const ROUTE_TIMEOUT_MS = 180_000;
const DEFAULT_TIMEOUT_MS = 45_000;
const CONDITIONS_TIMEOUT_MS = 10_000;

async function requestJson<T>(
  path: string,
  parse: (data: unknown) => T,
  signal?: AbortSignal,
  body?: unknown,
  timeout = DEFAULT_TIMEOUT_MS,
): Promise<T> {
  if (signal?.aborted) throw abortError();
  const controller = new AbortController();
  let timedOut = false;
  const deadline = performance.now() + timeout;
  const abort = () => controller.abort();
  signal?.addEventListener("abort", abort, { once: true });
  const timer = setTimeout(() => {
    timedOut = true;
    controller.abort();
  }, timeout);
  try {
    const response = await fetch(path, {
      method: body === undefined ? "GET" : "POST",
      headers:
        body === undefined
          ? { Accept: "application/json" }
          : { Accept: "application/json", "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: controller.signal,
      cache: "no-store",
    });
    let data: unknown;
    try {
      data = await response.json();
    } catch {
      if (controller.signal.aborted) throw abortError();
      throw new ApiError(
        "サーバーから正しいJSONを受信できませんでした。",
        "invalid_response",
        response.status,
      );
    }
    if (!response.ok) {
      const failure = validateError(data);
      throw new ApiError(
        failure.error.message,
        failure.error.code,
        response.status,
      );
    }
    const parsed = parse(data);
    // Contract checks run synchronously, so the timer cannot fire during them.
    // A result completed after the deadline is still a timeout.
    if (performance.now() > deadline) {
      timedOut = true;
      throw abortError();
    }
    return parsed;
  } catch (error) {
    if (signal?.aborted) throw abortError();
    // The timer cannot fire during synchronous parsing and checks, so the
    // elapsed time is checked here too when those steps throw past the deadline.
    if (timedOut || performance.now() > deadline)
      throw new ApiError(
        "通信がタイムアウトしました。時間をおいて再試行してください。",
        "timeout",
      );
    if (error instanceof ApiError) throw error;
    throw new ApiError(
      "サーバーに接続できません。接続を確認して再試行してください。",
      "network_error",
    );
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener("abort", abort);
  }
}

export async function getHealth(signal?: AbortSignal): Promise<HealthResponse> {
  if (isDemo) {
    await demoDelay(200, signal);
    return validateHealth(structuredClone(demoHealth));
  }
  return requestJson("/api/health", validateHealth, signal);
}

export async function searchPlaces(
  q: string,
  signal?: AbortSignal,
  scenario: Scenario = "normal",
): Promise<GeocodeResponse> {
  const query = q.trim();
  if (Array.from(query).length < 2 || Array.from(query).length > 200)
    throw new ApiError(
      "住所や地点名は2〜200文字で入力してください。",
      "invalid_request",
      400,
    );
  if (isDemo) {
    await demoStep(signal, scenario, 250);
    const result = findDemoPlaces(query);
    if (scenario === "empty") result.results = [];
    return validatePlaces(result);
  }
  return requestJson(
    `/api/geocode?${new URLSearchParams({ q: query })}`,
    validatePlaces,
    signal,
  );
}

export async function getConditions(
  datetime: string,
  origin?: LatLon,
  dest?: LatLon,
  signal?: AbortSignal,
  scenario: Scenario = "normal",
): Promise<ConditionsResponse> {
  const location =
    origin && dest
      ? {
          lat: Number(((origin.lat + dest.lat) / 2).toFixed(2)),
          lon: Number(((origin.lon + dest.lon) / 2).toFixed(2)),
        }
      : undefined;
  if (isDemo) {
    await demoStep(signal, scenario, 300);
    if (scenario === "conditions-error")
      throw new ApiError(
        "デモ：条件を取得できませんでした。手動指定または再試行ができます。",
        "internal_error",
        500,
      );
    return validateConditions(createDemoConditions(datetime, location));
  }
  const params = new URLSearchParams({ datetime });
  if (location) {
    params.set("lat", location.lat.toFixed(2));
    params.set("lon", location.lon.toFixed(2));
  }
  return requestJson(
    `/api/conditions?${params}`,
    validateConditions,
    signal,
    undefined,
    CONDITIONS_TIMEOUT_MS,
  );
}

export async function searchRoutes(
  request: RouteRequest,
  signal?: AbortSignal,
  scenario: Scenario = "normal",
): Promise<RouteResponse> {
  validateRouteRequest(request);
  if (isDemo) {
    await demoStep(signal, scenario, 500);
    return validateRouteResponse(
      createDemoRouteResponse(
        request,
        scenario === "same" || scenario === "zero" ? scenario : "normal",
      ),
      request,
    );
  }
  return requestJson(
    "/api/route",
    (data) => validateRouteResponse(data, request),
    signal,
    request,
    ROUTE_TIMEOUT_MS,
  );
}
