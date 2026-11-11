import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api/errors";
import { CommunitySafety, RELOAD_DELAY_MS, TOO_WIDE } from "./CommunitySafety";
import { listSafetyReports, voteSafetyReport } from "../community/api";
import { isTooWide, type SafetyReport } from "../community/reports";

vi.mock("../api/client", () => ({
  isDemo: true,
  searchPlaces: () => Promise.resolve({ results: [], attribution: [] }),
}));
vi.mock("../community/api", () => ({
  listSafetyReports: vi.fn(),
  createSafetyReport: vi.fn(),
  voteSafetyReport: vi.fn(),
}));

beforeEach(() => {
  vi.mocked(listSafetyReports).mockReset();
  vi.mocked(voteSafetyReport).mockReset();
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
    width: 390,
    height: 640,
    top: 0,
    left: 0,
    right: 390,
    bottom: 640,
    x: 0,
    y: 0,
    toJSON: () => ({}),
  });
});
afterEach(() => {
  vi.restoreAllMocks();
});

describe("loading community reports", () => {
  it("asks for the visible area and shows why loading failed", async () => {
    vi.mocked(listSafetyReports).mockRejectedValue(
      new ApiError(
        "混み合っています。時間をおいて再試行してください。",
        "service_busy",
        503,
      ),
    );
    render(<CommunitySafety />);
    expect(await screen.findByRole("status")).toHaveTextContent(
      "混み合っています。",
    );
    const [bounds] = vi.mocked(listSafetyReports).mock.lastCall!;
    expect(bounds.minLat).toBeLessThan(35.6812);
    expect(bounds.maxLat).toBeGreaterThan(35.6812);
    expect(bounds.minLon).toBeLessThan(139.7671);
    expect(bounds.maxLon).toBeGreaterThan(139.7671);
  });

  it("asks to zoom in instead of loading an area wider than one degree", async () => {
    vi.mocked(listSafetyReports).mockResolvedValue([]);
    const user = userEvent.setup();
    render(<CommunitySafety />);
    await waitFor(() =>
      expect(vi.mocked(listSafetyReports)).toHaveBeenCalledTimes(1),
    );
    // 幅390pxの地図はズーム9で経度1度を超える
    for (let i = 0; i < 5; i++)
      await user.click(screen.getByRole("button", { name: "地図を縮小" }));
    expect(await screen.findByRole("status")).toHaveTextContent(TOO_WIDE);
    await new Promise((resolve) => setTimeout(resolve, RELOAD_DELAY_MS + 100));
    for (const [bounds] of vi.mocked(listSafetyReports).mock.calls)
      expect(isTooWide(bounds)).toBe(false);
    await user.click(screen.getByRole("button", { name: "地図を拡大" }));
    await waitFor(() =>
      expect(screen.queryByRole("status")).not.toBeInTheDocument(),
    );
  });

  it("does not let a list read from before a vote overwrite the vote", async () => {
    const before: SafetyReport = {
      id: "a".repeat(32),
      lat: 35.6812,
      lon: 139.7671,
      created_at: "2026-11-04T09:15:00.000Z",
      safe_count: 0,
      danger_count: 1,
      my_vote: null,
      comments: [
        {
          id: "b".repeat(32),
          kind: "danger",
          text: "夜は暗い",
          created_at: "2026-11-04T09:15:00.000Z",
        },
      ],
    };
    const after: SafetyReport = { ...before, safe_count: 1, my_vote: "safe" };
    let resolveStale: (reports: SafetyReport[]) => void = () => {};
    vi.mocked(listSafetyReports)
      .mockResolvedValueOnce([before])
      .mockImplementationOnce(
        () => new Promise((resolve) => (resolveStale = resolve)),
      )
      .mockResolvedValue([after]);
    vi.mocked(voteSafetyReport).mockResolvedValue(after);
    const user = userEvent.setup();
    render(<CommunitySafety />);
    await user.click(
      await screen.findByRole("button", {
        name: "安全0票・危険1票の地点：夜は暗い",
      }),
    );
    // 画面に戻って読み直し始めたが、応答はまだ届かない
    fireEvent.focus(window);
    await waitFor(() =>
      expect(vi.mocked(listSafetyReports)).toHaveBeenCalledTimes(2),
    );
    await user.click(screen.getByRole("button", { name: "安全に投票" }));
    await user.click(screen.getByRole("button", { name: "スキップ" }));
    const review = await screen.findByRole("region", { name: "口コミ" });
    expect(within(review).getByLabelText("安全1票・危険1票")).toBeVisible();
    // 投票前の一覧が遅れて届いても、投票後の票のまま
    await act(async () => resolveStale([before]));
    expect(within(review).getByLabelText("安全1票・危険1票")).toBeVisible();
    await waitFor(() =>
      expect(vi.mocked(listSafetyReports)).toHaveBeenCalledTimes(3),
    );
  });
});
