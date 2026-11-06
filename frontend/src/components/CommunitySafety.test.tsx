import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CommunitySafety } from "./CommunitySafety";
import { DEMO_PLACES, findDemoPlaces } from "../api/demo";
import {
  pointKey,
  readSessionVotes,
  REPORTS_KEY,
  SESSION_VOTES_KEY,
} from "../community/reports";

vi.mock("../api/client", () => ({
  isDemo: true,
  searchPlaces: (query: string) => Promise.resolve(findDemoPlaces(query)),
}));

beforeEach(() => {
  localStorage.clear();
  sessionStorage.clear();
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

function seed(text = "夜は暗い") {
  localStorage.setItem(
    REPORTS_KEY,
    JSON.stringify([
      {
        id: "recorded",
        kind: "danger",
        text,
        lat: 35.6812,
        lon: 139.7671,
        createdAt: "2026-11-04T09:15:00.000Z",
      },
    ]),
  );
}
function pick() {
  fireEvent.click(screen.getByRole("application"), {
    clientX: 195,
    clientY: 320,
  });
}
async function beginPost(
  user: ReturnType<typeof userEvent.setup>,
  label = "危険",
) {
  pick();
  await user.click(screen.getByRole("button", { name: label }));
  expect(
    screen.queryByRole("textbox", { name: "詳細情報を入力" }),
  ).not.toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "投票" }));
  const input = screen.getByRole("textbox", { name: "詳細情報を入力" });
  expect(input).toHaveFocus();
  return input;
}

describe("community safety reports", () => {
  it.each([
    ["安全", "safe", "見通しが良い"],
    ["危険", "danger", "夜道は危ない"],
  ] as const)(
    "selects an icon before opening details, then saves and reopens a %s post",
    async (label, kind, text) => {
      const user = userEvent.setup();
      const first = render(<CommunitySafety />);
      expect(
        screen.queryByRole("group", { name: "安全・危険" }),
      ).not.toBeInTheDocument();
      pick();
      const choices = screen.getByRole("group", { name: "安全・危険" });
      expect(within(choices).getAllByRole("button")).toHaveLength(2);
      for (const icon of choices.querySelectorAll("svg")) {
        expect(icon).toHaveAttribute("width", "32");
        expect(icon).toHaveAttribute("fill", "none");
      }
      await user.click(within(choices).getByRole("button", { name: label }));
      expect(within(choices).getAllByRole("button")).toHaveLength(2);
      expect(
        screen.queryByRole("button", { name: "投稿を閉じる" }),
      ).not.toBeInTheDocument();
      expect(
        screen.queryByRole("textbox", { name: "詳細情報を入力" }),
      ).not.toBeInTheDocument();
      await user.click(screen.getByRole("button", { name: "投票" }));
      const input = screen.getByRole("textbox", { name: "詳細情報を入力" });
      expect(input).toHaveAttribute(
        "placeholder",
        "例：夜は見通しが悪くて危険",
      );
      await user.type(input, text);
      await user.click(screen.getByRole("button", { name: "確定" }));
      expect(screen.getByRole("region", { name: "口コミ" })).toHaveTextContent(
        text,
      );
      expect(screen.getByRole("button", { name: "投票" })).toHaveAttribute(
        "aria-disabled",
        "true",
      );
      const [saved] = JSON.parse(localStorage.getItem(REPORTS_KEY)!);
      expect(saved).toMatchObject({
        kind,
        text,
        supportCount: 1,
        comments: [],
        createdAt: expect.any(String),
      });
      expect(readSessionVotes().has(pointKey(saved))).toBe(true);
      first.unmount();
      render(<CommunitySafety />);
      await user.click(
        screen.getByRole("button", { name: `${label}の口コミ：${text}` }),
      );
      const review = screen.getByRole("region", { name: "口コミ" });
      expect(review).toHaveTextContent(text);
      expect(review.querySelector("time")).toHaveAttribute(
        "datetime",
        saved.createdAt,
      );
      expect(within(review).getByRole("listitem").querySelector("time")).toHaveAttribute("datetime", saved.createdAt);
      expect(screen.getByRole("button", { name: "投票" })).toHaveAttribute(
        "aria-disabled",
        "true",
      );
    },
  );

  it("posts without details through Skip and opens the existing point instead of creating a duplicate", async () => {
    const user = userEvent.setup();
    render(<CommunitySafety />);
    await beginPost(user);
    await user.click(screen.getByRole("button", { name: "スキップ" }));
    expect(screen.getByText("記入日時")).toBeVisible();
    expect(
      screen.queryByRole("heading", { name: "詳細情報" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "投票" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await user.click(screen.getByRole("button", { name: "口コミを閉じる" }));
    pick();
    expect(screen.getByRole("region", { name: "口コミ" })).toBeVisible();
    expect(
      screen.queryByRole("group", { name: "安全・危険" }),
    ).not.toBeInTheDocument();
    expect(JSON.parse(localStorage.getItem(REPORTS_KEY)!)).toHaveLength(1);
  });

  it("keeps a chosen address when an older GPS request completes", async () => {
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
    const input = await beginPost(user);
    await user.type(input, "夜道は危ない");
    act(() =>
      resolveLocation({
        coords: { latitude: 36, longitude: 140 },
      } as GeolocationPosition),
    );
    expect(input).toHaveValue("夜道は危ない");
    await user.click(screen.getByRole("button", { name: "確定" }));
    const [saved] = JSON.parse(localStorage.getItem(REPORTS_KEY)!);
    expect(saved.lat).toBeCloseTo(35.6812, 5);
    expect(saved.lon).toBeCloseTo(139.7671, 5);
  });

  it("keeps the draft and leaves the vote unrecorded if storage cannot save the post", async () => {
    const user = userEvent.setup();
    render(<CommunitySafety />);
    const input = await beginPost(user);
    await user.type(input, "夜道は危ない");
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("unavailable");
    });
    await user.click(screen.getByRole("button", { name: "確定" }));
    expect(screen.getByRole("alert")).toHaveTextContent(
      "口コミを保存できませんでした。",
    );
    expect(input).toHaveValue("夜道は危ない");
    expect(
      screen.queryByRole("button", { name: /危険の口コミ/ }),
    ).not.toBeInTheDocument();
  });

  it.each(["", "夜は暗い"])(
    "shows a date whether details exist or not: %s",
    async (text) => {
      const user = userEvent.setup();
      seed(text);
      render(<CommunitySafety />);
      await user.click(
        screen.getByRole("button", {
          name: text ? `危険の口コミ：${text}` : "危険の口コミ",
        }),
      );
      expect(screen.getAllByText("2026/11/04 18:15")).toHaveLength(text ? 2 : 1);
      expect(!!screen.queryByRole("heading", { name: "詳細情報" })).toBe(
        !!text,
      );
      expect(screen.queryByLabelText("1票")).not.toBeInTheDocument();
    },
  );

  it("counts once per session and keeps Skip as a vote without a comment across remounts", async () => {
    const user = userEvent.setup();
    seed();
    const first = render(<CommunitySafety />);
    await user.click(
      screen.getByRole("button", { name: "危険の口コミ：夜は暗い" }),
    );
    await user.click(screen.getByRole("button", { name: "投票" }));
    expect(screen.getByLabelText("2票")).toBeVisible();
    expect(screen.getByRole("button", { name: "確定" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "スキップ" }));
    await user.click(screen.getByRole("button", { name: "投票" }));
    expect(JSON.parse(localStorage.getItem(REPORTS_KEY)!)[0]).toMatchObject({
      supportCount: 2,
      comments: [],
    });
    first.unmount();
    render(<CommunitySafety />);
    await user.click(
      screen.getByRole("button", { name: "危険の口コミ：夜は暗い" }),
    );
    expect(screen.getByRole("button", { name: "投票" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await user.click(screen.getByRole("button", { name: "投票" }));
    expect(
      within(screen.getByRole("region", { name: "口コミ" })).getByLabelText(
        "2票",
      ),
    ).toBeVisible();
    expect(JSON.parse(localStorage.getItem(REPORTS_KEY)!)[0].supportCount).toBe(
      2,
    );
  });

  it("allows one agreement at each of two different points in the same session", async () => {
    const user = userEvent.setup();
    seed();
    const [first] = JSON.parse(localStorage.getItem(REPORTS_KEY)!);
    localStorage.setItem(
      REPORTS_KEY,
      JSON.stringify([
        first,
        { ...first, id: "other", text: "別の地点", lat: first.lat + 0.01 },
      ]),
    );
    render(<CommunitySafety />);
    for (const text of ["夜は暗い", "別の地点"]) {
      await user.click(
        screen.getByRole("button", { name: `危険の口コミ：${text}` }),
      );
      expect(screen.getByRole("button", { name: "投票" })).toHaveAttribute(
        "aria-disabled",
        "false",
      );
      await user.click(screen.getByRole("button", { name: "投票" }));
      await user.click(screen.getByRole("button", { name: "スキップ" }));
      expect(screen.getByRole("button", { name: "投票" })).toHaveAttribute(
        "aria-disabled",
        "true",
      );
      await user.click(screen.getByRole("button", { name: "口コミを閉じる" }));
    }
    expect(
      JSON.parse(localStorage.getItem(REPORTS_KEY)!).map(
        (report: { supportCount: number }) => report.supportCount,
      ),
    ).toEqual([2, 2]);
  });

  it("retains successive dated comments from different sessions without replacing earlier details", async () => {
    const user = userEvent.setup();
    seed();
    let view = render(<CommunitySafety />);
    for (const text of ["歩道も狭い", "街灯が少ない"]) {
      await user.click(
        screen.getByRole("button", { name: "危険の口コミ：夜は暗い" }),
      );
      await user.click(screen.getByRole("button", { name: "投票" }));
      await user.type(
        screen.getByRole("textbox", { name: "詳細情報を入力" }),
        text,
      );
      await user.click(screen.getByRole("button", { name: "確定" }));
      expect(screen.getByRole("button", { name: "投票" })).toHaveAttribute(
        "aria-disabled",
        "true",
      );
      view.unmount();
      sessionStorage.clear();
      view = render(<CommunitySafety />);
    }
    await user.click(
      screen.getByRole("button", { name: "危険の口コミ：夜は暗い" }),
    );
    const review = screen.getByRole("region", { name: "口コミ" });
    expect(within(review).getByLabelText("3票")).toBeVisible();
    expect(
      within(review)
        .getAllByRole("listitem")
        .map((item) => item.querySelector("p")?.textContent),
    ).toEqual([
      "夜は暗い",
      expect.stringContaining("歩道も狭い"),
      expect.stringContaining("街灯が少ない"),
    ]);
    const [saved] = JSON.parse(localStorage.getItem(REPORTS_KEY)!);
    expect(saved.comments).toHaveLength(2);
    for (const comment of saved.comments)
      expect(Number.isFinite(Date.parse(comment.createdAt))).toBe(true);
  });

  it("explains the voting restriction on hover, focus, and click without casting another vote", async () => {
    const user = userEvent.setup();
    seed();
    const savedBeforeVote = localStorage.getItem(REPORTS_KEY);
    sessionStorage.setItem(
      SESSION_VOTES_KEY,
      JSON.stringify([pointKey({ lat: 35.6812, lon: 139.7671 })]),
    );
    render(<CommunitySafety />);
    await user.click(
      screen.getByRole("button", { name: "危険の口コミ：夜は暗い" }),
    );
    const vote = screen.getByRole("button", { name: "投票" });
    expect(vote).toHaveAttribute("aria-disabled", "true");
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    await user.hover(vote);
    expect(screen.getByRole("tooltip")).toHaveTextContent(
      "※ 同一地点には一度しか入力できません",
    );
    await user.unhover(vote);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    fireEvent.focus(vote);
    expect(screen.getByRole("tooltip")).toHaveTextContent(
      "※ 同一地点には一度しか入力できません",
    );
    fireEvent.blur(vote);
    await user.click(vote);
    await user.unhover(vote);
    fireEvent.blur(vote);
    expect(screen.getByRole("tooltip")).toHaveTextContent(
      "※ 同一地点には一度しか入力できません",
    );
    expect(
      screen.queryByRole("textbox", { name: "詳細情報を入力" }),
    ).not.toBeInTheDocument();
    expect(localStorage.getItem(REPORTS_KEY)).toBe(savedBeforeVote);
    fireEvent.keyDown(vote, { key: "Escape" });
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });

  it("rolls back the session marker when saving an agreement fails", async () => {
    const user = userEvent.setup();
    seed();
    const savedBeforeVote = localStorage.getItem(REPORTS_KEY);
    render(<CommunitySafety />);
    await user.click(
      screen.getByRole("button", { name: "危険の口コミ：夜は暗い" }),
    );
    const original = Storage.prototype.setItem;
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(function (
      this: Storage,
      key,
      value,
    ) {
      if (this === localStorage) throw new Error("unavailable");
      original.call(this, key, value);
    });
    await user.click(screen.getByRole("button", { name: "投票" }));
    expect(screen.getByRole("alert")).toHaveTextContent(
      "口コミを保存できませんでした。",
    );
    expect(screen.queryByLabelText("2票")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "投票" })).toHaveAttribute(
      "aria-disabled",
      "false",
    );
    expect(sessionStorage.getItem(SESSION_VOTES_KEY)).toBeNull();
    expect(localStorage.getItem(REPORTS_KEY)).toBe(savedBeforeVote);
  });
});
