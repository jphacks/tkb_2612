import { describe, expect, it } from "vitest";
import { findDemoPlaces } from "./demo";

describe("findDemoPlaces", () => {
  it("matches addresses as well as labels and returns the two Chiyoda stations", () => {
    const results = findDemoPlaces("千代田区").results;
    expect(results.map((place) => place.label)).toEqual(["東京駅", "秋葉原駅"]);
  });
});
