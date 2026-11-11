import { ApiError } from "../api/errors";
import type {
  SafetyComment,
  SafetyKind,
  SafetyReport,
  SafetyReportCreate,
  SafetyVote,
} from "../lib/types";
import { MAX_BOUNDS_SPAN_DEG, type SafetyBounds } from "./reports";

/**
 * デモ（`VITE_API_MODE` が live 以外）用の `/api/reports` の代わり。サーバと同じ規則
 * （投票者ごとに1地点1票、口コミは新しい100件、日本国内の地点、500文字、一覧の範囲は1度以内）をこのブラウザーの
 * localStorageで再現する。利用者間では共有されない。
 */
export const DEMO_REPORTS_KEY = "accilessnavi.safety-reports.v2";
const MAX_TEXT = 500;
const MAX_COMMENTS = 100;
const MAX_LIST = 500;

type StoredReport = {
  id: string;
  lat: number;
  lon: number;
  created_at: string;
  votes: { voter: string; kind: SafetyKind }[];
  comments: SafetyComment[];
};

function read(): StoredReport[] {
  try {
    const saved: unknown = JSON.parse(
      localStorage.getItem(DEMO_REPORTS_KEY) ?? "[]",
    );
    return Array.isArray(saved) ? (saved as StoredReport[]) : [];
  } catch {
    return [];
  }
}

function write(reports: StoredReport[]) {
  try {
    localStorage.setItem(DEMO_REPORTS_KEY, JSON.stringify(reports));
  } catch {
    throw new ApiError(
      "口コミを保存できませんでした。",
      "internal_error",
      500,
    );
  }
}

function toReport(stored: StoredReport, voter: string | null): SafetyReport {
  const count = (kind: SafetyKind) =>
    stored.votes.filter((vote) => vote.kind === kind).length;
  return {
    id: stored.id,
    lat: stored.lat,
    lon: stored.lon,
    created_at: stored.created_at,
    safe_count: count("safe"),
    danger_count: count("danger"),
    my_vote: stored.votes.find((vote) => vote.voter === voter)?.kind ?? null,
    comments: stored.comments,
  };
}

function invalid(message: string): never {
  throw new ApiError(message, "invalid_request", 400);
}

function cleanText(text: string | null | undefined) {
  const trimmed = (text ?? "").trim();
  if (Array.from(trimmed).length > MAX_TEXT)
    invalid("口コミは500文字以内の文字列で指定してください。");
  return trimmed || null;
}

function newId() {
  return crypto.randomUUID().replaceAll("-", "");
}

function addVote(
  stored: StoredReport,
  voter: string,
  kind: SafetyKind,
  text: string | null,
) {
  if (stored.votes.some((vote) => vote.voter === voter))
    throw new ApiError("この地点には投票済みです。", "already_voted", 409);
  const now = new Date().toISOString();
  stored.votes.push({ voter, kind });
  if (text) {
    stored.comments.push({ id: newId(), kind, text, created_at: now });
    stored.comments.splice(0, Math.max(0, stored.comments.length - MAX_COMMENTS));
  }
}

export async function demoList(
  bounds: SafetyBounds,
  voter: string | null,
): Promise<unknown> {
  if (
    bounds.maxLat - bounds.minLat > MAX_BOUNDS_SPAN_DEG ||
    bounds.maxLon - bounds.minLon > MAX_BOUNDS_SPAN_DEG
  )
    invalid(
      "表示範囲が広すぎます。緯度・経度とも1度以内の範囲で指定してください。",
    );
  const reports = read()
    .filter(
      (report) =>
        report.lat >= bounds.minLat &&
        report.lat <= bounds.maxLat &&
        report.lon >= bounds.minLon &&
        report.lon <= bounds.maxLon,
    )
    .sort((a, b) => b.created_at.localeCompare(a.created_at))
    .slice(0, MAX_LIST)
    .map((report) => toReport(report, voter));
  return { reports };
}

export async function demoCreate(
  input: SafetyReportCreate,
  voter: string,
): Promise<unknown> {
  if (
    !(input.lat >= 20 && input.lat <= 46 && input.lon >= 122 && input.lon <= 154)
  )
    invalid(
      "地点は日本国内（北緯20〜46度、東経122〜154度）の数値で指定してください。",
    );
  const stored: StoredReport = {
    id: newId(),
    lat: input.lat,
    lon: input.lon,
    created_at: new Date().toISOString(),
    votes: [],
    comments: [],
  };
  addVote(stored, voter, input.kind, cleanText(input.text));
  write([...read(), stored]);
  return { report: toReport(stored, voter) };
}

export async function demoVote(
  id: string,
  input: SafetyVote,
  voter: string,
): Promise<unknown> {
  const reports = read();
  const stored = reports.find((report) => report.id === id);
  if (!stored)
    throw new ApiError(
      "指定の地点が見つかりません。地図を更新してください。",
      "not_found",
      404,
    );
  addVote(stored, voter, input.kind, cleanText(input.text));
  write(reports);
  return { report: toReport(stored, voter) };
}
