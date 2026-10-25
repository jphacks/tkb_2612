import type { Clock } from "./clock";
export interface PositionFix { lon: number; lat: number; accuracy: number; timestamp: number; }
export type PositionConsumer = (position: PositionFix) => void;
/** 実機アダプタも同じ位置と鮮度時計を公開する。登録解除はアダプタ側の責任。 */
export interface PositionSource {
  readonly clock: Clock;
  start(realNow: number, consumer: PositionConsumer): void;
  pause(): void;
  resume(realNow: number): void;
  stop(): void;
}
