import { afterEach, describe, expect, it, vi } from "vitest";
import { createDemoConditions } from "./demo";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
  vi.resetModules();
});

describe("API client cancellation and live separation", () => {
  it("allows an indefinite loading demonstration to be canceled without networking", async () => {
    vi.stubEnv("VITE_API_MODE", "demo");
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    const { searchPlaces } = await import("./client");
    const controller = new AbortController();
    const result = searchPlaces("東京", controller.signal, "loading");
    controller.abort();
    await expect(result).rejects.toMatchObject({ name: "AbortError" });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("rejects a pre-canceled request immediately", async () => {
    const { demoDelay } = await import("./client");
    const controller = new AbortController();
    controller.abort();
    await expect(demoDelay(300, controller.signal)).rejects.toMatchObject({
      name: "AbortError",
    });
  });

  it("sends only the two-decimal midpoint and the supplied offset time for live conditions", async () => {
    vi.stubEnv("VITE_API_MODE", "live");
    const datetime = "2026-10-24T12:00+09:00";
    const response = createDemoConditions(datetime);
    const fetchMock = vi
      .fn()
      .mockResolvedValue(
        new Response(JSON.stringify(response), { status: 200 }),
      );
    vi.stubGlobal("fetch", fetchMock);
    const { getConditions, isDemo } = await import("./client");
    expect(isDemo).toBe(false);
    await getConditions(
      datetime,
      { lat: 35.6812, lon: 139.7671 },
      { lat: 35.7138, lon: 139.7773 },
    );
    const url = new URL(fetchMock.mock.calls[0][0], "http://localhost");
    expect(url.pathname).toBe("/api/conditions");
    expect(url.searchParams.get("lat")).toBe("35.70");
    expect(url.searchParams.get("lon")).toBe("139.77");
    expect(url.searchParams.get("datetime")).toBe(datetime);
    expect(url.searchParams.has("origin")).toBe(false);
  });

  it("does not replace a live server error with demo data or retry it", async () => {
    vi.stubEnv("VITE_API_MODE", "live");
    const fetchMock = vi
      .fn()
      .mockResolvedValue(
        new Response(
          JSON.stringify({
            error: { code: "service_busy", message: "混み合っています。" },
          }),
          { status: 503 },
        ),
      );
    vi.stubGlobal("fetch", fetchMock);
    const { getHealth } = await import("./client");
    await expect(getHealth()).rejects.toMatchObject({
      code: "service_busy",
      status: 503,
      message: "混み合っています。",
    });
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("waits up to 180 seconds for a route response, not the 45-second default", async () => {
    vi.useFakeTimers();
    vi.stubEnv("VITE_API_MODE", "live");
    vi.stubGlobal(
      "fetch",
      vi.fn(
        (_url: string, options: RequestInit) =>
          new Promise((_resolve, reject) =>
            options.signal?.addEventListener("abort", () =>
              reject(new DOMException("aborted", "AbortError")),
            ),
          ),
      ),
    );
    const { searchRoutes } = await import("./client");
    let settled = false;
    const result = searchRoutes({
      origin: { lat: 35.6812, lon: 139.7671 },
      dest: { lat: 35.7138, lon: 139.7773 },
      mode: "walk",
      period: "all",
      day_type: "all",
      weather: "all",
      max_detour_min: 10,
    }).finally(() => {
      settled = true;
    });
    const assertion = expect(result).rejects.toMatchObject({ code: "timeout" });
    await vi.advanceTimersByTimeAsync(179_000);
    expect(settled).toBe(false);
    await vi.advanceTimersByTimeAsync(1_000);
    await assertion;
  });

  it("ends condition requests at 10 seconds and distinguishes timeouts from caller aborts", async () => {
    vi.useFakeTimers();
    vi.stubEnv("VITE_API_MODE", "live");
    vi.stubGlobal(
      "fetch",
      vi.fn(
        (_url: string, options: RequestInit) =>
          new Promise((_resolve, reject) =>
            options.signal?.addEventListener("abort", () =>
              reject(new DOMException("aborted", "AbortError")),
            ),
          ),
      ),
    );
    const { getConditions } = await import("./client");
    const result = getConditions("2026-10-24T12:00+09:00");
    const assertion = expect(result).rejects.toMatchObject({ code: "timeout" });
    await vi.advanceTimersByTimeAsync(10_000);
    await assertion;
  });

  it("reports a timeout when synchronous contract checks fail after the deadline", async () => {
    vi.stubEnv("VITE_API_MODE", "live");
    let now = 0;
    vi.spyOn(performance, "now").mockImplementation(() => now);
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        // The body arrives just before the deadline; parsing and checks then run past it.
        now = 10_001;
        return new Response(JSON.stringify({ invalid: true }), {
          status: 200,
        });
      }),
    );
    const { getConditions } = await import("./client");
    await expect(
      getConditions("2026-10-24T12:00+09:00"),
    ).rejects.toMatchObject({ code: "timeout" });
  });

  it("keeps an invalid response that completes within the deadline as invalid_response", async () => {
    vi.stubEnv("VITE_API_MODE", "live");
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(JSON.stringify({ invalid: true }), { status: 200 }),
      ),
    );
    const { getConditions } = await import("./client");
    await expect(
      getConditions("2026-10-24T12:00+09:00"),
    ).rejects.toMatchObject({ code: "invalid_response" });
  });
});
