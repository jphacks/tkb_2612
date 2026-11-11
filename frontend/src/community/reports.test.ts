import { afterEach, describe, expect, it, vi } from "vitest";
import {
  boundsAround,
  dominantKind,
  isTooWide,
  storedVoterId,
  tallyLabel,
  voterId,
  VOTER_KEY,
  type SafetyReport,
} from "./reports";
import { demoCreate, demoList, demoVote } from "./demoStore";

const base: SafetyReport = {
  id: "0".repeat(32),
  lat: 35,
  lon: 139,
  created_at: "2026-11-04T09:15:00.000Z",
  safe_count: 1,
  danger_count: 1,
  my_vote: null,
  comments: [],
};
const TOKYO = { minLat: 35.6, minLon: 139.7, maxLat: 35.7, maxLon: 139.8 };

afterEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

describe("community report helpers", () => {
  it("colors a tie as danger and labels both tallies", () => {
    expect(dominantKind(base)).toBe("danger");
    expect(dominantKind({ ...base, safe_count: 2 })).toBe("safe");
    expect(tallyLabel(base)).toBe("安全1票・危険1票");
  });

  it("widens the bounds of route coordinates by a margin", () => {
    expect(boundsAround([], 0.1)).toBeNull();
    const bounds = boundsAround(
      [
        [139.7, 35.6],
        [139.8, 35.7],
      ],
      0.01,
    )!;
    expect(bounds.minLon).toBeCloseTo(139.69);
    expect(bounds.maxLat).toBeCloseTo(35.71);
  });

  it("treats bounds wider than one degree (with rounding room) as too wide", () => {
    expect(isTooWide(TOKYO)).toBe(false);
    expect(
      isTooWide({ minLat: 35, minLon: 139, maxLat: 35.9999, maxLon: 139.5 }),
    ).toBe(false);
    expect(isTooWide({ minLat: 35, minLon: 139, maxLat: 36, maxLon: 139.5 })).toBe(
      true,
    );
    expect(isTooWide({ minLat: 35, minLon: 139, maxLat: 35.5, maxLon: 140.2 })).toBe(
      true,
    );
  });

  it("keeps one anonymous voter id per browser", () => {
    const id = voterId();
    expect(id).toMatch(/^[A-Za-z0-9_-]{16,64}$/);
    expect(voterId()).toBe(id);
    expect(localStorage.getItem(VOTER_KEY)).toBe(id);
  });

  it("falls back to a per-tab id when storage is unavailable", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    const id = voterId();
    expect(id).toMatch(/^[A-Za-z0-9_-]{16,64}$/);
    expect(voterId()).toBe(id);
  });

  it("keeps the per-tab id when storage is readable but not writable", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("quota exceeded");
    });
    const id = voterId();
    expect(localStorage.getItem(VOTER_KEY)).toBeNull();
    expect(voterId()).toBe(id);
    expect(storedVoterId()).toBe(id);
  });
});

describe("demo store", () => {
  it("follows the server rules for one vote per voter and point", async () => {
    const created = (await demoCreate(
      { lat: 35.65, lon: 139.75, kind: "danger", text: "  暗い  " },
      "voter-a-000000000000",
    )) as { report: SafetyReport };
    expect(created.report).toMatchObject({
      safe_count: 0,
      danger_count: 1,
      my_vote: "danger",
      comments: [{ kind: "danger", text: "暗い" }],
    });
    const id = created.report.id;
    await expect(
      demoVote(id, { kind: "safe" }, "voter-a-000000000000"),
    ).rejects.toMatchObject({ code: "already_voted", status: 409 });
    await demoVote(id, { kind: "safe", text: " " }, "voter-b-000000000000");
    const { reports } = (await demoList(TOKYO, "voter-b-000000000000")) as {
      reports: SafetyReport[];
    };
    expect(reports).toHaveLength(1);
    expect(reports[0]).toMatchObject({ safe_count: 1, danger_count: 1, my_vote: "safe" });
    expect(reports[0].comments).toHaveLength(1);
    await expect(
      demoVote("f".repeat(32), { kind: "safe" }, "voter-b-000000000000"),
    ).rejects.toMatchObject({ code: "not_found" });
  });

  it("rejects listing an area wider than one degree like the server", async () => {
    await expect(
      demoList({ minLat: 35, minLon: 139, maxLat: 35.5, maxLon: 140.5 }, null),
    ).rejects.toMatchObject({ code: "invalid_request", status: 400 });
    await expect(
      demoList({ minLat: 35, minLon: 139, maxLat: 36, maxLon: 140 }, null),
    ).resolves.toEqual({ reports: [] });
  });

  it("rejects points outside Japan and overlong comments", async () => {
    await expect(
      demoCreate({ lat: 10, lon: 139, kind: "safe" }, "voter-a-000000000000"),
    ).rejects.toMatchObject({ code: "invalid_request" });
    await expect(
      demoCreate(
        { lat: 35.65, lon: 139.75, kind: "safe", text: "あ".repeat(501) },
        "voter-a-000000000000",
      ),
    ).rejects.toMatchObject({ code: "invalid_request" });
  });
});
