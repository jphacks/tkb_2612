import { describe, expect, it } from "vitest";
import { attributionLabel } from "./accidentAttribution";

describe("事故の発生場所の表示", () => {
  it("道路の区間に割り当てた事故を道路と表す", () => {
    expect(attributionLabel("edge")).toBe("道路");
  });

  it("交差点に割り当てた事故を交差点と表す", () => {
    expect(attributionLabel("intersection")).toBe("交差点");
  });

  it("帰属不明の事故を経路の近くと表す", () => {
    expect(attributionLabel("unattributed")).toBe("経路の近く");
  });
});
