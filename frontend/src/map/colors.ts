/**
 * 地図アダプタが読む色。styles.css の :root のトークンと同じ名前で、
 * CSSを読めない環境（試験・読込前）ではトークンと同じ値の既定を使う。
 */
export const TOKEN_DEFAULTS = {
  "--blue": "#2563eb",
  "--caution": "#d97706",
  "--surface": "#fff",
} as const;
export type MapToken = keyof typeof TOKEN_DEFAULTS;

export function mapColor(token: MapToken): string {
  try {
    const value = getComputedStyle(document.documentElement).getPropertyValue(token).trim();
    if (value) return value;
  } catch {
    /* CSSを読めないときは既定値。 */
  }
  return TOKEN_DEFAULTS[token];
}
