import type { RouteCandidate } from "../lib/types";
import type { MatchStatus } from "./matcher";
import { VirtualClock } from "./clock";
import type { PositionConsumer, PositionFix, PositionSource } from "./source";
import { createRouteGeometry, pointAtProgress, unprojectPoint } from "./geometry";
export type SimulationMultiplier = 1 | 2 | 4 | 8;
export type SimulationStopReason = "照合保留" | "精度不足" | "逸脱";
export interface SimulatorOptions {
  seed?: number; jitter?: boolean; multiplier?: SimulationMultiplier;
  inject?: (fix: PositionFix, virtualSeconds: number) => PositionFix | null;
}
export const lcgNext = (seed: number): number => (Math.imul(1664525, seed) + 1013904223) >>> 0;
export class NavigationSimulator implements PositionSource {
  readonly clock = new VirtualClock();
  readonly baseSpeedMps: number;
  readonly progressSpeedMps: number;
  state: "idle" | "running" | "paused" | "stopped" | "arrived" = "idle";
  stopReason: SimulationStopReason | null = null;
  private readonly geometry;
  private seed: number;
  private jitter: boolean;
  private multiplier: SimulationMultiplier;
  private real = 0;
  private virtual = 0;
  private tick = 0;
  private measurementTimeOffset = 0;
  private deviationUntil = -1;
  private holdingSince: number | null = null;
  private endDeadline: number;
  private consumer: PositionConsumer | null = null;
  private feedbackStatus: MatchStatus = "waiting";
  private feedbackReason: SimulationStopReason | undefined;
  constructor(private readonly route: RouteCandidate, private readonly options: SimulatorOptions = {}) {
    if (!route.navigation || route.navigation.length_m <= 0 || route.duration <= 0) throw new Error("模擬走行できる経路がありません");
    this.geometry = createRouteGeometry(route.geometry.coordinates, route.navigation);
    this.seed = (options.seed ?? 1) >>> 0; this.jitter = options.jitter ?? true; this.multiplier = options.multiplier ?? 1;
    this.baseSpeedMps = route.distance / route.duration; this.progressSpeedMps = route.navigation.length_m / route.duration;
    this.endDeadline = route.duration * 1000 + 15000;
  }
  start(realNow: number, consumer: PositionConsumer): void {
    if (this.state !== "idle") return;
    this.endDeadline = this.route.duration * 1000 + 15000;
    this.real = realNow; this.consumer = consumer; this.state = "running"; this.emit();
  }
  setMultiplier(value: SimulationMultiplier): void { this.multiplier = value; }
  setJitter(value: boolean): void { this.jitter = value; }
  tryDeviation(): void { if (this.state === "running") this.deviationUntil = this.clock.now() + 20000; }
  pause(): void { if (this.state === "running") this.state = "paused"; }
  resume(realNow: number): void {
    if (this.state !== "paused" && this.state !== "stopped") return;
    this.real = realNow; this.state = "running"; this.stopReason = null; this.holdingSince = null;
    this.endDeadline = Math.max(this.virtual, this.route.duration * 1000) + 15000;
  }
  stop(): void {
    this.state = "idle"; this.consumer = null; this.seed = (this.options.seed ?? 1) >>> 0;
    this.virtual = 0; this.tick = 0; this.measurementTimeOffset = 0; this.clock.set(0); this.deviationUntil = -1; this.holdingSince = null; this.stopReason = null;
    this.endDeadline = this.route.duration * 1000 + 15000;
    this.feedbackStatus = "waiting"; this.feedbackReason = undefined;
  }
  /** 呼出し元がタイマーを管理する。各tick中の時計はその測位の仮想時刻。 */
  pump(realNow: number): void {
    if (this.state !== "running") return;
    const elapsed = realNow - this.real;
    if (elapsed < 0) return;
    if (elapsed > 5000) { this.pause(); return; }
    this.real = realNow;
    const target = this.virtual + elapsed * this.multiplier;
    while ((this.tick + 1) * 1000 + this.measurementTimeOffset <= target && this.state === "running") {
      this.tick++; this.virtual = this.tick * 1000 + this.measurementTimeOffset; this.clock.set(this.virtual); this.emit(); this.checkDeadline();
    }
    if (this.state === "running") { this.virtual = target; this.clock.set(target); }
  }
  /** 基準進捗は停止期限にだけ使い、判定器へ渡さない。毎tickの判定後に呼ぶ。 */
  report(status: MatchStatus, reason?: SimulationStopReason): void {
    if (this.state !== "running") return;
    this.feedbackStatus = status; this.feedbackReason = reason;
    if (status === "arrived") { this.state = "arrived"; return; }
    if (status === "matching-hold") this.holdingSince ??= this.clock.now(); else this.holdingSince = null;
    this.checkDeadline();
  }
  private checkDeadline(): void {
    if (this.state !== "running") return;
    if ((this.holdingSince !== null && this.clock.now() - this.holdingSince >= 60000) || this.clock.now() >= this.endDeadline) {
      this.state = "stopped"; this.stopReason = this.feedbackReason ?? (["poor-accuracy", "waiting"].includes(this.feedbackStatus) ? "精度不足" : "照合保留");
    }
  }
  /** 停止進捗を保ち、仮想時刻だけ1秒進めて未補正測位を返す。配信しない。 */
  measureStoppedPosition(): PositionFix | null {
    if (this.state !== "paused" && this.state !== "stopped") throw new Error("模擬走行を停止してから測位してください");
    this.measurementTimeOffset += 1000;
    this.virtual += 1000;
    this.clock.set(this.virtual);
    return this.generateFix(this.virtual);
  }
  private emit(): void {
    const fix = this.generateFix(this.tick * 1000 + this.measurementTimeOffset);
    if (fix) this.consumer?.(fix);
  }
  private generateFix(timestamp: number): PositionFix | null {
    const at = pointAtProgress(this.geometry, Math.min(this.geometry.lengthM, this.progressSpeedMps * this.tick));
    const a = this.geometry.points[at.segmentIndex], b = this.geometry.points[at.segmentIndex + 1];
    const len = Math.hypot(b.x - a.x, b.y - a.y);
    const offset = this.clock.now() < this.deviationUntil && len ? 120 / len : 0;
    this.seed = lcgNext(this.seed); const r = 8 * Math.sqrt(this.seed / 4294967296);
    this.seed = lcgNext(this.seed); const theta = 2 * Math.PI * this.seed / 4294967296;
    const position = unprojectPoint(this.geometry, { x: at.point.x - (b.y - a.y) * offset + (this.jitter ? r * Math.cos(theta) : 0), y: at.point.y + (b.x - a.x) * offset + (this.jitter ? r * Math.sin(theta) : 0) });
    const fix = { ...position, accuracy: 10, timestamp };
    return this.options.inject ? this.options.inject(fix, timestamp / 1000) : fix;
  }
}
