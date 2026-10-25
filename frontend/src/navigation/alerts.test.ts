import { describe, expect, it } from "vitest";
import fixtures from "../../../docs/api/fixtures/navigation-cases.json";
import type { Mode, NavigationData } from "../lib/types";
import { createAlerts } from "./alerts";

const section = (id = "a", start_m = 200, end_m = 250): NavigationData["sections"][number] => ({
  id, start_m, end_m, start_segment_index: 0, end_segment_index: 0, severity_weight: 3,
});
const fix = (timestampMs: number, progressM: number | null, progressing = true) => ({ timestampMs, progressM, progressing });

describe("接近通知", () => {
  it.each<[Mode, number]>([["walk", 80], ["bike", 150], ["car", 300]])("%s は %im の境界と1秒以上の2測位を使う", (mode, distance) => {
    const alerts = createAlerts(mode, [section("a", 500, 550)]);
    expect(alerts.update(fix(0, 500 - distance - 0.01)).notification).toBeNull();
    expect(alerts.update(fix(1000, 500 - distance)).notification).toBeNull();
    expect(alerts.update(fix(1999, 500 - distance)).notification).toBeNull();
    expect(alerts.update(fix(2000, 500 - distance)).notification?.sectionIds).toEqual(["a"]);
  });
  it("区間内開始と0長区間も2測位で通知する", () => {
    for (const end of [200, 250]) {
      const alerts = createAlerts("walk", [section("a", 200, end)]);
      alerts.update(fix(0, 200));
      expect(alerts.update(fix(1000, 200)).notification?.message).toBe("過去事故の参考区間にいます");
    }
  });
  it("初回の既通過と測位間の飛越しを後追いしない", () => {
    const alerts = createAlerts("walk", [section("past", 0, 10), section()]);
    expect(alerts.update(fix(0, 100)).passedIds).toContain("past");
    alerts.update(fix(1000, 120));
    expect(alerts.update(fix(2000, 251)).notification).toBeNull();
    expect(alerts.update(fix(3000, 251)).notification).toBeNull();
  });
  it("候補はD+30mまで保持し、D超で連続カウンタを解除する", () => {
    const alerts = createAlerts("walk", [section()]);
    alerts.update(fix(0, 100));
    alerts.update(fix(1000, 120));
    expect(alerts.update(fix(2000, 90)).candidateIds).toEqual(["a"]);
    expect(alerts.update(fix(3000, 89.99)).candidateIds).toEqual([]);
    alerts.update(fix(4000, 120));
    expect(alerts.update(fix(5000, 119)).notification).toBeNull();
    expect(alerts.update(fix(6000, 120)).notification).toBeNull();
    expect(alerts.update(fix(7000, 120)).notification?.sectionIds).toEqual(["a"]);
  });
  it("end+30mを超えて通過済みにし、後戻りでも保持する", () => {
    const alerts = createAlerts("walk", [section()]);
    alerts.update(fix(0, 100));
    expect(alerts.update(fix(1000, 280)).passedIds).toEqual([]);
    expect(alerts.update(fix(2000, 280.01)).passedIds).toEqual(["a"]);
    for (let t = 3000; t <= 6000; t += 1000) expect(alerts.update(fix(t, 200)).notification).toBeNull();
  });
  it("末尾直後からend+30mまでは候補を保持し、通知の連続確認だけをリセットする", () => {
    const alerts = createAlerts("walk", [section()]);
    expect(alerts.update(fix(0, 249)).candidateIds).toEqual(["a"]);
    for (const [time, progress] of [[1000, 251], [2000, 280]]) {
      const result = alerts.update(fix(time, progress));
      expect(result.candidateIds).toEqual(["a"]);
      expect(result.passedIds).toEqual([]);
      expect(result.notification).toBeNull();
    }
    expect(alerts.update(fix(3000, 250)).notification).toBeNull();
    expect(alerts.update(fix(4000, 250)).notification).toBeNull();
    expect(alerts.update(fix(5000, 250)).notification?.kind).toBe("inside");
  });
  it("末尾直後から5m以内で戻っても、改めて2測位を確認する", () => {
    const alerts = createAlerts("walk", [section()]);
    alerts.update(fix(0, 249));
    expect(alerts.update(fix(1000, 251)).notification).toBeNull();
    expect(alerts.update(fix(2000, 250)).notification).toBeNull();
    expect(alerts.update(fix(3000, 250)).notification?.kind).toBe("inside");
  });
  it("末尾直後では候補を新設せず、end+30m超で既存候補を削除する", () => {
    const alerts = createAlerts("walk", [section()]);
    alerts.update(fix(0, 100));
    expect(alerts.update(fix(1000, 251)).candidateIds).toEqual([]);
    alerts.update(fix(2000, 249));
    const result = alerts.update(fix(3000, 280.01));
    expect(result.candidateIds).toEqual([]);
    expect(result.passedIds).toEqual(["a"]);
    expect(result.notification).toBeNull();
  });
  it("5m超の後退測位を数えず、非後退の2測位から再開する", () => {
    const alerts = createAlerts("walk", [section()]);
    alerts.update(fix(0, 140));
    expect(alerts.update(fix(1000, 134)).notification).toBeNull();
    expect(alerts.update(fix(2000, 134)).notification).toBeNull();
    expect(alerts.update(fix(3000, 135)).notification?.sectionIds).toEqual(["a"]);
    const boundary = createAlerts("walk", [section()]);
    boundary.update(fix(0, 140));
    expect(boundary.update(fix(1000, 135)).notification).not.toBeNull();
  });
  it("通知済みは後戻り・中断・復帰で消さず新規インスタンスだけで解除する", () => {
    const alerts = createAlerts("walk", [section()]);
    alerts.update(fix(0, 120));
    expect(alerts.update(fix(1000, 120)).notification).not.toBeNull();
    alerts.suspend();
    alerts.update(fix(2000, null, false));
    for (let t = 3000; t <= 6000; t += 1000) expect(alerts.update(fix(t, 200)).notification).toBeNull();
    const fresh = createAlerts("walk", [section()]);
    fresh.update(fix(0, 200));
    expect(fresh.update(fix(1000, 200)).notification).not.toBeNull();
  });
  it.each(["保留", "進行停止", "欠測", "古い入力"])("%sで連続確認をリセットする", (reason) => {
    const alerts = createAlerts("walk", [section()]);
    alerts.update(fix(1000, 120));
    const input = reason === "保留" ? fix(2000, null) : reason === "進行停止" ? fix(2000, 120, false) : reason === "欠測" ? fix(6001, 120) : fix(1000, 120);
    expect(alerts.update(input).notification).toBeNull();
    const t = input.timestampMs + 1000;
    const next = alerts.update(fix(t, 120));
    if (reason !== "欠測") expect(next.notification).toBeNull();
  });
  it("同時通知はIDを個別記録して1文にまとめ、連続区間を分割しない", () => {
    const alerts = createAlerts("car", [section("a"), section("b", 300, 350)]);
    alerts.update(fix(0, 100));
    expect(alerts.update(fix(1000, 100)).notification).toEqual({ kind: "multiple", sectionIds: ["a", "b"], message: "この先に、過去事故の参考区間が続きます" });

  });
  it.each(fixtures.cases)("fixture $name の結合済み全区間を距離規則でそのまま通知する", fixture => {
    const data = fixture.expected.navigation;
    if (!data) throw new Error("navigation fixture がありません");
    const alerts = createAlerts("car", data.sections);
    alerts.update(fix(0, 0));
    const expected = data.sections.filter(s => s.start_m <= 300).map(s => s.id);
    const result = alerts.update(fix(1000, 0));
    expect(result.notifiedIds).toEqual(expected);
    expect(result.notification?.sectionIds ?? []).toEqual(expected);
    if (fixture.name === "over_100_contiguous") expect(expected).toHaveLength(1);
    if (fixture.name === "over_100_separate") expect(expected.length).toBeGreaterThan(100);
  });
  it("別時点で接近する区間はそれぞれ通知する", () => {
    const alerts = createAlerts("walk", [section("a"), section("b", 400, 450)]);
    alerts.update(fix(0, 120));
    expect(alerts.update(fix(1000, 120)).notification?.message).toBe("この先に、過去事故の参考区間があります。周囲に注意してください");
    alerts.update(fix(2000, 320));
    expect(alerts.update(fix(3000, 320)).notification?.sectionIds).toEqual(["b"]);
  });
  it("中断後の再確認でもend+30mの通過猶予を維持する", () => {
    const alerts = createAlerts("walk", [section()]);
    alerts.update(fix(0, 100));
    alerts.suspend();
    expect(alerts.update(fix(1000, 251)).passedIds).toEqual([]);
    expect(alerts.update(fix(2000, 250)).notification).toBeNull();
    expect(alerts.update(fix(3000, 250)).notification?.message).toBe("過去事故の参考区間にいます");
  });

});
