/** API durations are seconds; displayed estimates round up to the next minute. */
export function formatDuration(seconds: number): string {
  const minutes = Math.max(1, Math.ceil(seconds / 60));
  return minutes < 60
    ? `${minutes}分`
    : `${Math.floor(minutes / 60)}時間${minutes % 60 ? `${minutes % 60}分` : ""}`;
}

export function formatDistance(metres: number): string {
  return metres < 1000
    ? `${Math.round(metres)} m`
    : `${(metres / 1000).toFixed(1)} km`;
}

/** datetime-local has no timezone. Always populate it with Japan time, not host time. */
export function toTokyoInput(date: Date): string {
  if (!Number.isFinite(date.getTime()))
    throw new Error("有効な日時を指定してください。");
  return new Date(date.getTime() + 9 * 60 * 60 * 1000)
    .toISOString()
    .slice(0, 16);
}

export function toTokyoIso(localValue: string): string {
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(localValue))
    throw new Error("出発日時を入力してください。");
  const result = `${localValue}+09:00`;
  const date = new Date(result);
  if (!Number.isFinite(date.getTime()) || toTokyoInput(date) !== localValue)
    throw new Error("有効な出発日時を入力してください。");
  const year = Number(localValue.slice(0, 4));
  if (year < 2000 || year > 2100)
    throw new Error("出発日時は2000年から2100年の間で指定してください。");
  return result;
}

/** frontend-spec 5: comparison durations in minutes with one decimal (API seconds). */
export function formatMinutes(seconds: number): string {
  return `${(seconds / 60).toFixed(1)}分`;
}

/** frontend-spec 5: comparison distances in kilometres with two decimals (API metres). */
export function formatKilometres(metres: number): string {
  return `${(metres / 1000).toFixed(2)} km`;
}

/** frontend-spec 5: extra_minutes is already minutes; shown with one decimal. */
export function formatExtraMinutes(minutes: number): string {
  return `+${minutes.toFixed(1)}分`;
}

/** ナビの残り距離（10m単位に切り上げ済みの値）。1km以上はkm、未満はm。 */
export function formatRemainingDistance(metres: number): string {
  return metres < 1000 ? `${metres} m` : `${(metres / 1000).toFixed(2)} km`;
}

/** 最後の位置の時点。実機は端末の時刻、模擬は走行開始からの仮想秒。 */
export function formatPositionTime(
  kind: "device" | "simulation",
  timestamp: number,
): string {
  if (kind === "simulation") return `走行開始から${Math.floor(timestamp / 1000)}秒`;
  const date = new Date(timestamp);
  const two = (value: number) => String(value).padStart(2, "0");
  return `${two(date.getHours())}:${two(date.getMinutes())}:${two(date.getSeconds())}`;
}
