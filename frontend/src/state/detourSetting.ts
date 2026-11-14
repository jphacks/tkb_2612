import type { ConditionValues } from "../lib/types";

/** 許容時間の設定。`"auto"` は要求で max_detour_min を省略する。 */
export type DetourSetting = ConditionValues["max_detour_min"];

/**
 * 既存の localStorage キー（accilessnavi.*）の命名に合わせる。
 * 保存するのは許容時間の設定（自動、または手動の分）だけ。
 */
export const DETOUR_SETTING_KEY = "accilessnavi.detour-limit.v1";

/** 保存済みの設定。未保存・不正値・読み取り例外は自動として扱う。 */
export function readDetourSetting(): DetourSetting {
  try {
    const saved = localStorage.getItem(DETOUR_SETTING_KEY);
    if (saved === null || saved === "auto") return "auto";
    // 手動は0〜30分の1分刻み（UIのスライダーと同じ）。空文字や小数は不正。
    if (!/^(?:0|[1-9]\d*)$/.test(saved)) return "auto";
    const value = Number(saved);
    return value >= 0 && value <= 30 ? value : "auto";
  } catch {
    return "auto";
  }
}

/** 設定を保存する。保存できない環境でも検索自体は続行する。 */
export function saveDetourSetting(setting: DetourSetting): void {
  try {
    localStorage.setItem(
      DETOUR_SETTING_KEY,
      setting === "auto" ? "auto" : String(setting),
    );
  } catch {
    // プライベートブラウズ・容量切れ等では保存しない。
  }
}

/**
 * ConditionValues から POST /api/route の条件値を作る。
 * 自動のときは max_detour_min を要求へ入れない（キー自体を送らない）。
 */
export function routeConditions(
  values: ConditionValues,
): Omit<ConditionValues, "max_detour_min"> & { max_detour_min?: number } {
  const { max_detour_min, ...rest } = values;
  return max_detour_min === "auto"
    ? rest
    : { ...rest, max_detour_min };
}
