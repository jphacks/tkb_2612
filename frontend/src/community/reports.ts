import type { LatLon, SafetyKind, SafetyReport } from "../lib/types";

export type { SafetyReport, SafetyComment } from "../lib/types";

/** 地図の表示範囲（両端を含む、十進度）。 */
export type SafetyBounds = {
  minLat: number;
  minLon: number;
  maxLat: number;
  maxLon: number;
};

export const VOTER_KEY = "accilessnavi.voter-id.v1";
const VOTER_PATTERN = /^[A-Za-z0-9_-]{16,64}$/;
let sessionVoter: string | null = null;

/**
 * ブラウザーごとの匿名の投票者ID。サーバは同じ地点への2回目の投票をこのIDで拒否する。
 * 保存できない環境（プライベートブラウズ等）ではタブを開いている間だけのIDになる。
 */
export function voterId(): string {
  const saved = savedVoterId();
  if (saved) return saved;
  const id = sessionVoter ?? crypto.randomUUID();
  try {
    localStorage.setItem(VOTER_KEY, id);
  } catch {
    // 保存できなくてもタブを開いている間は同じIDを使う
  }
  sessionVoter = id;
  return id;
}

/**
 * 作らずに読むだけの投票者ID（一覧で自分の票を知るため）。まだ投票していなければnull。
 * 閲覧だけ（ナビ等）ではストレージに何も書かない。
 * 保存できなかった環境（プライベートブラウズ・容量切れ等）ではタブ内のIDを使う。
 */
export function storedVoterId(): string | null {
  return savedVoterId() ?? sessionVoter;
}

/** ストレージに保存済みの有効な投票者ID。 */
function savedVoterId(): string | null {
  try {
    const saved = localStorage.getItem(VOTER_KEY);
    return saved && VOTER_PATTERN.test(saved) ? saved : null;
  } catch {
    return null;
  }
}

export function pointKey(point: LatLon) {
  return `${point.lat.toFixed(6)},${point.lon.toFixed(6)}`;
}

/** 地図の色に使う種類。同数なら注意を優先して「危険」。 */
export function dominantKind(report: SafetyReport): SafetyKind {
  return report.danger_count >= report.safe_count ? "danger" : "safe";
}

export function totalVotes(report: SafetyReport) {
  return report.safe_count + report.danger_count;
}

/** 読み上げ・ツールチップ用の票の内訳。 */
export function tallyLabel(report: SafetyReport) {
  return `安全${report.safe_count}票・危険${report.danger_count}票`;
}

/** 点の集まりを `margin` 度だけ広げた範囲。 */
export function boundsAround(
  points: readonly (readonly number[])[],
  margin: number,
): SafetyBounds | null {
  if (!points.length) return null;
  const lons = points.map((point) => point[0]);
  const lats = points.map((point) => point[1]);
  return clampBounds({
    minLon: Math.min(...lons) - margin,
    minLat: Math.min(...lats) - margin,
    maxLon: Math.max(...lons) + margin,
    maxLat: Math.max(...lats) + margin,
  });
}

export function clampBounds(bounds: SafetyBounds): SafetyBounds {
  const lat = (value: number) => Math.max(-90, Math.min(90, value));
  const lon = (value: number) => Math.max(-180, Math.min(180, value));
  return {
    minLat: lat(bounds.minLat),
    minLon: lon(bounds.minLon),
    maxLat: lat(bounds.maxLat),
    maxLon: lon(bounds.maxLon),
  };
}

/** 一覧を読める範囲の幅の上限（緯度・経度とも、十進度。サーバと同じ）。 */
export const MAX_BOUNDS_SPAN_DEG = 1;

/**
 * サーバが受け付けない広さの範囲か。送るときの丸め（小数5桁）で上限を越えないよう、
 * 少しだけ手前で打ち切る。
 */
export function isTooWide(bounds: SafetyBounds) {
  const limit = MAX_BOUNDS_SPAN_DEG - 0.0001;
  return (
    bounds.maxLat - bounds.minLat > limit || bounds.maxLon - bounds.minLon > limit
  );
}

/**
 * 経路の近くとみなす距離（m）。事故記録の「経路の近くの事故」の範囲
 * （応答の `unattributed_radius_m`）と同じ値にそろえる。
 */
export const NEAR_ROUTE_M = 72;
const METRES_PER_DEG = (6371008.8 * Math.PI) / 180;

/**
 * 地点から折れ線（`[lon, lat]` の並び）までの最短距離（m）。
 * 地点の緯度で平面に近似する（数百mの範囲なら誤差は無視できる）。
 */
export function distanceToLineM(
  point: LatLon,
  line: readonly (readonly number[])[],
): number {
  if (!line.length) return Infinity;
  const kx = METRES_PER_DEG * Math.cos((point.lat * Math.PI) / 180);
  const plane = ([lon, lat]: readonly number[]) => [
    (lon - point.lon) * kx,
    (lat - point.lat) * METRES_PER_DEG,
  ];
  let [x, y] = plane(line[0]);
  let best = Math.hypot(x, y);
  for (let index = 1; index < line.length; index++) {
    const [nextX, nextY] = plane(line[index]);
    const dx = nextX - x;
    const dy = nextY - y;
    const length2 = dx * dx + dy * dy;
    const t = length2
      ? Math.max(0, Math.min(1, -(x * dx + y * dy) / length2))
      : 0;
    best = Math.min(best, Math.hypot(x + t * dx, y + t * dy));
    [x, y] = [nextX, nextY];
  }
  return best;
}

/** 折れ線から `radius` m以内の地点（並びはそのまま）。 */
export function reportsNearLine<T extends LatLon>(
  reports: readonly T[],
  line: readonly (readonly number[])[],
  radius = NEAR_ROUTE_M,
): T[] {
  return reports.filter((report) => distanceToLineM(report, line) <= radius);
}

export function containsPoint(bounds: SafetyBounds, point: LatLon) {
  return (
    point.lat >= bounds.minLat &&
    point.lat <= bounds.maxLat &&
    point.lon >= bounds.minLon &&
    point.lon <= bounds.maxLon
  );
}
