import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ConditionValues } from "../lib/types";
import {
  DETOUR_SETTING_KEY,
  readDetourSetting,
  routeConditions,
  saveDetourSetting,
} from "./detourSetting";

const values: ConditionValues = {
  mode: "walk",
  period: "昼",
  day_type: "平日",
  weather: "晴",
  max_detour_min: 7,
};

beforeEach(() => {
  localStorage.clear();
});

describe("許容時間の localStorage 設定", () => {
  it("未保存なら自動", () => {
    expect(readDetourSetting()).toBe("auto");
  });

  it("自動と手動の値を往復して保存・復元する", () => {
    saveDetourSetting("auto");
    expect(localStorage.getItem(DETOUR_SETTING_KEY)).toBe("auto");
    expect(readDetourSetting()).toBe("auto");

    saveDetourSetting(0);
    expect(readDetourSetting()).toBe(0);
    saveDetourSetting(30);
    expect(readDetourSetting()).toBe(30);
    saveDetourSetting(7);
    expect(readDetourSetting()).toBe(7);
  });

  it.each(["", " ", "-1", "31", "1.5", "7分", "NaN", "Infinity"])(
    "不正な保存値 %j は自動として扱う",
    (saved) => {
      localStorage.setItem(DETOUR_SETTING_KEY, saved);
      expect(readDetourSetting()).toBe("auto");
    },
  );

  it("読み取りが例外を投げたら自動として扱う", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("storage disabled");
    });
    expect(readDetourSetting()).toBe("auto");
  });

  it("保存が例外を投げても検索を止めない", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("quota exceeded");
    });
    expect(() => saveDetourSetting(7)).not.toThrow();
  });
});

describe("routeConditions", () => {
  it("自動では max_detour_min を要求へ入れない", () => {
    const result = routeConditions({ ...values, max_detour_min: "auto" });
    expect(result).not.toHaveProperty("max_detour_min");
    expect(result).toEqual({
      mode: "walk",
      period: "昼",
      day_type: "平日",
      weather: "晴",
    });
  });

  it("手動では値をそのまま要求へ入れる", () => {
    expect(routeConditions({ ...values, max_detour_min: 0 })).toMatchObject({
      max_detour_min: 0,
    });
    expect(routeConditions(values).max_detour_min).toBe(7);
  });
});
