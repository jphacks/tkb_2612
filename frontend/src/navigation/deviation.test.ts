import { describe, expect, it } from "vitest";
import { createDeviation } from "./deviation";
import { ACCURACY_LIMIT_M, CONFIRM_FIXES, CONFIRM_MS, FIX_GAP_RESET_MS, ROUTE_CORRIDOR_MIN_M } from "./thresholds";
const fix = (timestampMs: number, distanceToRouteM = 61, accuracyM = 10, matchConfirmed = true) => ({ timestampMs, distanceToRouteM, accuracyM, matchConfirmed });
/** 0/5000/10000msの3測位で逸脱を確定させた検出器。 */
const deviatedDetector = () => {
  const detector = createDeviation();
  [0, 5000, 10000].forEach(t => detector.update(fix(t)));
  expect(detector.update(fix(10000 + 1)).deviated).toBe(true);
  return detector;
};
const POOR = ACCURACY_LIMIT_M + 1;
describe("逸脱と復帰", () => {
  it("復帰は確認時間を過ぎても測位数が足りなければ確定せず、規定数で確定する", () => {
    const detector = deviatedDetector();
    const start = 11000;
    // 規定数-1の測位で確認時間を十分に過ぎても確定しない。
    for (let i = 0; i < CONFIRM_FIXES - 1; i++) expect(detector.update(fix(start + i * CONFIRM_MS * 2, 0)).deviated).toBe(true);
    const last = start + (CONFIRM_FIXES - 1) * CONFIRM_MS * 2;
    expect(detector.update(fix(last, 0)).deviated).toBe(false);
  });
  it("復帰は規定数以上でも確認時間の1ms前では確定せず、ちょうどで確定する", () => {
    const detector = deviatedDetector();
    const start = 11000;
    for (let i = 0; i < CONFIRM_FIXES; i++) expect(detector.update(fix(start + i * (CONFIRM_MS - 1) / CONFIRM_FIXES, 0)).deviated).toBe(true);
    expect(detector.update(fix(start + CONFIRM_MS - 1, 0)).deviated).toBe(true);
    expect(detector.update(fix(start + CONFIRM_MS, 0)).deviated).toBe(false);
  });
  it("逸脱は3測位以上でも9999msでは確定せず10000msで確定する", () => {
    const detector = createDeviation();
    for (const t of [0, 5000, 9999]) expect(detector.update(fix(t)).deviated).toBe(false);
    expect(detector.update(fix(10000)).deviated).toBe(true);
  });
  it("逸脱は欠測で連続確認をリセットする", () => {
    const detector = createDeviation();
    expect(detector.update(fix(0)).deviated).toBe(false);
    expect(detector.update(fix(5000)).deviated).toBe(false);
    // 欠測の上限を1ms超えると、ここから数え直す。
    const after = 5000 + FIX_GAP_RESET_MS + 1;
    expect(detector.update(fix(after)).deviated).toBe(false);
    expect(detector.update(fix(after + 5000)).deviated).toBe(false);
    expect(detector.update(fix(after + 9999)).deviated).toBe(false);
    expect(detector.update(fix(after + 10000)).deviated).toBe(true);
  });
  it.each([10, ACCURACY_LIMIT_M])("accuracy=%i の厳密な逸脱閾値と10秒・3測位", accuracy => {
    const detector = createDeviation();
    const threshold = Math.max(40, 2 * accuracy);
    expect(detector.update(fix(0, threshold, accuracy)).suspected).toBe(false);
    expect(detector.update(fix(1000, threshold + 0.01, accuracy)).suspected).toBe(true);
    expect(detector.update(fix(6000, threshold + 0.01, accuracy)).deviated).toBe(false);
    expect(detector.update(fix(11000, threshold + 0.01, accuracy)).deviated).toBe(true);
  });
  it("復帰は一意照合と確認時間・規定数を要し、精度不足でも逸脱表示を保持", () => {
    const detector = deviatedDetector();
    expect(detector.update(fix(11000, 0, POOR)).deviated).toBe(true);
    [12000, 14500, 17000].forEach(t => expect(detector.update(fix(t, 20, 10, false)).deviated).toBe(true));
    const start = 18000;
    for (let i = 0; i < CONFIRM_FIXES - 1; i++) expect(detector.update(fix(start + i, 20)).deviated).toBe(true);
    expect(detector.update(fix(start + CONFIRM_MS - 1, 20)).deviated).toBe(true);
    expect(detector.update(fix(start + CONFIRM_MS, 20)).deviated).toBe(false);
  });
  it.each(["精度不足", "欠測", "古い入力", "無効値"])("%sは逸脱時間に足さない", reason => {
    const detector = createDeviation();
    detector.update(fix(0)); detector.update(fix(5000));
    const invalid = reason === "精度不足" ? fix(10000, 120, POOR) : reason === "古い入力" ? fix(5000)
      : reason === "無効値" ? fix(10000, NaN) : fix(5000 + FIX_GAP_RESET_MS + 1);
    expect(detector.update(invalid).deviated).toBe(false);
    // 欠測後の測位は新しい連続の1件目、それ以外は無効入力の次から数え直す。
    const start = reason === "欠測" ? 5000 + FIX_GAP_RESET_MS + 1 : 15000;
    if (reason !== "欠測") expect(detector.update(fix(start)).deviated).toBe(false);
    expect(detector.update(fix(start + 5000)).deviated).toBe(false);
    expect(detector.update(fix(start + 9999)).deviated).toBe(false);
    expect(detector.update(fix(start + 10000)).deviated).toBe(true);
  });
  it("復帰閾値はmax(下限, accuracy)、超過で復帰確認をリセット", () => {
    const detector = deviatedDetector();
    const limit = Math.max(ROUTE_CORRIDOR_MIN_M, ACCURACY_LIMIT_M);
    detector.update(fix(11000, limit, ACCURACY_LIMIT_M));
    detector.update(fix(13500, limit + 0.01, ACCURACY_LIMIT_M));
    const start = 16000;
    for (let i = 0; i < CONFIRM_FIXES - 1; i++) expect(detector.update(fix(start + i, limit, ACCURACY_LIMIT_M)).deviated).toBe(true);
    expect(detector.update(fix(start + CONFIRM_MS - 1, limit, ACCURACY_LIMIT_M)).deviated).toBe(true);
    expect(detector.update(fix(start + CONFIRM_MS, limit, ACCURACY_LIMIT_M)).deviated).toBe(false);
  });
  it("中断で時間だけを破棄し、逸脱は良好な復帰まで維持する", () => {
    const detector = deviatedDetector();
    detector.update(fix(11000, 0));
    detector.suspend();
    const start = 15000;
    for (let i = 0; i < CONFIRM_FIXES - 1; i++) expect(detector.update(fix(start + i, 0)).deviated).toBe(true);
    expect(detector.update(fix(start + CONFIRM_MS - 1, 0)).deviated).toBe(true);
    expect(detector.update(fix(start + CONFIRM_MS, 0)).deviated).toBe(false);
  });
});
