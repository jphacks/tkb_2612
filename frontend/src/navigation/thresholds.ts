/** ナビ判定の閾値。照合・逸脱・接近通知・再検索で同じ値を使う。 */

/** これを超えるaccuracy(m)の測位は判定に使わない。 */
export const ACCURACY_LIMIT_M = 50;
/** 最後の良好な測位からこの時間(ms)は、測位が届かなくても直前の状態を保つ。 */
export const NO_FIX_GRACE_MS = 10_000;
/** 測位間隔がこれ(ms)を超えたら、確認・連続判定をやり直す。 */
export const FIX_GAP_RESET_MS = 10_000;
/** 受理する測位の古さの上限(ms)。古い位置で進捗を誤らないよう緩めない。 */
export const FIX_MAX_AGE_MS = 5_000;
/** 端末時計とのずれとして、未来側に許すtimestampの幅(ms)。 */
export const FIX_FUTURE_SKEW_MS = 1_000;
/** 照合の確定（初回・再確認・逸脱からの復帰）に要する測位数と継続時間(ms)。 */
export const CONFIRM_FIXES = 2;
export const CONFIRM_MS = 2_000;
/** 経路上とみなす距離の下限(m)。逸脱開始の下限40mより小さく保つ。 */
export const ROUTE_CORRIDOR_MIN_M = 30;
