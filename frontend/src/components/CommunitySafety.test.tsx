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
      const input = screen.getByRole("textbox", { name: "口コミ" });
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
    vi.mocked(navigator.geolocation.getCurrentPosition).mockImplementation(success => { resolveLocation = success; });
    render(<CommunitySafety />);
    fireEvent.click(screen.getByRole("application"), { clientX: 195, clientY: 320 });
    await user.click(screen.getByRole("button", { name: "危険" }));
    await user.type(screen.getByRole("textbox", { name: "口コミ" }), "夜道は危ない");
    act(() => resolveLocation({ coords: { latitude: 36, longitude: 140 } } as GeolocationPosition));
    expect(screen.getByRole("textbox", { name: "口コミ" })).toHaveValue("夜道は危ない");
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
      screen.getByRole("textbox", { name: "口コミ" }),
      "夜道は危ない",
    );
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("unavailable");
    });
    await user.click(screen.getByRole("button", { name: "投稿" }));
    expect(screen.getByRole("alert")).toHaveTextContent(
      "口コミを保存できませんでした。",
    );
    expect(screen.getByRole("textbox", { name: "口コミ" })).toHaveValue(
      "夜道は危ない",
    );
    expect(
      screen.queryByRole("button", { name: /危険の口コミ/ }),
    ).not.toBeInTheDocument();
  });
});
