import { describe, expect, it } from "vitest";
import {
  formatDistance,
  formatDuration,
  toTokyoInput,
  toTokyoIso,
} from "./format";
import { createDemoConditions } from "../api/demo";

describe("Japanese datetime and display formatting", () => {
  it("converts an absolute date to Japanese local input regardless of host timezone", () => {
    expect(toTokyoInput(new Date("2026-10-23T18:10:42Z"))).toBe(
      "2026-10-24T03:10",
    );
    expect(toTokyoIso("2026-10-24T03:10")).toBe("2026-10-24T03:10+09:00");
    expect(() => toTokyoIso("2026-02-30T10:00")).toThrow();
    expect(() => toTokyoIso("1999-12-31T10:00")).toThrow();
  });

  it.each([
    ["03:59", "深夜"],
    ["04:00", "早朝"],
    ["08:59", "早朝"],
    ["09:00", "昼"],
    ["15:59", "昼"],
    ["16:00", "夕"],
    ["18:59", "夕"],
    ["19:00", "夜"],
    ["23:59", "夜"],
    ["00:00", "深夜"],
  ])(
    "uses the API 2.0 period boundary at %s in the demo adapter",
    (time, expected) => {
      expect(createDemoConditions(`2026-10-24T${time}+09:00`).period).toBe(
        expected,
      );
    },
  );

  it("does not claim complete holiday coverage in the demo adapter", () => {
    expect(createDemoConditions("2026-10-24T12:00+09:00").day_type).toBe(
      "休日",
    );
    expect(createDemoConditions("2026-10-26T12:00+09:00").day_type).toBe(
      "平日",
    );
    expect(createDemoConditions("2026-12-30T12:00+09:00").day_type_reason).toBe(
      "年末年始",
    );
    expect(
      createDemoConditions("2026-11-03T12:00+09:00").holiday_table_covered,
    ).toBe(false);
  });

  it("formats API seconds and metres with explicit display units", () => {
    expect(formatDuration(61)).toBe("2分");
    expect(formatDuration(3600)).toBe("1時間");
    expect(formatDuration(3660)).toBe("1時間1分");
    expect(formatDistance(923)).toBe("923 m");
    expect(formatDistance(2450)).toBe("2.5 km");
  });
});
