import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createDemoConditions } from "../api/demo";
import {
  DETOUR_SETTING_KEY,
  readDetourSetting,
} from "./detourSetting";
import { useConditions } from "./useConditions";

const client = vi.hoisted(() => ({ getConditions: vi.fn() }));
vi.mock("../api/client", () => client);

beforeEach(() => {
  localStorage.clear();
  client.getConditions
    .mockReset()
    .mockImplementation((datetime: string) =>
      Promise.resolve(createDemoConditions(datetime)),
    );
});

function setup() {
  const view = renderHook(() =>
    useConditions({
      origin: null,
      dest: null,
      scenario: "normal",
      onEdit: vi.fn(),
    }),
  );
  return view;
}

describe("useConditions の許容時間の初期値と保存", () => {
  it("保存済みの手動値を初期値にし、選択の変更を保存する", async () => {
    localStorage.setItem(DETOUR_SETTING_KEY, "7");
    const view = setup();
    await waitFor(() => expect(client.getConditions).toHaveBeenCalled());
    expect(view.result.current.values.max_detour_min).toBe(7);

    act(() => view.result.current.change({ max_detour_min: "auto" }));
    expect(view.result.current.values.max_detour_min).toBe("auto");
    expect(readDetourSetting()).toBe("auto");

    act(() => view.result.current.change({ max_detour_min: 12 }));
    expect(readDetourSetting()).toBe(12);
  });

  it("保存値が不正なら自動を初期値にする", async () => {
    localStorage.setItem(DETOUR_SETTING_KEY, "不正");
    const view = setup();
    await waitFor(() => expect(client.getConditions).toHaveBeenCalled());
    expect(view.result.current.values.max_detour_min).toBe("auto");
  });

  it("読み取りが例外を投げても自動で起動する", async () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("storage disabled");
    });
    const view = setup();
    await waitFor(() => expect(client.getConditions).toHaveBeenCalled());
    expect(view.result.current.values.max_detour_min).toBe("auto");
  });
});
