import { describe, expect, it } from "vitest";
import { createDeviation } from "./deviation";
const fix = (timestampMs: number, distanceToRouteM = 61, accuracyM = 10, matchConfirmed = true) => ({ timestampMs, distanceToRouteM, accuracyM, matchConfirmed });
describe("逸脱と復帰", () => {
  it("復帰は5秒経過しても2測位なら確定せず、3測位目で確定する", () => {
    const detector = createDeviation();
    [0, 5000, 10000].forEach(t => detector.update(fix(t)));
    expect(detector.update(fix(11000, 0)).deviated).toBe(true);
    expect(detector.update(fix(16000, 0)).deviated).toBe(true);
    expect(detector.update(fix(16001, 0)).deviated).toBe(false);
  });
  it("復帰は3測位以上でも4999msでは確定せず5000msで確定する", () => {
    const detector = createDeviation();
    [0, 5000, 10000].forEach(t => detector.update(fix(t)));
    for (const t of [11000, 13500, 14000, 15999]) {
      expect(detector.update(fix(t, 0)).deviated).toBe(true);
    }
    expect(detector.update(fix(16000, 0)).deviated).toBe(false);
  });
  it("逸脱は3測位以上でも9999msでは確定せず10000msで確定する", () => {
    const detector = createDeviation();
    for (const t of [0, 5000, 9999]) expect(detector.update(fix(t)).deviated).toBe(false);
    expect(detector.update(fix(10000)).deviated).toBe(true);
  });
  it("逸脱は10秒経過しても2測位なら欠測で連続確認をリセットする", () => {
    const detector = createDeviation();
    // 10秒を2測位だけで隔てると必ず5秒超の欠測になるため、測位数条件単独では検証できない。
    expect(detector.update(fix(0)).deviated).toBe(false);
    expect(detector.update(fix(10000)).deviated).toBe(false);
    expect(detector.update(fix(15000)).deviated).toBe(false);
    expect(detector.update(fix(20000)).deviated).toBe(true);
  });
  it.each([10, 30])("accuracy=%i の厳密な逸脱閾値と10秒・3測位", accuracy => {
    const detector = createDeviation();
    const threshold = Math.max(40, 2 * accuracy);
    expect(detector.update(fix(0, threshold, accuracy)).suspected).toBe(false);
    expect(detector.update(fix(1000, threshold + 0.01, accuracy)).suspected).toBe(true);
    expect(detector.update(fix(6000, threshold + 0.01, accuracy)).deviated).toBe(false);
    expect(detector.update(fix(11000, threshold + 0.01, accuracy)).deviated).toBe(true);
  });
  it("復帰は一意照合と5秒・3測位、精度不足でも逸脱表示を保持", () => {
    const detector = createDeviation();
    [0, 5000, 10000].forEach(t => detector.update(fix(t)));
    expect(detector.update(fix(11000, 0, 31)).deviated).toBe(true);
    [12000, 14500, 17000].forEach(t => expect(detector.update(fix(t, 20, 10, false)).deviated).toBe(true));
    expect(detector.update(fix(18000, 20)).deviated).toBe(true);
    expect(detector.update(fix(20500, 20)).deviated).toBe(true);
    expect(detector.update(fix(23000, 20)).deviated).toBe(false);
  });
  it.each(["精度不足", "欠測", "古い入力", "無効値"])("%sは逸脱時間に足さない", reason => {
    const detector = createDeviation();
    detector.update(fix(0)); detector.update(fix(5000));
    const invalid = reason === "精度不足" ? fix(10000, 120, 31) : reason === "古い入力" ? fix(5000) : reason === "無効値" ? fix(10000, NaN) : fix(10001);
    expect(detector.update(invalid).deviated).toBe(false);
    expect(detector.update(fix(15000)).deviated).toBe(false);
    expect(detector.update(fix(20000)).deviated).toBe(false);
    expect(detector.update(fix(25000)).deviated).toBe(true);
  });
  it("accuracy=30 の復帰閾値は30m、超過と保留で復帰確認をリセット", () => {
    const detector = createDeviation();
    [0, 5000, 10000].forEach(t => detector.update(fix(t)));
    detector.update(fix(11000, 30, 30));
    detector.update(fix(13500, 30.01, 30));
    expect(detector.update(fix(16000, 30, 30)).deviated).toBe(true);
    expect(detector.update(fix(18500, 30, 30)).deviated).toBe(true);
    expect(detector.update(fix(21000, 30, 30)).deviated).toBe(false);
  });
  it("中断で時間だけを破棄し、逸脱は良好な復帰まで維持する", () => {
    const detector = createDeviation();
    [0, 5000, 10000].forEach(t => detector.update(fix(t)));
    detector.suspend();
    expect(detector.update(fix(15000, 0)).deviated).toBe(true);
    expect(detector.update(fix(17500, 0)).deviated).toBe(true);
    expect(detector.update(fix(20000, 0)).deviated).toBe(false);
  });
});
