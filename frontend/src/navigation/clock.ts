/** timestampとnowはミリ秒。位置判定と実時間の音声制御は別の時計を使う。 */
export interface Clock { now(): number; }
export class VirtualClock implements Clock {
  private time = 0;
  now(): number { return this.time; }
  set(time: number): void { this.time = time; }
}
export class RealClock implements Clock {
  constructor(private readonly wall: () => number = Date.now, private readonly monotonic: () => number = () => performance.now()) { }
  now(): number { return this.wall(); }
  monotonicNow(): number { return this.monotonic(); }
}
