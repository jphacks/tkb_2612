import { useState } from "react";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { searchPlaces } from "../api/client";
import type { GeocodeResponse, Place } from "../lib/types";
import LocationInput, { type LocationValue } from "./LocationInput";

vi.mock("../api/client", () => ({ isDemo: true, searchPlaces: vi.fn() }));

const tokyo: Place = {
  label: "東京駅 — 東京都千代田区丸の内1丁目",
  lat: 35.6812,
  lon: 139.7671,
};
const ueno: Place = {
  label: "上野駅 — 東京都台東区上野7丁目",
  lat: 35.7138,
  lon: 139.7773,
};
const akihabara: Place = {
  label: "秋葉原駅 — 東京都千代田区外神田1丁目",
  lat: 35.6984,
  lon: 139.7731,
};
const response = (results: Place[]): GeocodeResponse => ({
  results,
  attribution: [{ text: "デモ候補", url: "https://example.com/" }],
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

function renderLocation(initial: LocationValue = { text: "", place: null }) {
  const onChange = vi.fn();
  function Harness() {
    const [value, setValue] = useState(initial);
    return (
      <LocationInput
        label="出発地"
        value={value}
        scenario="normal"
        onChange={(next) => {
          onChange(next);
          setValue(next);
        }}
      />
    );
  }
  render(<Harness />);
  return { input: screen.getByRole("combobox", { name: "出発地" }), onChange };
}

async function advance(milliseconds: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(milliseconds);
  });
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.mocked(searchPlaces).mockReset();
  vi.mocked(searchPlaces).mockResolvedValue(response([tokyo, ueno, akihabara]));
});
afterEach(() => {
  vi.useRealTimers();
});

describe("LocationInput", () => {
  it("waits for two characters and a 300ms debounce, then confirms the chosen API label verbatim", async () => {
    const { input, onChange } = renderLocation();
    fireEvent.change(input, { target: { value: "東" } });
    await advance(500);
    expect(searchPlaces).not.toHaveBeenCalled();

    fireEvent.change(input, { target: { value: "東京" } });
    await advance(299);
    expect(searchPlaces).not.toHaveBeenCalled();
    expect(screen.queryByText("地点を確定しました")).not.toBeInTheDocument();
    await advance(1);
    expect(searchPlaces).toHaveBeenCalledWith(
      "東京",
      expect.any(AbortSignal),
      "normal",
    );
    fireEvent.click(screen.getByRole("option", { name: tokyo.label }));
    expect(onChange).toHaveBeenLastCalledWith({
      text: tokyo.label,
      place: tokyo,
    });
    expect(input).toHaveValue(tokyo.label);
    expect(input).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByText("地点を確定しました")).toBeInTheDocument();
  });

  it("does not search or confirm during IME composition and debounces after composition ends", async () => {
    const { input, onChange } = renderLocation();
    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "とうきょう" } });
    await advance(1_000);
    fireEvent.keyDown(input, { key: "Enter", isComposing: true, keyCode: 229 });
    expect(searchPlaces).not.toHaveBeenCalled();
    expect(onChange.mock.calls.every(([value]) => value.place === null)).toBe(
      true,
    );

    fireEvent.change(input, { target: { value: "東京" } });
    fireEvent.compositionEnd(input);
    await advance(299);
    expect(searchPlaces).not.toHaveBeenCalled();
    await advance(1);
    expect(searchPlaces).toHaveBeenCalledTimes(1);
    expect(searchPlaces).toHaveBeenLastCalledWith(
      "東京",
      expect.any(AbortSignal),
      "normal",
    );
    expect(
      screen.getByRole("option", { name: tokyo.label }),
    ).toBeInTheDocument();
  });

  it("cancels an outstanding debounce when composition begins", async () => {
    const { input } = renderLocation();
    fireEvent.change(input, { target: { value: "東京" } });
    await advance(250);
    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "東京え" } });
    await advance(1_000);
    expect(searchPlaces).not.toHaveBeenCalled();
  });

  it("clears confirmed coordinates immediately when the label is edited", async () => {
    const { input, onChange } = renderLocation({
      text: tokyo.label,
      place: tokyo,
    });
    expect(screen.getByText("地点を確定しました")).toBeInTheDocument();
    fireEvent.change(input, { target: { value: "上野" } });
    expect(onChange).toHaveBeenLastCalledWith({ text: "上野", place: null });
    expect(screen.queryByText("地点を確定しました")).not.toBeInTheDocument();
    expect(searchPlaces).not.toHaveBeenCalled();
    await advance(300);
    expect(searchPlaces).toHaveBeenCalledWith(
      "上野",
      expect.any(AbortSignal),
      "normal",
    );
  });

  it("discards a late response even when the upstream mock ignores abort", async () => {
    const oldResponse = deferred<GeocodeResponse>();
    vi.mocked(searchPlaces).mockImplementationOnce(() => oldResponse.promise);
    vi.mocked(searchPlaces).mockResolvedValueOnce(response([ueno]));
    const { input } = renderLocation();
    fireEvent.change(input, { target: { value: "東京" } });
    await advance(300);
    const oldSignal = vi.mocked(searchPlaces).mock.calls[0][1]!;

    fireEvent.change(input, { target: { value: "上野" } });
    expect(oldSignal.aborted).toBe(true);
    await advance(300);
    expect(
      screen.getByRole("option", { name: ueno.label }),
    ).toBeInTheDocument();
    await act(async () => {
      oldResponse.resolve(response([tokyo]));
      await oldResponse.promise;
    });
    expect(
      screen.queryByRole("option", { name: tokyo.label }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("option", { name: ueno.label }),
    ).toBeInTheDocument();
  });

  it("selects the last suggestion on the first ArrowUp and confirms it with Enter", async () => {
    const { input, onChange } = renderLocation();
    fireEvent.change(input, { target: { value: "東京" } });
    await advance(300);
    fireEvent.keyDown(input, { key: "ArrowUp" });
    const last = screen.getByRole("option", { name: akihabara.label });
    expect(last).toHaveAttribute("aria-selected", "true");
    expect(input).toHaveAttribute("aria-activedescendant", last.id);
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onChange).toHaveBeenLastCalledWith({
      text: akihabara.label,
      place: akihabara,
    });
    expect(screen.getByText("地点を確定しました")).toBeInTheDocument();
  });
});
