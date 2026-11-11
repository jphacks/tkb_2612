import { useEffect, useState } from "react";
import type { SafetyReport } from "../lib/types";
import { listSafetyReports } from "./api";
import { isTooWide, type SafetyBounds } from "./reports";

const NONE: SafetyReport[] = [];

/**
 * 範囲内の口コミ（ナビの地図に重ねる表示専用）。範囲が無い・広すぎる（長い経路）なら空。
 * 読めなくても経路案内は続けられるので、失敗は空として扱い画面に出さない。
 */
export function useSafetyReports(bounds: SafetyBounds | null): SafetyReport[] {
  const [reports, setReports] = useState<SafetyReport[]>(NONE);
  const key =
    bounds && !isTooWide(bounds)
      ? [bounds.minLat, bounds.minLon, bounds.maxLat, bounds.maxLon].join(",")
      : "";
  useEffect(() => {
    if (!bounds || !key) {
      setReports(NONE);
      return;
    }
    const controller = new AbortController();
    listSafetyReports(bounds, controller.signal)
      .then((latest) => {
        if (!controller.signal.aborted)
          setReports((previous) =>
            previous.length || latest.length ? latest : previous,
          );
      })
      .catch(() => {
        if (!controller.signal.aborted) setReports(NONE);
      });
    return () => controller.abort();
    // 範囲は値（key）で比べる。経路の再描画ごとに読み直さない
  }, [key]);
  return reports;
}
