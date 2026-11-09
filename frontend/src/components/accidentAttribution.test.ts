import { describe, expect, it } from "vitest";
import { attributionLabel } from "./accidentAttribution";

describe("事故の発生場所の表示", () => {
  it("道路の区間に割り当てた事故を経路上の道路と表す", () => {
    expect(attributionLabel("edge", 72)).toBe("経路上の道路");
  });

  it("交差点に割り当てた事故を経路上の交差点と表す", () => {
    expect(attributionLabel("intersection", 72)).toBe("経路上の交差点");
  });

  it("帰属不明の事故を経路の近くと表し、範囲を添える", () => {
    expect(attributionLabel("unattributed", 72)).toBe("経路の近く（72m以内）");
    expect(attributionLabel("unattributed", 118)).toBe(
      "経路の近く（118m以内）",
    );
  });
});
