import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CommunitySafety } from "./CommunitySafety";
import { DEMO_PLACES, findDemoPlaces } from "../api/demo";
import { VOTER_KEY } from "../community/reports";
import { DEMO_REPORTS_KEY } from "../community/demoStore";

// デモ（isDemo）では /api/reports の代わりに demoStore がサーバと同じ規則で動く。
vi.mock("../api/client", () => ({
  isDemo: true,
  searchPlaces: (query: string) => Promise.resolve(findDemoPlaces(query)),
}));

const SEEDED_ID = "a".repeat(32);
const OTHER_BROWSER = "other-browser-000000000001";

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

type StoredVote = { voter: string; kind: "safe" | "danger" };

/** 別のブラウザーが危険に1票（口コミ付き）入れた地点を地図の中心に置く。 */
function seed(text = "夜は暗い", votes: StoredVote[] = [{ voter: "seed-voter-000000001", kind: "danger" }]) {
  localStorage.setItem(
    DEMO_REPORTS_KEY,
    JSON.stringify([
      {
        id: SEEDED_ID,
        lat: 35.6812,
        lon: 139.7671,
        created_at: "2026-11-04T09:15:00.000Z",
        votes,
        comments: text
          ? [
              {
                id: "c1",
                kind: "danger",
                text,
                created_at: "2026-11-04T09:15:00.000Z",
              },
            ]
          : [],
      },
    ]),
  );
}
function stored() {
  return JSON.parse(localStorage.getItem(DEMO_REPORTS_KEY) ?? "[]");
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
const voteGroup = () => screen.getByRole("group", { name: "この地点に投票" });
async function openSeeded(
  user: ReturnType<typeof userEvent.setup>,
  name = "安全0票・危険1票の地点：夜は暗い",
) {
  await user.click(await screen.findByRole("button", { name }));
  return screen.getByRole("region", { name: "口コミ" });
}

describe("community safety reports", () => {
  it.each([
    ["安全", "safe", "見通しが良い", "安全1票・危険0票"],
    ["危険", "danger", "夜道は危ない", "安全0票・危険1票"],
  ] as const)(
    "selects an icon before opening details, then saves and reopens a %s post",
    async (label, kind, text, tally) => {
      const user = userEvent.setup();
      const first = render(<CommunitySafety />);
      expect(
        screen.queryByRole("group", { name: "安全・危険" }),
      ).not.toBeInTheDocument();
      pick();
      const choices = screen.getByRole("group", { name: "安全・危険" });
      expect(within(choices).getAllByRole("button")).toHaveLength(2);
      await user.click(within(choices).getByRole("button", { name: label }));
      await user.click(screen.getByRole("button", { name: "投票" }));
      const input = screen.getByRole("textbox", { name: "詳細情報を入力" });
      expect(input).toHaveAttribute("maxlength", "500");
      await user.type(input, text);
      await user.click(screen.getByRole("button", { name: "確定" }));
      const review = await screen.findByRole("region", { name: "口コミ" });
      expect(review).toHaveTextContent(text);
      expect(within(review).getByLabelText(tally)).toBeVisible();
      for (const button of within(voteGroup()).getAllByRole("button"))
        expect(button).toHaveAttribute("aria-disabled", "true");

      const [saved] = stored();
      expect(saved.votes).toEqual([
        { voter: localStorage.getItem(VOTER_KEY), kind },
      ]);
      expect(saved.comments).toMatchObject([{ kind, text }]);
      first.unmount();

      // 読み直しても同じ地点が出て、このブラウザーは投票済みのまま
      render(<CommunitySafety />);
      const reopened = await openSeeded(user, `${tally}の地点：${text}`);
      expect(reopened.querySelector("time")).toHaveAttribute(
        "datetime",
        saved.created_at,
      );
      expect(
        within(voteGroup()).getByRole("button", { name: `${label}に投票` }),
      ).toHaveAttribute("aria-pressed", "true");
    },
  );

  it("posts without details through Skip and opens the existing point instead of creating a duplicate", async () => {
    const user = userEvent.setup();
    render(<CommunitySafety />);
    await beginPost(user);
    await user.click(screen.getByRole("button", { name: "スキップ" }));
    expect(await screen.findByText("記入日時")).toBeVisible();
    expect(
      screen.queryByRole("heading", { name: "詳細情報" }),
    ).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "口コミを閉じる" }));
    pick();
    expect(screen.getByRole("region", { name: "口コミ" })).toBeVisible();
    expect(
      screen.queryByRole("group", { name: "安全・危険" }),
    ).not.toBeInTheDocument();
    expect(stored()).toHaveLength(1);
    expect(stored()[0].comments).toEqual([]);
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
    await screen.findByRole("region", { name: "口コミ" });
    const [saved] = stored();
    expect(saved.lat).toBeCloseTo(35.6812, 5);
    expect(saved.lon).toBeCloseTo(139.7671, 5);
  });

  it("keeps the draft and shows the error if the post cannot be saved", async () => {
    const user = userEvent.setup();
    render(<CommunitySafety />);
    const input = await beginPost(user);
    await user.type(input, "夜道は危ない");
    const setItem = Storage.prototype.setItem;
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(function (
      this: Storage,
      key: string,
      value: string,
    ) {
      if (key === DEMO_REPORTS_KEY) throw new Error("unavailable");
      setItem.call(this, key, value);
    });
    await user.click(screen.getByRole("button", { name: "確定" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "口コミを保存できませんでした。",
    );
    expect(input).toHaveValue("夜道は危ない");
    expect(screen.queryByRole("region", { name: "口コミ" })).not.toBeInTheDocument();
    expect(stored()).toEqual([]);
  });

  it.each(["", "夜は暗い"])(
    "shows a date whether details exist or not: %s",
    async (text) => {
      const user = userEvent.setup();
      seed(text);
      render(<CommunitySafety />);
      await openSeeded(
        user,
        text ? `安全0票・危険1票の地点：${text}` : "安全0票・危険1票の地点",
      );
      expect(screen.getAllByText("2026/11/04 18:15")).toHaveLength(text ? 2 : 1);
      expect(!!screen.queryByRole("heading", { name: "詳細情報" })).toBe(
        !!text,
      );
    },
  );

  it("counts safe and danger votes separately and appends a dated comment with its kind", async () => {
    const user = userEvent.setup();
    seed();
    render(<CommunitySafety />);
    await openSeeded(user);
    await user.click(
      within(voteGroup()).getByRole("button", { name: "安全に投票" }),
    );
    const input = screen.getByRole("textbox", { name: "詳細情報を入力" });
    await user.type(input, "昼は人通りが多い");
    await user.click(screen.getByRole("button", { name: "確定" }));
    const review = await screen.findByRole("region", { name: "口コミ" });
    expect(within(review).getByLabelText("安全1票・危険1票")).toBeVisible();
    const items = within(review).getAllByRole("listitem");
    expect(items.map((item) => item.querySelector("p")?.textContent)).toEqual([
      "夜は暗い",
      "昼は人通りが多い",
    ]);
    expect(items[1]).toHaveTextContent("安全");
    expect(
      within(voteGroup()).getByRole("button", { name: "安全に投票" }),
    ).toHaveAttribute("aria-pressed", "true");
    const [saved] = stored();
    expect(saved.votes.map((vote: StoredVote) => vote.kind)).toEqual([
      "danger",
      "safe",
    ]);
    expect(Number.isFinite(Date.parse(saved.comments[1].created_at))).toBe(true);
  });

  it("keeps Skip as a vote without a comment and blocks a second vote after remounting", async () => {
    const user = userEvent.setup();
    seed();
    const first = render(<CommunitySafety />);
    await openSeeded(user);
    await user.click(
      within(voteGroup()).getByRole("button", { name: "危険に投票" }),
    );
    // 入れた後の危険の票数をフォームで示す
    expect(screen.getByLabelText("2票")).toBeVisible();
    expect(screen.getByRole("button", { name: "確定" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "スキップ" }));
    expect(
      await screen.findByLabelText("安全0票・危険2票"),
    ).toBeVisible();
    expect(stored()[0].comments).toHaveLength(1);
    first.unmount();

    render(<CommunitySafety />);
    await openSeeded(user, "安全0票・危険2票の地点：夜は暗い");
    for (const button of within(voteGroup()).getAllByRole("button"))
      expect(button).toHaveAttribute("aria-disabled", "true");
    await user.click(
      within(voteGroup()).getByRole("button", { name: "安全に投票" }),
    );
    expect(
      screen.queryByRole("textbox", { name: "詳細情報を入力" }),
    ).not.toBeInTheDocument();
    expect(stored()[0].votes).toHaveLength(2);
  });

  it("lets another browser vote on the same point", async () => {
    const user = userEvent.setup();
    seed();
    const first = render(<CommunitySafety />);
    await openSeeded(user);
    await user.click(
      within(voteGroup()).getByRole("button", { name: "危険に投票" }),
    );
    await user.click(screen.getByRole("button", { name: "スキップ" }));
    await screen.findByLabelText("安全0票・危険2票");
    first.unmount();

    localStorage.setItem(VOTER_KEY, OTHER_BROWSER);
    render(<CommunitySafety />);
    await openSeeded(user, "安全0票・危険2票の地点：夜は暗い");
    await user.click(
      within(voteGroup()).getByRole("button", { name: "安全に投票" }),
    );
    await user.click(screen.getByRole("button", { name: "スキップ" }));
    expect(await screen.findByLabelText("安全1票・危険2票")).toBeVisible();
  });

  it("shows the server's refusal and switches to the voted state when another tab voted first", async () => {
    const user = userEvent.setup();
    localStorage.setItem(VOTER_KEY, OTHER_BROWSER);
    seed();
    render(<CommunitySafety />);
    await openSeeded(user);
    await user.click(
      within(voteGroup()).getByRole("button", { name: "安全に投票" }),
    );
    // このブラウザーの別のタブが先に投票した
    const [report] = stored();
    report.votes.push({ voter: OTHER_BROWSER, kind: "danger" });
    localStorage.setItem(DEMO_REPORTS_KEY, JSON.stringify([report]));
    await user.click(screen.getByRole("button", { name: "スキップ" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "この地点には投票済みです。",
    );
    expect(await screen.findByLabelText("安全0票・危険2票")).toBeVisible();
    expect(
      within(voteGroup()).getByRole("button", { name: "危険に投票" }),
    ).toHaveAttribute("aria-pressed", "true");
  });

  it("explains the voting restriction on hover, focus, and click without casting another vote", async () => {
    const user = userEvent.setup();
    localStorage.setItem(VOTER_KEY, OTHER_BROWSER);
    seed("夜は暗い", [{ voter: OTHER_BROWSER, kind: "danger" }]);
    render(<CommunitySafety />);
    await openSeeded(user);
    const savedBeforeVote = localStorage.getItem(DEMO_REPORTS_KEY);
    const vote = within(voteGroup()).getByRole("button", {
      name: "安全に投票",
    });
    expect(vote).toHaveAttribute("aria-disabled", "true");
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    await user.hover(vote);
    expect(screen.getByRole("tooltip")).toHaveTextContent(
      "※ 同一地点には一度しか入力できません",
    );
    await user.unhover(vote);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    fireEvent.focus(vote);
    expect(screen.getByRole("tooltip")).toBeVisible();
    fireEvent.blur(vote);
    await user.click(vote);
    await user.unhover(vote);
    fireEvent.blur(vote);
    expect(screen.getByRole("tooltip")).toBeVisible();
    expect(
      screen.queryByRole("textbox", { name: "詳細情報を入力" }),
    ).not.toBeInTheDocument();
    expect(localStorage.getItem(DEMO_REPORTS_KEY)).toBe(savedBeforeVote);
    fireEvent.keyDown(vote, { key: "Escape" });
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });

  it("reloads reports and the open review when another tab writes storage", async () => {
    const user = userEvent.setup();
    seed();
    render(<CommunitySafety />);
    await openSeeded(user);
    const [report] = stored();
    report.votes.push({ voter: "third-browser-0000001", kind: "safe" });
    localStorage.setItem(DEMO_REPORTS_KEY, JSON.stringify([report]));
    act(() => {
      window.dispatchEvent(
        new StorageEvent("storage", { key: DEMO_REPORTS_KEY }),
      );
    });
    expect(await screen.findByLabelText("安全1票・危険1票")).toBeVisible();
    expect(screen.getByRole("region", { name: "口コミ" })).toBeVisible();
  });
});

