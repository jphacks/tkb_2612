import { describe, expect, it } from "vitest";
import { visibleDetail } from "./placeDisplay";
import type { Place } from "./types";

const place = (
  label: string,
  detail: string | null = null,
  lat = 35,
  lon = 139,
): Place => ({ label, detail, lat, lon });

describe("visibleDetail", () => {
  it("normalizes labels with NFKC and trims them before counting duplicates", () => {
    const target = place("  ＴＯＫＹＯ駅  ", "東京都千代田区丸の内1丁目");
    const same = place("TOKYO駅", "東京都千代田区丸の内2丁目", 36, 140);
    expect(
      visibleDetail(target, [target, same], "  ＴＯＫＹＯ  "),
    ).toBe("東京都千代田区丸の内1丁目");
  });

  it("normalizes and trims the query before checking inclusion", () => {
    const target = place("東京駅", "東京都千代田区丸の内1丁目");
    expect(visibleDetail(target, [target], "  東京  ")).toBeNull();
  });

  it("returns null when the place has no detail even if its label is duplicated", () => {
    const target = place("東京駅");
    const same = place("東京駅", "東京都千代田区丸の内1丁目", 36, 140);
    expect(visibleDetail(target, [target, same], "東京")).toBeNull();
  });

  it("returns null for a single candidate whose label contains the query", () => {
    const target = place("東京駅", "東京都千代田区丸の内1丁目");
    expect(visibleDetail(target, [target], "東京")).toBeNull();
  });

  it("returns the detail when the query is not part of the label", () => {
    const target = place("東京駅", "東京都千代田区丸の内1丁目");
    expect(visibleDetail(target, [target], "千代田区")).toBe(
      "東京都千代田区丸の内1丁目",
    );
  });
});
