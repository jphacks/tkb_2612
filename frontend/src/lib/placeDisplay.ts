import type { Place } from "./types";

const normalize = (value: string) => value.normalize("NFKC").trim();

/**
 * 候補一覧に所在地を添えるか決める。
 * 同名の地点が複数ある場合と、検索語が施設名に含まれない場合だけ表示する。
 */
export function visibleDetail(
  place: Place,
  candidates: readonly Place[],
  query: string,
): string | null {
  if (place.detail == null) return null;
  const label = normalize(place.label);
  const sameLabelCount = candidates.filter(
    (candidate) => normalize(candidate.label) === label,
  ).length;
  if (sameLabelCount >= 2) return place.detail;
  return label.includes(normalize(query)) ? null : place.detail;
}
