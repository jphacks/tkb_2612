import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CommunitySafety } from "./CommunitySafety";
import { DEMO_PLACES, findDemoPlaces } from "../api/demo";
import { REPORTS_KEY } from "../community/reports";

vi.mock("../api/client", () => ({
  isDemo: true,
  searchPlaces: (query: string) => Promise.resolve(findDemoPlaces(query)),
}));

beforeEach(() => {
  localStorage.clear();
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
  Object.defineProperty(navigator, "geolocation", {
    configurable: true,
    value: { getCurrentPosition: vi.fn() },
  });
});
afterEach(() => {
  vi.restoreAllMocks();
  Reflect.deleteProperty(navigator, "geolocation");
});

describe("community safety reports", () => {
  it.each([
    ["安全", "safe", "見通しが良い"],
    ["危険", "danger", "夜道は危ない"],
  ] as const)(
    "posts and reopens a %s review at the selected point after remount",
    async (label, kind, text) => {
      const user = userEvent.setup();
      const first = render(<CommunitySafety />);
      const map = screen.getByRole("application", {
        name: "みんなの安全情報の地図",
      });
      expect(
        screen.queryByRole("group", { name: "安全・危険" }),
      ).not.toBeInTheDocument();
      fireEvent.click(map, { clientX: 195, clientY: 320 });
      const choices = screen.getByRole("group", { name: "安全・危険" });
      expect(within(choices).getAllByRole("button")).toHaveLength(2);
      for (const icon of choices.querySelectorAll("svg")) {
        expect(icon).toHaveAttribute("width", "32");
        expect(icon).toHaveAttribute("fill", "none");
      }
      await user.click(within(choices).getByRole("button", { name: label }));
      const input = screen.getByRole("textbox", { name: "詳細情報を記入" });
      expect(input).toHaveAttribute("placeholder", "例：夜道は危ない");
      expect(input).toHaveFocus();
      await user.type(input, text);
      await user.click(screen.getByRole("button", { name: "投稿" }));
      expect(screen.getByRole("region", { name: "口コミ" })).toHaveTextContent(
        text,
      );
      const saved = JSON.parse(localStorage.getItem(REPORTS_KEY)!);
      expect(saved).toEqual([
        expect.objectContaining({
          kind,
          text,
          lat: expect.any(Number),
          lon: expect.any(Number),
          createdAt: expect.any(String),
          supportCount: 1,
          comments: [],
        }),
      ]);
      first.unmount();
      render(<CommunitySafety />);
      await user.click(
        screen.getByRole("button", { name: `${label}の口コミ：${text}` }),
      );
      expect(screen.getByRole("region", { name: "口コミ" })).toHaveTextContent(
        text,
      );
      expect(screen.getByRole("heading", { name: "詳細情報" })).toBeVisible();
      expect(
        screen.getByRole("region", { name: "口コミ" }).querySelector("time"),
      ).toHaveAttribute("datetime", saved[0].createdAt);
    },
  );

  it("uses geolocation as the map center and keeps a chosen address when an older GPS request completes", async () => {
    const user = userEvent.setup();
    let resolveLocation!: PositionCallback;
    vi.mocked(navigator.geolocation.getCurrentPosition).mockImplementation(
      (success) => {
        resolveLocation = success;
      },
    );
    render(<CommunitySafety />);
    act(() =>
      resolveLocation({
        coords: { latitude: 36.0827, longitude: 140.1115 },
      } as GeolocationPosition),
    );
    expect(screen.getByRole("combobox", { name: "表示する場所" })).toHaveValue(
      "現在地",
    );
    expect(screen.getByRole("img", { name: "現在地" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "現在地を表示" }));
    const late = resolveLocation;
    fireEvent.change(screen.getByRole("combobox", { name: "表示する場所" }), {
      target: { value: "上野" },
    });
    await user.click(
      await screen.findByRole("option", { name: DEMO_PLACES[1].label }),
    );
    act(() =>
      late({ coords: { latitude: 36, longitude: 140 } } as GeolocationPosition),
    );
    expect(screen.getByRole("combobox", { name: "表示する場所" })).toHaveValue(
      DEMO_PLACES[1].label,
    );
    expect(screen.queryByText("地点を確定しました")).not.toBeInTheDocument();
  });

  it("keeps a draft when an older GPS request completes after picking a point", async () => {
    const user = userEvent.setup();
    let resolveLocation!: PositionCallback;
    vi.mocked(navigator.geolocation.getCurrentPosition).mockImplementation(
      (success) => {
        resolveLocation = success;
      },
    );
    render(<CommunitySafety />);
    fireEvent.click(screen.getByRole("application"), {
      clientX: 195,
      clientY: 320,
    });
    await user.click(screen.getByRole("button", { name: "危険" }));
    await user.type(
      screen.getByRole("textbox", { name: "詳細情報を記入" }),
      "夜道は危ない",
    );
    act(() =>
      resolveLocation({
        coords: { latitude: 36, longitude: 140 },
      } as GeolocationPosition),
    );
    expect(screen.getByRole("textbox", { name: "詳細情報を記入" })).toHaveValue(
      "夜道は危ない",
    );
    await user.click(screen.getByRole("button", { name: "投稿" }));
    const [report] = JSON.parse(localStorage.getItem(REPORTS_KEY)!);
    expect(report.lat).toBeCloseTo(35.6812, 5);
    expect(report.lon).toBeCloseTo(139.7671, 5);
  });

  it("does not lose the typed review if browser storage cannot save it", async () => {
    const user = userEvent.setup();
    render(<CommunitySafety />);
    fireEvent.click(screen.getByRole("application"), {
      clientX: 195,
      clientY: 320,
    });
    await user.click(screen.getByRole("button", { name: "危険" }));
    await user.type(
      screen.getByRole("textbox", { name: "詳細情報を記入" }),
      "夜道は危ない",
    );
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("unavailable");
    });
    await user.click(screen.getByRole("button", { name: "投稿" }));
    expect(screen.getByRole("alert")).toHaveTextContent(
      "口コミを保存できませんでした。",
    );
    expect(screen.getByRole("textbox", { name: "詳細情報を記入" })).toHaveValue(
      "夜道は危ない",
    );
    expect(
      screen.queryByRole("button", { name: /危険の口コミ/ }),
    ).not.toBeInTheDocument();
  });

  it.each(["", "夜は暗い"])(
    "shows the recorded date with and without details: %s",
    async (text) => {
      const user = userEvent.setup();
      localStorage.setItem(
        REPORTS_KEY,
        JSON.stringify([
          {
            id: "dated",
            kind: "danger",
            text,
            lat: 35.6812,
            lon: 139.7671,
            createdAt: "2026-11-04T09:15:00.000Z",
          },
        ]),
      );
      render(<CommunitySafety />);
      await user.click(
        screen.getByRole("button", {
          name: text ? `危険の口コミ：${text}` : "危険の口コミ",
        }),
      );
      expect(screen.getByText("2026/11/04 18:15")).toBeVisible();
      expect(!!screen.queryByRole("heading", { name: "詳細情報" })).toBe(
        !!text,
      );
      expect(screen.queryByLabelText("1票")).not.toBeInTheDocument();
    },
  );

  it("counts each support once, skips optional details, and retains successive dated comments", async () => {
    const user = userEvent.setup();
    localStorage.setItem(
      REPORTS_KEY,
      JSON.stringify([
        {
          id: "supported",
          kind: "danger",
          text: "夜は暗い",
          lat: 35.6812,
          lon: 139.7671,
          createdAt: "2026-11-04T09:15:00.000Z",
        },
      ]),
    );
    const first = render(<CommunitySafety />);
    await user.click(
      screen.getByRole("button", { name: "危険の口コミ：夜は暗い" }),
    );
    expect(screen.queryByLabelText("1票")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "私もそう思う" }));
    expect(screen.getByLabelText("2票")).toBeVisible();
    expect(
      screen.getByRole("textbox", { name: "詳細情報を入力" }),
    ).toHaveFocus();
    expect(screen.getByRole("button", { name: "確定" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "スキップ" }));
    expect(JSON.parse(localStorage.getItem(REPORTS_KEY)!)[0]).toMatchObject({
      supportCount: 2,
      comments: [],
    });
    await user.click(screen.getByRole("button", { name: "私もそう思う" }));
    await user.type(
      screen.getByRole("textbox", { name: "詳細情報を入力" }),
      "歩道も狭い",
    );
    await user.click(screen.getByRole("button", { name: "確定" }));
    expect(screen.getByLabelText("3票")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "私もそう思う" }));
    await user.type(
      screen.getByRole("textbox", { name: "詳細情報を入力" }),
      "街灯が少ない",
    );
    await user.click(screen.getByRole("button", { name: "確定" }));
    first.unmount();
    render(<CommunitySafety />);
    await user.click(
      screen.getByRole("button", { name: "危険の口コミ：夜は暗い" }),
    );
    const review = screen.getByRole("region", { name: "口コミ" });
    expect(within(review).getByLabelText("4票")).toBeVisible();
    expect(
      within(review)
        .getAllByRole("listitem")
        .map((item) => item.textContent),
    ).toEqual([
      "夜は暗い",
      expect.stringContaining("歩道も狭い"),
      expect.stringContaining("街灯が少ない"),
    ]);
    const [saved] = JSON.parse(localStorage.getItem(REPORTS_KEY)!);
    expect(saved.supportCount).toBe(4);
    expect(saved.comments).toHaveLength(2);
    for (const comment of saved.comments)
      expect(Number.isFinite(Date.parse(comment.createdAt))).toBe(true);
  });

  it("keeps the vote unchanged if browser storage cannot save support", async () => {
    const user = userEvent.setup();
    localStorage.setItem(
      REPORTS_KEY,
      JSON.stringify([
        {
          id: "legacy",
          kind: "safe",
          text: "見通しが良い",
          lat: 35.6812,
          lon: 139.7671,
        },
      ]),
    );
    render(<CommunitySafety />);
    await user.click(
      screen.getByRole("button", { name: "安全の口コミ：見通しが良い" }),
    );
    expect(screen.getByText("日時不明")).toBeVisible();
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("unavailable");
    });
    await user.click(screen.getByRole("button", { name: "私もそう思う" }));
    expect(screen.getByRole("alert")).toHaveTextContent(
      "口コミを保存できませんでした。",
    );
    expect(screen.queryByLabelText("2票")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("textbox", { name: "詳細情報を入力" }),
    ).not.toBeInTheDocument();
  });
});
