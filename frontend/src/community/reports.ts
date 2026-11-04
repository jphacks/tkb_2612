import type { LatLon } from "../lib/types";
import type { SafetyKind } from "../components/icons/SafetyIcon";

export type SafetyReport = LatLon & {
  id: string;
  kind: SafetyKind;
  text: string;
};
export const REPORTS_KEY = "accilessnavi.safety-reports.v1";

export function readReports(): SafetyReport[] {
  try {
    const saved: unknown = JSON.parse(
      localStorage.getItem(REPORTS_KEY) ?? "[]",
    );
    if (!Array.isArray(saved)) return [];
    return saved.filter(
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
    );
  } catch {
    return [];
  }
}

export function saveReports(reports: SafetyReport[]) {
  localStorage.setItem(REPORTS_KEY, JSON.stringify(reports));
}
