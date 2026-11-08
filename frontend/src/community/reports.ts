import type { LatLon } from "../lib/types";
import type { SafetyKind } from "../components/icons/SafetyIcon";

export type SafetyComment = {
  id: string;
  text: string;
  createdAt: string | null;
};
export type SafetyReport = LatLon & {
  id: string;
  kind: SafetyKind;
  text: string;
  createdAt: string | null;
  supportCount: number;
  comments: SafetyComment[];
};
export const REPORTS_KEY = "accilessnavi.safety-reports.v1";
export const SESSION_VOTES_KEY = "accilessnavi.safety-votes.v1";

export function pointKey(point: LatLon) {
  return `${point.lat.toFixed(6)},${point.lon.toFixed(6)}`;
}

export function readSessionVotes(): Set<string> {
  try {
    const saved: unknown = JSON.parse(
      sessionStorage.getItem(SESSION_VOTES_KEY) ?? "[]",
    );
    return new Set(
      Array.isArray(saved)
        ? saved.filter((key): key is string => typeof key === "string")
        : [],
    );
  } catch {
    return new Set();
  }
}

/** Store the session marker first; restore it if saving the report fails. */
export function saveReportsWithVote(
  reports: SafetyReport[],
  point: LatLon,
): Set<string> {
  const votes = readSessionVotes();
  const key = pointKey(point);
  if (votes.has(key)) throw new Error("この地点には投票済みです。");
  const previous = sessionStorage.getItem(SESSION_VOTES_KEY);
  votes.add(key);
  sessionStorage.setItem(SESSION_VOTES_KEY, JSON.stringify([...votes]));
  try {
    saveReports(reports);
  } catch (error) {
    if (previous === null) sessionStorage.removeItem(SESSION_VOTES_KEY);
    else sessionStorage.setItem(SESSION_VOTES_KEY, previous);
    throw error;
  }
  return votes;
}

export function readReports(): SafetyReport[] {
  try {
    const saved: unknown = JSON.parse(
      localStorage.getItem(REPORTS_KEY) ?? "[]",
    );
    if (!Array.isArray(saved)) return [];
    return saved
      .filter(
        (report): report is SafetyReport =>
          !!report &&
          typeof report === "object" &&
          typeof report.id === "string" &&
          (report.kind === "safe" || report.kind === "danger") &&
          typeof report.text === "string" &&
          report.text.length <= 2000 &&
          Number.isFinite(report.lat) &&
          Math.abs(report.lat) <= 90 &&
          Number.isFinite(report.lon) &&
          Math.abs(report.lon) <= 180,
      )
      .map((report) => ({
        ...report,
        createdAt:
          typeof report.createdAt === "string" &&
          Number.isFinite(Date.parse(report.createdAt))
            ? report.createdAt
            : null,
        supportCount:
          Number.isSafeInteger(report.supportCount) && report.supportCount >= 1
            ? report.supportCount
            : 1,
        comments: Array.isArray(report.comments)
          ? report.comments
              .filter(
                (comment) =>
                  !!comment &&
                  typeof comment === "object" &&
                  typeof comment.id === "string" &&
                  typeof comment.text === "string" &&
                  comment.text.length > 0 &&
                  comment.text.length <= 2000,
              )
              .map((comment) => ({
                id: comment.id,
                text: comment.text,
                createdAt:
                  typeof comment.createdAt === "string" &&
                  Number.isFinite(Date.parse(comment.createdAt))
                    ? comment.createdAt
                    : null,
              }))
          : [],
      }));
  } catch {
    return [];
  }
}

export function saveReports(reports: SafetyReport[]) {
  localStorage.setItem(REPORTS_KEY, JSON.stringify(reports));
}
