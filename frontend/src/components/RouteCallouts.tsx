import { useId } from "react";
import { Send } from "lucide-react";
import type { RouteCandidate, RouteResponse } from "../lib/types";
import { formatMinutes } from "../lib/format";

export const SAFE_LABEL = "安全重視";
export const FASTEST_LABEL = "最短時間";
export const MIDDLE_LABEL = "バランス";

export type MapPoint = { x: number; y: number };
export type MapPinFootprint = MapPoint & { radius: number };
export type MapSize = { width: number; height: number };
export type RouteTone = "considered" | "fastest" | "middle" | "alternative";

export function routeTone(id: string, result: RouteResponse): RouteTone {
  if (id === result.safest_id) return "considered";
  if (id === result.middle_id) return "middle";
  return id === result.fastest_id ? "fastest" : "alternative";
}

export const routeColors: Record<RouteTone, string> = {
  considered: "#16a34a",
  fastest: "#2563eb",
  // 注意区間（--caution の橙）と区別できる色にする。
  middle: "#7c3aed",
  alternative: "#94a3b8",
};

export function featuredRoutes(result: RouteResponse): RouteCandidate[] {
  const ids = [...new Set([result.fastest_id, result.middle_id, result.safest_id])];
  return ids.flatMap((id) =>
    id === null
      ? []
      : result.candidates.filter((route) => route.id === id),
  );
}

/** 表示順の位置を A・B・C の記号にする。 */
export function routeSymbol(index: number): string {
  return String.fromCharCode("A".charCodeAt(0) + index);
}

export function featuredSymbol(
  result: RouteResponse,
  id: string,
): string | null {
  const index = featuredRoutes(result).findIndex((route) => route.id === id);
  return index < 0 ? null : routeSymbol(index);
}

// Follow the route itself so labels stay attached when the map moves or resizes.
export function routeAnchor(
  route: RouteCandidate,
  ratio: number,
): [number, number] {
  const points = route.geometry.coordinates;
  const lengths = points
    .slice(1)
    .map((point, index) =>
      Math.hypot(point[0] - points[index][0], point[1] - points[index][1]),
    );
  let remaining = lengths.reduce((sum, length) => sum + length, 0) * ratio;
  for (let index = 0; index < lengths.length; index++) {
    if (remaining <= lengths[index] && lengths[index] > 0) {
      const fraction = remaining / lengths[index];
      return [
        points[index][0] + (points[index + 1][0] - points[index][0]) * fraction,
        points[index][1] + (points[index + 1][1] - points[index][1]) * fraction,
      ];
    }
    remaining -= lengths[index];
  }
  return [points[0][0], points[0][1]];
}

type Props = {
  result: RouteResponse;
  positions: Record<string, MapPoint>;
  size: MapSize;
  selectedRouteId: string | null;
  hoveredRouteId: string | null;
  onSelectRoute: (id: string) => void;
  onHoverRoute: (id: string | null) => void;
  onNavigate: (id: string) => void;
  detached?: boolean;
  protectedPoints?: MapPinFootprint[];
};

const clamp = (value: number, min: number, max: number) =>
  Math.max(min, Math.min(value, Math.max(min, max)));

export function RouteCallouts({
  result,
  positions,
  size,
  selectedRouteId,
  hoveredRouteId,
  onSelectRoute,
  onHoverRoute,
  onNavigate,
  detached = false,
  protectedPoints = [],
}: Props) {
  const maskId = useId();
  const routes = featuredRoutes(result);
  const compact = size.width < 480;
  // 幅が狭い地図で3枚のときは、吹き出しではなく地図下端の縦一覧にする。
  const list = compact && routes.length === 3;
  let cardWidth = compact && routes.length > 1
    ? Math.max(136, (size.width - 36) / 2)
    : Math.min(208, size.width - 24);
  // Narrow maps use smaller text and padding (.is-compact).
  const cardHeight = compact ? 130 : size.height < 480 ? 134 : 146;
  const topInset = size.height < 480 ? 56 : 72;
  // 地図が低いと（説明の欄を開いたときなど）縦に積めないので、横に並べる。
  // ナビ開始のボタンが切れたり隠れたりしないように、カードを手前に出す（.is-tight）。
  const tight =
    !list &&
    size.height < topInset + (cardHeight + 16) * routes.length + 88;
  if (tight) {
    cardWidth = Math.max(
      120,
      Math.min(
        cardWidth,
        (size.width - 24 - 12 * (routes.length - 1)) / routes.length,
      ),
    );
  }
  const cards = routes.map((route) => {
    const tone = routeTone(route.id, result);
    const anchor = positions[route.id] ?? {
      x: size.width / 2,
      y: size.height / 2,
    };
    return {
      route,
      tone,
      anchor,
      x: clamp(
        tone === "considered" ? anchor.x - cardWidth - 28 : anchor.x + 28,
        12,
        size.width - cardWidth - 12,
      ),
      y: clamp(
        anchor.y - cardHeight / 2,
        topInset,
        size.height - cardHeight - 88,
      ),
    };
  });
  if (tight) {
    const y = Math.max(8, Math.min(topInset, size.height - cardHeight - 8));
    cards.forEach((card, index) => {
      card.x = routes.length > 1 ? 12 + index * (cardWidth + 12) : card.x;
      card.y = y;
    });
  } else if (!list && cards.length > 1) {
    // 重なりがあれば、時間の順に上から縦に積む。
    const overlaps = cards.some((first, index) =>
      cards
        .slice(index + 1)
        .some(
          (second) =>
            Math.abs(first.x - second.x) < cardWidth + 12 &&
            Math.abs(first.y - second.y) < cardHeight + 12,
        ),
    );
    if (overlaps) {
      const upper = clamp(
        Math.min(...cards.map((card) => card.y)) - cardHeight / 2,
        topInset,
        size.height -
          cardHeight * cards.length -
          16 * (cards.length - 1) -
          88,
      );
      cards.forEach((card, index) => {
        card.y = upper + index * (cardHeight + 16);
      });
    }
  }

  const fastest = result.candidates.find(
    (route) => route.id === result.fastest_id,
  );
  const baseDuration = fastest?.duration ?? routes[0]?.duration ?? 0;
  const cardContent = (
    route: RouteCandidate,
    tone: RouteTone,
    index: number,
    listRow = false,
  ) => {
    const same = route.id === result.fastest_id && route.id === result.safest_id;
    // APIのdurationは秒。最短時間経路との差を分にして小数1桁で示す。
    const extra = ((route.duration - baseDuration) / 60).toFixed(1);
    const label = same
      ? `${SAFE_LABEL}・${FASTEST_LABEL}`
      : tone === "middle"
        ? MIDDLE_LABEL
        : tone === "considered"
          ? SAFE_LABEL
          : FASTEST_LABEL;
    const extraTitle =
      same || tone === "fastest" ? null : `（+${extra}分）`;
    const title = extraTitle === null ? label : `${label}${extraTitle}`;
    const reduction =
      route.id === result.fastest_id ? null : route.risk_reduction_percent;
    const risk =
      reduction === null
        ? null
        : `推定リスク ${Math.abs(reduction).toFixed(1)}%${reduction < 0 ? "増" : "減"}`;
    // 一覧の行は2段組み。記号・見出し・所要時間を1段目、事故・削減率を2段目に置く。
    const content = listRow ? (
      <>
        <span
          className="map-card-symbol map-route-list-symbol"
          aria-hidden="true"
        >
          {routeSymbol(index)}
        </span>
        <span className="map-card-title map-route-list-title">
          <span>{title}</span>
        </span>
        <strong className="map-card-duration map-route-list-duration">
          {formatMinutes(route.duration)}
        </strong>
        <span className="map-card-accidents map-route-list-accidents">
          過去事故 <b>{route.accident_count}</b> 件
          {same && <span className="map-card-same">同一経路</span>}
        </span>
        {risk !== null ? (
          <span className="map-card-risk map-route-list-risk">{risk}</span>
        ) : (
          <span
            className="map-card-risk-placeholder map-route-list-risk"
            aria-hidden="true"
          />
        )}
      </>
    ) : (
      <>
        <span className="map-card-title">
          <span className="map-card-symbol" aria-hidden="true">
            {routeSymbol(index)}
          </span>
          <i aria-hidden="true" />
          <span>
            {extraTitle === null ? (
              title
            ) : (
              <>
                <span className="map-card-title-name">{label}</span>
                <span className="map-card-title-extra">{extraTitle}</span>
              </>
            )}
          </span>
        </span>
        <strong className="map-card-duration">
          {formatMinutes(route.duration)}
        </strong>
        <span className="map-card-accidents">
          過去事故 <b>{route.accident_count}</b> 件
          {same && <span className="map-card-same">同一経路</span>}
        </span>
        {risk !== null && <span className="map-card-risk">{risk}</span>}
      </>
    );
    return (
      <>
        <button
          type="button"
          className="map-card-select"
          aria-pressed={selectedRouteId === route.id}
          onClick={() => onSelectRoute(route.id)}
          onFocus={() => onHoverRoute(route.id)}
          onBlur={() => onHoverRoute(null)}
        >
          {content}
        </button>
        <button
          type="button"
          className="map-card-navigate"
          aria-label={`${label}でナビを開始`}
          onClick={() => onNavigate(route.id)}
          onFocus={() => onHoverRoute(route.id)}
          onBlur={() => onHoverRoute(null)}
        >
          <Send size={18} aria-hidden="true" />
        </button>
      </>
    );
  };

  return (
    <div
      className={`map-route-callouts${compact ? " is-compact" : ""}${tight ? " is-tight" : ""}${list ? " is-list" : ""}`}
      role="group"
      aria-label="表示する経路"
    >
      {!detached && !list && (
        <svg
          className="map-callout-connectors"
          width={size.width}
          height={size.height}
          aria-hidden="true"
        >
          <defs>
            <mask id={maskId} maskUnits="userSpaceOnUse" x="0" y="0" width={size.width} height={size.height}>
              <rect width={size.width} height={size.height} fill="white" />
              {protectedPoints.map(({ x, y, radius }, index) => (
                <circle key={index} cx={x} cy={y} r={radius} fill="black" />
              ))}
            </mask>
          </defs>
          {/* 地図上の点には補助線を重ねない。カード自体はマスクしない。 */}
          <g mask={`url(#${maskId})`}>
            {cards.map(({ route, tone, anchor, x, y }) => (
              <g key={route.id} stroke={routeColors[tone]}>
                <line
                  x1={anchor.x}
                  y1={anchor.y}
                  x2={clamp(anchor.x, x, x + cardWidth)}
                  y2={clamp(anchor.y, y + 16, y + cardHeight - 16)}
                  strokeWidth="2"
                />
                <circle
                  cx={anchor.x}
                  cy={anchor.y}
                  r="5"
                  fill="white"
                  strokeWidth="2"
                />
              </g>
            ))}
          </g>
        </svg>
      )}
      {list ? (
        <div className="map-route-list" role="list">
          {cards.map(({ route, tone }, index) => (
            <div
              key={route.id}
              role="listitem"
              className={`map-route-card map-route-list-row route-${tone}${selectedRouteId === route.id ? " is-selected" : ""}${hoveredRouteId === route.id ? " is-hovered" : ""}`}
              onPointerEnter={() => onHoverRoute(route.id)}
              onPointerLeave={() => onHoverRoute(null)}
            >
              {cardContent(route, tone, index, true)}
            </div>
          ))}
        </div>
      ) : (
        cards.map(({ route, tone, x, y }, index) => (
          <div
            key={route.id}
            className={`map-route-card route-${tone}${selectedRouteId === route.id ? " is-selected" : ""}${hoveredRouteId === route.id ? " is-hovered" : ""}`}
            style={{ left: x, top: y, width: cardWidth, height: cardHeight }}
            onPointerEnter={() => onHoverRoute(route.id)}
            onPointerLeave={() => onHoverRoute(null)}
          >
            {cardContent(route, tone, index)}
          </div>
        ))
      )}
    </div>
  );
}
