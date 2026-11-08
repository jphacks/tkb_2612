import { describe, expect, it } from "vitest";
import { attributionLabel } from "./accidentAttribution";

describe("事故の結び付けの表示", () => {
  it("道路の区間への割り当てを表す", () => {
    expect(attributionLabel("edge", 72)).toBe("道路の区間");
  });

  it("交差点への割り当てを表す", () => {
    expect(attributionLabel("intersection", 72)).toBe("交差点");
  });

  it("帰属不明と経路からの範囲を表す", () => {
    expect(attributionLabel("unattributed", 72)).toBe(
      "道路を特定できず（経路から72m以内）",
    );
    expect(attributionLabel("unattributed", 118)).toBe(
      "道路を特定できず（経路から118m以内）",
    );
  });
});
