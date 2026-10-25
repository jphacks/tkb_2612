import { readFileSync } from "node:fs";
import { parse } from "yaml";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  afterAll,
  afterEach,
  beforeEach,
  describe,
  expect,
  it,
  vi,
} from "vitest";
import type {
  ConditionsResponse,
  GeocodeResponse,
  RouteRequest,
  RouteResponse,
} from "./lib/types";
import { createFetchCounter } from "./test/fetchCounter";

vi.hoisted(() => {
  // isDemoはimport時に確定するので、Appと実クライアントの読込前に設定する。
  vi.stubEnv("VITE_API_MODE", "live");
  vi.stubEnv("VITE_MAP_STYLE_URL", undefined);
});
vi.mock("./components/LogoIntro", () => ({
  LogoIntro: ({ onComplete }: { onComplete: () => void }) => (
    <button onClick={onComplete}>動画をスキップして開始</button>
  ),
}));
// styleの読込完了だけを手動にし、Appから実レイヤー処理までを通す。
type LiveSource = {
  data: import("geojson").FeatureCollection;
  setData: (data: import("geojson").FeatureCollection) => void;
};
type LiveMap = {
  sources: Map<string, LiveSource>;
  emit: (name: string) => void;
  remove: ReturnType<typeof vi.fn>;
};
const live = vi.hoisted(() => ({ maps: [] as LiveMap[] }));
vi.mock("maplibre-gl", () => ({
  setWorkerUrl: vi.fn(),
  NavigationControl: class {},
  AttributionControl: class {},
  Map: class {
    sources = new Map<string, LiveSource>();
    layers = new Map<string, import("maplibre-gl").LayerSpecification>();
    listeners = new Map<string, Set<() => void>>();
    loaded = false;
    constructor() {
      live.maps.push(this);
    }
    emit(name: string) {
      if (name === "load") this.loaded = true;
      this.listeners.get(name)?.forEach((listener) => listener());
    }
    on(name: string, listener: () => void) {
      if (!this.listeners.has(name)) this.listeners.set(name, new Set());
      this.listeners.get(name)!.add(listener);
    }
    off(name: string, listener: () => void) {
      this.listeners.get(name)?.delete(listener);
    }
    isStyleLoaded = () => this.loaded;
    getStyle = () =>
      this.loaded ? { version: 8, sources: {}, layers: [] } : undefined;
    getSource = (id: string) => this.sources.get(id);
    getLayer = (id: string) => this.layers.get(id);
    addSource(id: string, spec: { data: import("geojson").FeatureCollection }) {
      const source: LiveSource = {
        data: structuredClone(spec.data),
        setData: (data) => {
          source.data = structuredClone(data);
          this.emit("styledata");
        },
      };
      this.sources.set(id, source);
      this.emit("styledata");
    }
    addLayer(layer: import("maplibre-gl").LayerSpecification) {
      this.layers.set(layer.id, layer);
      this.emit("styledata");
    }
    addControl = vi.fn();
    remove = vi.fn();
    fitBounds = vi.fn();
    easeTo = vi.fn();
    project = () => ({ x: 100, y: 100 });
  },
}));
import App from "./App";

const read = (path: string) =>
  readFileSync(new URL(path, import.meta.url), "utf8");
const openapi = parse(read("../../docs/api/openapi.yaml"));
const conditionsExamples =
  openapi.paths["/api/conditions"].get.responses["200"].content[
    "application/json"
  ].examples;
const forecast = conditionsExamples.holidayForecast.value as ConditionsResponse;
const noLocation = conditionsExamples.noLocation.value as ConditionsResponse;
const routeExample = openapi.paths["/api/route"].post.responses["200"].content[
  "application/json"
].examples.thinnedPoints.value as RouteResponse;
const c2 = JSON.parse(
  read("../../backend/tests/fixtures/api-responses.json"),
).responses;
const origin = { label: "出発地点", lat: 35.01, lon: 139 };
const dest = { label: "到着地点", lat: 35.01, lon: 139.01 };
const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((accept, decline) => {
    resolve = accept;
    reject = decline;
  });
  return { promise, resolve, reject };
}
let automatic: ConditionsResponse;
let counter: ReturnType<typeof createFetchCounter>;
let conditionsHandler: ((url: URL) => Promise<Response>) | undefined;
let geocodeHandler: ((url: URL) => Promise<Response>) | undefined;
let routeHandler: (() => Promise<Response>) | undefined;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date("2026-10-24T01:10:00+09:00"));
  live.maps.length = 0;
  vi.stubEnv("VITE_MAP_STYLE_URL", undefined);
  automatic = structuredClone(forecast);
  conditionsHandler = undefined;
  geocodeHandler = undefined;
  routeHandler = undefined;
  counter = createFetchCounter(async (input) => {
    const url = new URL(String(input), "http://localhost");
    switch (url.pathname) {
      case "/api/health":
        return json(c2["health.ready"].body);
      case "/api/conditions":
        return conditionsHandler
          ? conditionsHandler(url)
          : json(url.searchParams.has("lat") ? automatic : noLocation);
      case "/api/geocode":
        return geocodeHandler
          ? geocodeHandler(url)
          : json({
              results: [
                url.searchParams.get("q") === "出発地点" ? origin : dest,
              ],
              attribution: c2["geocode.multiple"].body.attribution,
            } satisfies GeocodeResponse);
      case "/api/route":
        return routeHandler ? routeHandler() : json(routeExample);
      default:
        throw new Error(`想定外の通信: ${url.pathname}`);
    }
  });
  vi.stubGlobal("fetch", counter.fetch);
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});
afterAll(() => vi.unstubAllEnvs());

async function start() {
  const user = userEvent.setup();
  render(<App />);
  await user.click(
    screen.getByRole("button", { name: "動画をスキップして開始" }),
  );
  return user;
}
async function choose(
  user: ReturnType<typeof userEvent.setup>,
  label: string,
  text: string,
) {
  fireEvent.change(screen.getByRole("combobox", { name: label }), {
    target: { value: text },
  });
  await user.click(
    screen.getByRole("button", { name: `${label}の候補を検索` }),
  );
  await user.click(await screen.findByRole("option", { name: text }));
}
async function openConditions() {
  const user = await start();
  await choose(user, "出発地", origin.label);
  await choose(user, "目的地", dest.label);
  await user.click(screen.getByRole("button", { name: "条件を選ぶ" }));
  await ready();
  return user;
}
async function ready() {
  await waitFor(() =>
    expect(
      screen.getByRole("button", { name: "ルートを比較する" }),
    ).toBeEnabled(),
  );
}
const group = (name: string) => within(screen.getByRole("group", { name }));
const lastRequest = () =>
  JSON.parse(
    String(
      [...counter.fetch.mock.calls]
        .reverse()
        .find(([input]) => String(input) === "/api/route")?.[1]?.body,
    ),
  ) as RouteRequest;

describe("live App wiring (frontend-spec 3, 4, 5, 11)", () => {
  it("connects health, location-free conditions, explicit geocoding, rounded midpoint conditions and route examples", async () => {
    const user = await start();
    expect(screen.getByText("API接続")).toBeVisible();
    expect(screen.queryByText("デモ状態切替")).toBeNull();
    expect(counter.count("/api/health")).toBe(1);
    const initial = new URL(
      String(counter.fetch.mock.calls[1][0]),
      "http://localhost",
    );
    expect(initial.pathname).toBe("/api/conditions");
    expect(Object.fromEntries(initial.searchParams)).toEqual({
      datetime: "2026-10-24T01:10+09:00",
    });
    fireEvent.change(screen.getByRole("combobox", { name: "出発地" }), {
      target: { value: origin.label },
    });
    expect(counter.count("/api/geocode")).toBe(0);
    await user.click(
      screen.getByRole("button", { name: "出発地の候補を検索" }),
    );
    await user.click(await screen.findByRole("option", { name: origin.label }));
    await choose(user, "目的地", dest.label);
    await user.click(screen.getByRole("button", { name: "条件を選ぶ" }));
    await ready();
    expect(screen.getByText("自動：休日（祝日：文化の日）")).toBeVisible();
    // The weather supplement (source and target time) is intentionally not shown.
    expect(screen.queryByText(/自動：雨（Open-Meteo/)).toBeNull();
    const conditionCalls = counter.fetch.mock.calls.filter(([input]) =>
      String(input).startsWith("/api/conditions?"),
    );
    const urls = conditionCalls.map(
      ([input]) => new URL(String(input), "http://localhost"),
    );
    // 初期表示と片地点だけの確定では、生座標も中点も送らない。
    for (const url of urls.slice(0, -1))
      expect([...url.searchParams.keys()]).toEqual(["datetime"]);
    expect(Object.fromEntries(urls.at(-1)!.searchParams)).toEqual({
      datetime: expect.stringMatching(/\+09:00$/),
      lat: "35.01",
      lon: "139.00",
    });
    await user.click(screen.getByRole("button", { name: "ルートを比較する" }));
    await screen.findByRole("group", { name: "表示する経路" });
    expect(lastRequest()).toEqual({
      origin: { lat: 35.01, lon: 139 },
      dest: { lat: 35.01, lon: 139.01 },
      mode: "walk",
      period: "昼",
      day_type: "休日",
      weather: "雨",
      max_detour_min: 10,
    });
    const options = counter.fetch.mock.calls.find(
      ([input]) => input === "/api/route",
    )![1]!;
    expect(options).toMatchObject({
      method: "POST",
      cache: "no-store",
      headers: {
        Accept: "application/json",
        "Content-Type": "application/json",
      },
    });
    expect(counter.count("/api/route")).toBe(1);
    const checkpoint = counter.mark();
    // The sidebar starts closed; toggling it never calls the API.
    await user.click(
      screen.getByRole("button", { name: "条件サイドバーを開く" }),
    );
    await user.click(
      screen.getByRole("button", { name: "条件サイドバーを閉じる" }),
    );
    expect(counter.count(undefined, checkpoint)).toBe(0);
  });

  it("preserves manual fields during refresh and resets only the requested field to auto", async () => {
    const user = await openConditions();
    await user.click(screen.getByRole("radio", { name: "雪" }));
    await user.click(screen.getByRole("radio", { name: /夕 16:00/ }));
    await user.click(screen.getByRole("radio", { name: "平日" }));
    await user.click(group("天気").getByRole("button", { name: "自動に戻す" }));
    await ready();
    expect(screen.getByRole("radio", { name: "雨" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByRole("radio", { name: /夕 16:00/ })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByRole("radio", { name: "平日" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(group("時間帯").getByText("手動")).toBeVisible();
    expect(group("曜日区分").getByText("手動")).toBeVisible();
    expect(group("天気").getByText("自動")).toBeVisible();
  });

  it("returns every field to automatic values when departure changes and switches back to now", async () => {
    const user = await openConditions();
    await user.click(screen.getByRole("radio", { name: "雪" }));
    await user.click(screen.getByRole("radio", { name: /夕 16:00/ }));
    await user.click(screen.getByRole("radio", { name: "平日" }));
    await user.click(screen.getByRole("radio", { name: "日時を指定" }));
    await ready();
    expect(screen.queryAllByText("手動")).toHaveLength(0);
    // モード切替だけでなく、日時指定のまま時刻を編集した場合も自動に戻す。
    await user.click(screen.getByRole("radio", { name: "雪" }));
    await user.click(screen.getByRole("radio", { name: /夕 16:00/ }));
    await user.click(screen.getByRole("radio", { name: "平日" }));
    fireEvent.change(screen.getByLabelText("出発日時（日本時間）"), {
      target: { value: "2026-11-03T14:30" },
    });
    await ready();
    expect(screen.queryAllByText("手動")).toHaveLength(0);
    expect(screen.getByRole("radio", { name: "雨" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByRole("radio", { name: /昼 09:00/ })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByRole("radio", { name: "休日" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    const custom = new URL(
      String(counter.fetch.mock.calls.at(-1)![0]),
      "http://localhost",
    );
    expect(custom.searchParams.get("datetime")).toBe("2026-11-03T14:30+09:00");
    vi.setSystemTime(new Date("2026-11-03T15:31:00+09:00"));
    await user.click(screen.getByRole("radio", { name: "今すぐ出発" }));
    await ready();
    const now = new URL(
      String(counter.fetch.mock.calls.at(-1)![0]),
      "http://localhost",
    );
    expect(now.searchParams.get("datetime")).toBe("2026-11-03T15:31+09:00");
    expect(screen.queryByLabelText("出発日時（日本時間）")).toBeNull();
    expect(screen.getByRole("radio", { name: "今すぐ出発" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
  });

  // 日時とAPIの固定応答を対にする。境界や祝日をフロントの計算で求めない。
  it.each<
    [
      name: string,
      datetime: string,
      period: ConditionsResponse["period"],
      dayType: ConditionsResponse["day_type"],
      reason: string,
      covered: boolean,
    ]
  >([
    ["3:59", "2026-11-06T03:59", "深夜", "平日", "平日", true],
    ["4:00", "2026-11-06T04:00", "早朝", "平日", "平日", true],
    ["8:59", "2026-11-06T08:59", "早朝", "平日", "平日", true],
    ["9:00", "2026-11-06T09:00", "昼", "平日", "平日", true],
    ["15:59", "2026-11-06T15:59", "昼", "平日", "平日", true],
    ["16:00", "2026-11-06T16:00", "夕", "平日", "平日", true],
    ["18:59", "2026-11-06T18:59", "夕", "平日", "平日", true],
    ["19:00", "2026-11-06T19:00", "夜", "平日", "平日", true],
    ["23:59（金曜）", "2026-11-06T23:59", "夜", "平日", "平日", true],
    ["0:00（土曜の暦日）", "2026-11-07T00:00", "深夜", "休日", "土曜", true],
    ["平日", "2026-11-04T12:00", "昼", "平日", "平日", true],
    ["土曜", "2026-11-07T12:00", "昼", "休日", "土曜", true],
    ["日曜", "2026-11-08T12:00", "昼", "休日", "日曜", true],
    ["祝日", "2026-11-03T12:00", "昼", "休日", "祝日：文化の日", true],
    ["振替休日", "2026-05-06T12:00", "昼", "休日", "振替休日", true],
    ["国民の休日", "2026-09-22T12:00", "昼", "休日", "国民の休日", true],
    ["年末年始の開始前", "2026-12-28T23:59", "夜", "平日", "平日", true],
    ["年末年始の開始", "2026-12-29T00:00", "深夜", "休日", "年末年始", true],
    ["年末年始の終了", "2027-01-03T23:59", "夜", "休日", "年末年始", true],
    ["年末年始の終了後", "2027-01-04T00:00", "深夜", "平日", "平日", true],
    ["祝日表範囲外", "2099-11-03T12:00", "昼", "平日", "平日", false],
    // 日時から想定される区分と異なる応答でも、サーバーの値を優先する。
    ["API応答を優先", "2026-11-03T04:00", "夕", "平日", "平日", true],
  ])(
    "displays fixed conditions for %s",
    async (_name, datetime, period, dayType, reason, covered) => {
      const user = await openConditions();
      await user.click(screen.getByRole("radio", { name: "日時を指定" }));
      await ready();
      const response: ConditionsResponse = {
        api_version: "2.0",
        datetime: `${datetime}:00+09:00`,
        period,
        day_type: dayType,
        day_type_reason: reason,
        holiday_table_covered: covered,
        weather: null,
        weather_status: "unavailable",
        weather_source: null,
      };
      conditionsHandler = async () => json(response);
      const checkpoint = counter.mark();
      fireEvent.change(screen.getByLabelText("出発日時（日本時間）"), {
        target: { value: datetime },
      });
      await ready();
      expect(counter.count("/api/conditions", checkpoint)).toBe(1);
      const request = new URL(
        String(counter.fetch.mock.calls.at(-1)![0]),
        "http://localhost",
      );
      expect(Object.fromEntries(request.searchParams)).toEqual({
        datetime: `${datetime}+09:00`,
        lat: "35.01",
        lon: "139.00",
      });
      expect(screen.getByLabelText("出発日時（日本時間）")).toHaveValue(
        datetime,
      );
      expect(screen.getByRole("radio", { name: "日時を指定" })).toHaveAttribute(
        "aria-checked",
        "true",
      );
      for (const radio of group("時間帯").getAllByRole("radio"))
        expect(radio).toHaveAttribute(
          "aria-checked",
          String(radio.textContent?.startsWith(period)),
        );
      for (const radio of group("曜日区分").getAllByRole("radio"))
        expect(radio).toHaveAttribute(
          "aria-checked",
          String(radio.textContent === dayType),
        );
      expect(group("時間帯").getByText("自動")).toBeVisible();
      expect(
        group("曜日区分").getByText(`自動：${dayType}（${reason}）`),
      ).toBeVisible();
      expect(
        group("天気").getByRole("button", { name: "指定しない" }),
      ).toHaveAttribute("aria-pressed", "true");
      if (covered)
        expect(screen.queryByText(/祝日表の収録範囲外の日付/)).toBeNull();
      else expect(screen.getByText(/祝日表の収録範囲外の日付/)).toBeVisible();
    },
  );

  it.each([
    // ok: the weather is applied, but its source supplement is not shown.
    ["conditions.currentOk", null],
    ["conditions.past", "過去の日時の天気は自動設定できません。"],
    ["conditions.outOfRange", "指定日時は天気予報の対象期間外です。"],
    ["conditions.unavailable", "天気の取得元を利用できません。"],
    ["conditions.noLocation", "両地点を確定すると天気を取得できます。"],
  ])("displays the server's weather status for %s", async (name, message) => {
    automatic = structuredClone(c2[name].body);
    await openConditions();
    if (message) expect(screen.getByText(message)).toBeVisible();
    else expect(screen.queryByText(/自動：曇（Open-Meteo/)).toBeNull();
    if (automatic.weather_status !== "ok")
      expect(
        group("天気").getByRole("button", { name: "指定しない" }),
      ).toHaveAttribute("aria-pressed", "true");
    else
      expect(screen.getByRole("radio", { name: "曇" })).toHaveAttribute(
        "aria-checked",
        "true",
      );
  });

  it("uses the server's weekday even on an uncovered holiday date and shows the coverage warning", async () => {
    automatic = structuredClone(c2["conditions.holidayTableNotCovered"].body);
    await openConditions();
    expect(screen.getByText(/祝日表の収録範囲外の日付/)).toBeVisible();
    expect(screen.getByRole("radio", { name: "平日" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByText("自動：平日（平日）")).toBeVisible();
  });

  it("keeps manual weather and clears only automatic fields on a conditions failure without retrying", async () => {
    const user = await openConditions();
    await user.click(screen.getByRole("radio", { name: "雪" }));
    await user.click(screen.getByRole("radio", { name: /夕 16:00/ }));
    const checkpoint = counter.mark();
    conditionsHandler = async () =>
      json(c2["error.invalidRequest.conditions"].body, 400);
    await user.click(
      group("時間帯").getByRole("button", { name: "自動に戻す" }),
    );
    await ready();
    expect(screen.getByText(/自動項目は指定なしに戻しました/)).toBeVisible();
    expect(screen.getByRole("radio", { name: "雪" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByRole("radio", { name: "全曜日" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByRole("button", { name: "全時間帯" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(counter.count("/api/conditions", checkpoint)).toBe(1);
    expect(counter.count("/api/route", checkpoint)).toBe(0);
  });

  it.each([
    ["success", false],
    ["error", false],
    ["success", true],
    ["error", true],
  ] as const)(
    "discards a stale conditions %s (current completed: %s)",
    async (outcome, completed) => {
      const user = await openConditions();
      const old = deferred<Response>();
      const current = deferred<Response>();
      let requests = 0;
      conditionsHandler = () =>
        ++requests === 1 ? old.promise : current.promise;
      await user.click(screen.getByRole("radio", { name: "日時を指定" }));
      fireEvent.change(screen.getByLabelText("出発日時（日本時間）"), {
        target: { value: "2026-11-04T12:00" },
      });
      await waitFor(() => expect(requests).toBe(2));
      if (completed) {
        await act(async () => current.resolve(json(forecast)));
        await ready();
      }
      await act(async () => {
        if (outcome === "success")
          old.resolve(json(c2["conditions.currentOk"].body));
        else old.reject(new TypeError("古い通信エラー"));
      });
      if (!completed) {
        expect(
          screen.getByRole("button", { name: "ルートを比較する" }),
        ).toBeDisabled();
        expect(screen.getByText("条件を取得しています…")).toBeVisible();
        await act(async () => current.resolve(json(forecast)));
      }
      expect(screen.queryByText(/自動項目は指定なし/)).toBeNull();
      await ready();
      expect(screen.getByRole("radio", { name: "雨" })).toHaveAttribute(
        "aria-checked",
        "true",
      );
      expect(screen.getByRole("radio", { name: "曇" })).toHaveAttribute(
        "aria-checked",
        "false",
      );
    },
  );

  it.each([15 * 60_000 - 1, 15 * 60_000])(
    "refreshes now conditions only at the 15-minute boundary (%i ms)",
    async (elapsed) => {
      const startedAt = Date.now();
      const user = await openConditions();
      const checkpoint = counter.mark();
      vi.setSystemTime(startedAt + elapsed);
      const pending = deferred<Response>();
      conditionsHandler = () => pending.promise;
      await user.click(
        screen.getByRole("button", { name: "ルートを比較する" }),
      );
      if (elapsed === 15 * 60_000) {
        expect(counter.count("/api/conditions", checkpoint)).toBe(1);
        expect(counter.count("/api/route", checkpoint)).toBe(0);
        const request = new URL(
          String(counter.fetch.mock.calls.at(-1)![0]),
          "http://localhost",
        );
        expect(request.searchParams.get("datetime")).toBe(
          "2026-10-24T01:25+09:00",
        );
        routeHandler = async () =>
          json({
            ...routeExample,
            effective_conditions: {
              ...routeExample.effective_conditions,
              day_type: "平日",
            },
          });
        await act(async () =>
          pending.resolve(json(c2["conditions.forecastOk"].body)),
        );
      } else expect(counter.count("/api/conditions", checkpoint)).toBe(0);
      await screen.findByRole("group", { name: "表示する経路" });
      expect(counter.count("/api/route", checkpoint)).toBe(1);
      expect(lastRequest().day_type).toBe(
        elapsed === 15 * 60_000 ? "平日" : "休日",
      );
    },
  );

  it("drops a late address error after editing and keeps the newer confirmed place", async () => {
    const user = await start();
    const old = deferred<Response>();
    geocodeHandler = (url) =>
      url.searchParams.get("q") === "古い住所"
        ? old.promise
        : Promise.resolve(
            json({
              results: [origin],
              attribution: c2["geocode.multiple"].body.attribution,
            }),
          );
    fireEvent.change(screen.getByRole("combobox", { name: "出発地" }), {
      target: { value: "古い住所" },
    });
    await user.click(
      screen.getByRole("button", { name: "出発地の候補を検索" }),
    );
    await waitFor(() => expect(counter.count("/api/geocode")).toBe(1));
    expect(
      screen.getByRole("button", { name: "出発地の候補を検索" }),
    ).toBeDisabled();
    await choose(user, "出発地", origin.label);
    await act(async () => old.reject(new TypeError("古い通信エラー")));
    expect(screen.getByRole("combobox", { name: "出発地" })).toHaveValue(
      origin.label,
    );
    expect(screen.getByText("地点を確定しました")).toBeVisible();
    expect(screen.queryByText(/住所候補を取得できません/)).toBeNull();
    expect(counter.count("/api/geocode")).toBe(2);
  });

  it("suppresses repeated route submissions while the response is pending", async () => {
    const user = await openConditions();
    const pending = deferred<Response>();
    routeHandler = () => pending.promise;
    await user.dblClick(
      screen.getByRole("button", { name: "ルートを比較する" }),
    );
    // The result page opens with the sidebar closed.
    await user.click(
      screen.getByRole("button", { name: "条件サイドバーを開く" }),
    );
    expect(screen.getByRole("button", { name: "再検索" })).toBeDisabled();
    expect(counter.count("/api/route")).toBe(1);
    await act(async () => pending.resolve(json(routeExample)));
    await screen.findByRole("group", { name: "表示する経路" });
    expect(counter.count("/api/route")).toBe(1);
  });
});

describe("live Appの地図初期化との競合", () => {
  beforeEach(() => {
    vi.stubEnv("VITE_MAP_STYLE_URL", "https://example.invalid/style.json");
  });

  it.each(["応答完了", "再検索", "条件変更"] as const)(
    "%sの後にloadしても最新の結果を各sourceへ反映する",
    async (operation) => {
      const user = await openConditions();
      const pending = deferred<Response>();
      routeHandler = () => pending.promise;
      await user.click(
        screen.getByRole("button", { name: "ルートを比較する" }),
      );
      await waitFor(() => expect(live.maps).toHaveLength(1));
      const map = live.maps[0];
      expect(map.sources.size).toBe(0);
      await act(async () => pending.resolve(json(routeExample)));
      await screen.findByRole("group", { name: "表示する経路" });
      // The result page opens with the sidebar closed.
      await user.click(
        screen.getByRole("button", { name: "条件サイドバーを開く" }),
      );
      let latest: RouteResponse | null = routeExample;

      if (operation === "再検索") {
        // 前の応答とgeometry・区間・事故点が異なる契約例を返す。
        latest = structuredClone(
          openapi.paths["/api/route"].post.responses["200"].content[
            "application/json"
          ].examples.singleRoute.value,
        ) as RouteResponse;
        latest.effective_conditions = { ...routeExample.effective_conditions };
        const next = deferred<Response>();
        routeHandler = () => next.promise;
        await user.click(screen.getByRole("button", { name: "再検索" }));
        await act(async () => next.resolve(json(latest)));
        await screen.findByRole("group", { name: "表示する経路" });
      } else if (operation === "条件変更") {
        await user.click(screen.getByRole("radio", { name: "雪" }));
        expect(
          screen.queryByRole("group", { name: "表示する経路" }),
        ).toBeNull();
        latest = null;
      }

      expect(counter.count("/api/route")).toBe(operation === "再検索" ? 2 : 1);
      expect(live.maps).toHaveLength(1);
      expect(map.sources.size).toBe(0);
      act(() => map.emit("load"));
      expect([...map.sources.keys()].sort()).toEqual(
        [
          "route-fastest",
          "route-safest",
          "route-high-risk",
          "route-accidents",
          "current-location",
          "current-accuracy",
          "navigation-route",
          "navigation-alert",
        ].sort(),
      );
      const selected = latest?.candidates[0];
      expect(map.sources.get("route-fastest")!.data).toEqual({
        type: "FeatureCollection",
        features: selected
          ? [
              {
                type: "Feature",
                id: selected.id,
                geometry: selected.geometry,
                properties: { selected: true, color: "#16a34a" },
              },
            ]
          : [],
      });
      expect(map.sources.get("route-high-risk")!.data).toEqual({
        type: "FeatureCollection",
        features:
          selected?.high_risk_sections.features.map((section) => ({
            ...section,
            properties: { ...section.properties, selected: false },
          })) ?? [],
      });
      expect(map.sources.get("route-accidents")!.data).toEqual({
        type: "FeatureCollection",
        features: selected?.accident_points.features ?? [],
      });
      for (const id of [
        "route-safest",
        "current-location",
        "current-accuracy",
        "navigation-route",
        "navigation-alert",
      ])
        expect(map.sources.get(id)!.data).toEqual({
          type: "FeatureCollection",
          features: [],
        });
      expect(map.remove).not.toHaveBeenCalled();
      expect(screen.queryByText("地図を利用できません")).toBeNull();
    },
  );
});
