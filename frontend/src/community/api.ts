import { isDemo, requestJson } from "../api/client";
import { validateSafetyReport, validateSafetyReports } from "../api/validate";
import type {
  SafetyReport,
  SafetyReportCreate,
  SafetyVote,
} from "../lib/types";
import { demoCreate, demoList, demoVote } from "./demoStore";
import { storedVoterId, voterId, type SafetyBounds } from "./reports";

/** 1件の読み書きの期限（サーバの要求全体の期限35秒より長くする）。 */
const SAFETY_TIMEOUT_MS = 45_000;

const voterHeader = () => ({ "X-Voter-Id": voterId() });

/** 範囲内の地点（新しい順に最大500件）。投票したことがあれば自分の票（my_vote）も返る。 */
export async function listSafetyReports(
  bounds: SafetyBounds,
  signal?: AbortSignal,
): Promise<SafetyReport[]> {
  const viewer = storedVoterId();
  if (isDemo) return validateSafetyReports(await demoList(bounds, viewer));
  const bbox = [bounds.minLon, bounds.minLat, bounds.maxLon, bounds.maxLat]
    .map((value) => value.toFixed(5))
    .join(",");
  return requestJson(
    `/api/reports?${new URLSearchParams({ bbox })}`,
    validateSafetyReports,
    signal,
    undefined,
    SAFETY_TIMEOUT_MS,
    viewer ? { "X-Voter-Id": viewer } : {},
  );
}

/** 新しい地点に投稿する（投稿者の1票が入る）。 */
export async function createSafetyReport(
  input: SafetyReportCreate,
  signal?: AbortSignal,
): Promise<SafetyReport> {
  if (isDemo) return validateSafetyReport(await demoCreate(input, voterId()));
  return requestJson(
    "/api/reports",
    validateSafetyReport,
    signal,
    input,
    SAFETY_TIMEOUT_MS,
    voterHeader(),
  );
}

/** 既存の地点に1票（と任意の口コミ）を入れる。投票済みは409 already_voted。 */
export async function voteSafetyReport(
  id: string,
  input: SafetyVote,
  signal?: AbortSignal,
): Promise<SafetyReport> {
  if (isDemo) return validateSafetyReport(await demoVote(id, input, voterId()));
  return requestJson(
    `/api/reports/${encodeURIComponent(id)}/votes`,
    validateSafetyReport,
    signal,
    input,
    SAFETY_TIMEOUT_MS,
    voterHeader(),
  );
}
