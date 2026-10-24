import type {
  AccidentFeature,
  ConditionsResponse,
  GeocodeResponse,
  HealthResponse,
  LatLon,
  Period,
  Place,
  RouteCandidate,
  RouteRequest,
  RouteResponse,
  Weather,
} from "../lib/types";
import { toTokyoInput } from "../lib/format";
import { ApiError } from "./errors";

// Explicit synthetic fixtures: these addresses identify selectable places, but
// generated lines and accident counts do not represent real roads or records.
export const DEMO_PLACES: Place[] = [
  { label: "東京駅 — 東京都千代田区丸の内1丁目", lat: 35.6812, lon: 139.7671 },
  { label: "上野駅 — 東京都台東区上野7丁目", lat: 35.7138, lon: 139.7773 },
  {
    label: "秋葉原駅 — 東京都千代田区外神田1丁目",
    lat: 35.6984,
    lon: 139.7731,
  },
  { label: "新宿駅 — 東京都新宿区新宿3丁目", lat: 35.6909, lon: 139.7003 },
  { label: "渋谷駅 — 東京都渋谷区道玄坂1丁目", lat: 35.658, lon: 139.7016 },
  { label: "つくば駅 — 茨城県つくば市吾妻2丁目", lat: 36.0827, lon: 140.1115 },
  {
    label: "筑波大学 中央図書館 — 茨城県つくば市天王台1丁目",
    lat: 36.1098,
    lon: 140.1006,
  },
  {
    label: "研究学園駅 — 茨城県つくば市研究学園5丁目",
    lat: 36.0821,
    lon: 140.0811,
  },
  {
    label: "つくば市役所 — 茨城県つくば市研究学園1丁目",
    lat: 36.0835,
    lon: 140.0764,
  },
  {
    label: "洞峰公園 — 茨城県つくば市二の宮2丁目",
    lat: 36.0574,
    lon: 140.1174,
  },
];

const DATASET_ID = "d".repeat(64);
export const demoHealth: HealthResponse = {
  api_version: "2.0",
  status: "ok",
  release_id: "e".repeat(64),
  dataset_id: DATASET_ID,
  accidents: 4,
  load_seconds: 0,
  data_years: [2025],
};

export function findDemoPlaces(query: string): GeocodeResponse {
  const q = query.trim().normalize("NFKC").toLocaleLowerCase("ja");
  return {
    results: DEMO_PLACES.filter((place) =>
      place.label.normalize("NFKC").toLocaleLowerCase("ja").includes(q),
    ).slice(0, 5),
    attribution: [
      {
        text: "デモ候補：アプリ内サンプル地点",
        url: "https://github.com/jphacks/tkb_2612",
      },
    ],
  };
}

/** Demo adapter only. Live period and holiday classification belong to /api/conditions. */
export function demoPeriod(hour: number): Period {
  if (hour < 4) return "深夜";
  if (hour < 9) return "早朝";
  if (hour < 16) return "昼";
  if (hour < 19) return "夕";
  return "夜";
}

export function createDemoConditions(
  datetime: string,
  location?: LatLon,
): ConditionsResponse {
  const date = new Date(datetime);
  if (!Number.isFinite(date.getTime()))
    throw new ApiError("出発日時を確認してください。", "invalid_request", 400);
  const local = toTokyoInput(date);
  const day = new Date(`${local}:00Z`);
  const monthDay = local.slice(5, 10);
  const yearEnd = monthDay >= "12-29" || monthDay <= "01-03";
  const weekend = day.getUTCDay() === 0 || day.getUTCDay() === 6;
  return {
    api_version: "2.0",
    datetime: `${local}:00+09:00`,
    period: demoPeriod(day.getUTCHours()),
    day_type: yearEnd || weekend ? "休日" : "平日",
    day_type_reason: yearEnd
      ? "年末年始"
      : day.getUTCDay() === 6
        ? "土曜"
        : day.getUTCDay() === 0
          ? "日曜"
          : "平日",
    holiday_table_covered: false,
    weather: location ? "晴" : null,
    weather_status: location ? "ok" : "no_location",
    // This is a schema-compatible fixture, NOT a call to the named provider.
    // The UI labels automatic weather as fixed demo data.
    weather_source: location
      ? {
          provider: "Open-Meteo",
          target_time: `${local.slice(0, 13)}:00:00+09:00`,
          kind: "forecast",
        }
      : null,
  };
}

const round = (value: number, digits: number) => Number(value.toFixed(digits));
type Coordinate = [number, number];
const distance = (a: Coordinate, b: Coordinate) =>
  ((Math.PI * 6_371_008.8) / 180) *
  Math.hypot(
    a[1] - b[1],
    (a[0] - b[0]) * Math.cos((((a[1] + b[1]) / 2) * Math.PI) / 180),
  );
const interpolate = (
  a: Coordinate,
  b: Coordinate,
  ratio: number,
): Coordinate => [a[0] + (b[0] - a[0]) * ratio, a[1] + (b[1] - a[1]) * ratio];

function periodOf(request: RouteRequest): Period | "all" {
  const value = request.period ?? request.時間帯 ?? "all";
  const canonical =
    typeof value === "number"
      ? (["早朝", "昼", "夕", "夜", "深夜"] as const)[value]
      : value;
  return canonical === "all" && request.hour != null
    ? demoPeriod(request.hour)
    : canonical;
}

function weatherOf(request: RouteRequest): Weather | "all" {
  const value = request.weather ?? "all";
  return typeof value === "number"
    ? (["晴", "曇", "雨", "霧", "雪"] as const)[value - 1]
    : value;
}

export function createDemoRouteResponse(
  request: RouteRequest,
  scenario: "normal" | "same" | "zero" = "normal",
): RouteResponse {
  const start: Coordinate = [request.origin.lon, request.origin.lat];
  const end: Coordinate = [request.dest.lon, request.dest.lat];
  const delta: Coordinate = [end[0] - start[0], end[1] - start[1]];
  const period = periodOf(request);
  const weather = weatherOf(request);
  const dayType = request.day_type ?? "all";
  const maxDetour = request.max_detour_min ?? 10;
  const speed = request.mode === "car" ? 9 : request.mode === "bike" ? 4 : 1.3;
  const baseTime = Math.max(
    180,
    Math.round((distance(start, end) * 1.15) / speed),
  );

  const candidates: RouteCandidate[] = [0, 1].map((index) => {
    const id = `route-${index + 1}`;
    const bend = index === 0 ? 0.09 : -0.2;
    const coords: Coordinate[] = [
      start,
      ...[0.34, 0.67].map((ratio) => {
        const point = interpolate(start, end, ratio);
        return [
          point[0] - delta[1] * bend,
          point[1] + delta[0] * bend,
        ] as Coordinate;
      }),
      end,
    ];
    const length = Math.round(
      coords
        .slice(1)
        .reduce((sum, point, i) => sum + distance(coords[i], point), 0),
    );
    const count =
      scenario === "zero"
        ? 0
        : scenario === "same"
          ? index + 1
          : index === 0
            ? 3
            : 1;
    const duration = baseTime + index * 360;
    const precedingSegments = Math.max(
      1,
      Math.ceil(distance(coords[0], coords[1]) / 75),
    );
    const sourceSegments = Math.max(
      1,
      Math.ceil(distance(coords[1], coords[2]) / 75),
    );
    const localSegment = Math.floor(sourceSegments / 2);
    const segmentIndex = precedingSegments + localSegment;
    const sectionStart = interpolate(
      coords[1],
      coords[2],
      localSegment / sourceSegments,
    );
    const sectionEnd = interpolate(
      coords[1],
      coords[2],
      (localSegment + 1) / sourceSegments,
    );
    const points: AccidentFeature[] = Array.from(
      { length: count },
      (_, number) => ({
        type: "Feature",
        id: index * 100 + number,
        geometry: {
          type: "Point",
          coordinates: interpolate(
            sectionStart,
            sectionEnd,
            (number + 1) / (count + 1),
          ),
        },
        properties: {
          deaths: 0,
          injuries: 3,
          source_year: 2025,
          date: dayType === "休日" ? 20250105 : 20250106,
          weather: weather === "all" ? "晴" : weather,
          period: period === "all" ? "昼" : period,
          day_type: dayType === "all" ? "平日" : dayType,
        },
      }),
    );
    const weight = round(
      count *
        3 *
        (weather === "雨" ? 2 : weather === "霧" || weather === "雪" ? 4 : 1),
      2,
    );
    return {
      id,
      duration,
      distance: length,
      geometry: { type: "LineString", coordinates: coords },
      accident_count: count,
      fatal_accidents: 0,
      injury_accidents: count,
      deaths: 0,
      injuries: count * 3,
      weather_match:
        weather === "all"
          ? null
          : {
              accident_count: count,
              fatal_accidents: 0,
              injury_accidents: count,
              deaths: 0,
              injuries: count * 3,
            },
      weighted_risk: weight,
      risk_per_km: round(weight / Math.max(length / 1000, 0.001), 3),
      accident_points: { type: "FeatureCollection", features: points },
      points_shown: count,
      high_risk_sections: {
        type: "FeatureCollection",
        features: count
          ? [
              {
                type: "Feature",
                id: `${id}:segment-${segmentIndex}`,
                geometry: {
                  type: "LineString",
                  coordinates: [sectionStart, sectionEnd],
                },
                properties: {
                  route_id: id,
                  severity_weight: count * 3,
                  segment_index: segmentIndex,
                  source_vertex_start_index: 1,
                  source_vertex_end_index: 2,
                },
              },
            ]
          : [],
      },
      high_risk_total: count ? 1 : 0,
      exact_condition_count: count,
      extra_minutes: index * 6,
      within_detour: duration <= baseTime + maxDetour * 60 + 1e-6,
    };
  });
  const fastest = candidates[0];
  const safest =
    scenario === "zero"
      ? null
      : [...candidates]
          .filter((route) => route.within_detour)
          .sort(
            (a, b) =>
              a.weighted_risk - b.weighted_risk ||
              a.duration - b.duration ||
              a.distance - b.distance,
          )[0];
  return {
    api_version: "2.0",
    dataset_id: DATASET_ID,
    fastest_id: fastest.id,
    safest_id: safest?.id ?? null,
    candidates,
    extra_minutes: safest?.extra_minutes ?? null,
    risk_reduction_percent:
      safest && fastest.weighted_risk
        ? round((1 - safest.weighted_risk / fastest.weighted_risk) * 100, 1)
        : null,
    reason: !safest
      ? "全候補で該当する過去の事故がないため、事故を考慮した経路は選んでいません。"
      : safest.id === fastest.id
        ? "今回は最短時間経路が事故を考慮した経路と同じです。"
        : `取得した2候補のうち、最短時間経路から+${maxDetour}分以内で重みの合計が最小の候補を選びました。`,
    warnings: [
      "デモデータです。経路の線・事故記録・所要時間は実際の道路や事故を示しません。",
      ...(scenario === "zero"
        ? ["該当する事故が0件でも、安全であることの証明ではありません。"]
        : []),
    ],
    relaxed_conditions: [],
    effective_conditions: {
      mode: request.mode ?? "walk",
      period: period === "all" ? "全時間帯" : period,
      day_type: dayType === "all" ? "全曜日" : dayType,
      weather: weather === "all" ? "全天候" : weather,
    },
    radius_m: 72,
    distance_weight_sigma_m: 24,
    aggregation_ms: 12.4,
    data_years: [2025],
    risk_note:
      "過去の事故に基づく参考指標です。経路から72m以内の事故の深刻度を距離と天候で重み付けします。交通量や通行回数で補正した事故確率ではなく、安全を保証するものではありません。",
  };
}
