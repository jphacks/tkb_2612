export interface DeviationFix {
  /** 受理済み測位の仮想時刻。鮮度・座標検査は位置源側で行う。 */
  timestampMs: number;
  accuracyM: number;
  distanceToRouteM: number;
  /** 一意照合を確認できた場合だけtrue。省略時は復帰を確定しない。 */
  matchConfirmed?: boolean;
}
export interface DeviationResult {
  deviated: boolean;
  suspected: boolean;
  accuracyInsufficient: boolean;
}
export interface Deviation {
  update(fix: DeviationFix): DeviationResult;
  /** 連続判定だけを破棄する。表示中の逸脱は復帰まで維持する。 */
  suspend(): void;
}

export function createDeviation(): Deviation {
  let deviated = false;
  let since: number | null = null;
  let count = 0;
  let previousTime: number | null = null;
  function suspend() { since = null; count = 0; }
  return {
    suspend,
    update({ timestampMs: time, accuracyM: accuracy, distanceToRouteM: distance, matchConfirmed }) {
      const insufficient = !Number.isFinite(accuracy) || accuracy <= 0 || accuracy > 30;
      const invalid = !Number.isFinite(time) || !Number.isFinite(distance) || distance < 0 ||
        (previousTime !== null && time <= previousTime);
      if (invalid || insufficient) {
        suspend();
        if (Number.isFinite(time) && (previousTime === null || time > previousTime)) previousTime = time;
        return { deviated, suspected: false, accuracyInsufficient: insufficient };
      }
      if (previousTime !== null && time - previousTime > 5000) suspend();
      previousTime = time;
      const qualifies = deviated
        ? matchConfirmed === true && distance <= Math.max(20, accuracy)
        : distance > Math.max(40, 2 * accuracy);
      if (!qualifies) suspend();
      else {
        since ??= time;
        count += 1;
        if (count >= 3 && time - since >= (deviated ? 5000 : 10000)) {
          deviated = !deviated;
          suspend();
        }
      }
      return { deviated, suspected: !deviated && since !== null, accuracyInsufficient: false };
    },
  };
}
