import type { Mode, RouteCandidate } from "../lib/types";
import { createAlerts, type AlertMessage, type Alerts } from "./alerts";
import { RealClock } from "./clock";
import { createDeviation, type Deviation } from "./deviation";
import { createDeviceSource, type DeviceError } from "./device";
import { type MatchResult, type MatchStatus, NavigationMatcher } from "./matcher";
import { calculateRemaining, type Remaining } from "./remaining";
import {
  NavigationSimulator,
  type SimulationMultiplier,
  type SimulatorOptions,
  type SimulationStopReason,
} from "./simulator";
import type { PositionFix, PositionSource } from "./source";
import {
  createSpeech,
  createBrowserSpeechAdapter,
  type NavigationSpeech,
  type SpeechAdapter,
  type SpeechCancelReason,
} from "./speech";
import { createScreenWakeLock, type ScreenWakeLock, type WakeLockApi } from "./wakeLock";
import { createRouteGeometry, pointAtProgress, unprojectPoint } from "./geometry";
import type { NavigationSelection } from "./preflight";

export type NavigationKind = "device" | "simulation";
export type NavPhase =
  | "waiting"
  | "tracking"
  | "poor-accuracy"
  | "matching-hold"
  | "deviated"
  | "paused"
  | "arrived";
export type PauseReason =
  | "user"
  | "hidden"
  | "timer-gap"
  | "sim-limit"
  | "permission-denied"
  | "map-unavailable";

export interface SimulationSettings {
  seed: number;
  multiplier: SimulationMultiplier;
  jitter: boolean;
}

export interface NavigationSnapshot {
  routeId: string;
  selection: NavigationSelection;
  kind: NavigationKind;
  phase: NavPhase;
  pauseReason: PauseReason | null;
  simStopReason: SimulationStopReason | null;
  match: MatchResult;
  /** 逸脱が確定している間はtrue。復帰は新しい良好測位まで保持する。 */
  deviated: boolean;
  remaining: Remaining | null;
  remainingStale: boolean;
  /** 地図に置く現在地。照合できているときだけ経路上の位置。 */
  location: { lon: number; lat: number; onRoute: boolean } | null;
  /** 補正しない最後の受理測位。精度円と最後の位置時点に使う。 */
  lastFix: PositionFix | null;
  sourceError: DeviceError | null;
  wakeLock: "unknown" | "held" | "unavailable";
  /** 通知済みで通過前の区間ID。 */
  alertIds: string[];
  alertMessage: AlertMessage | null;
  /** 逸脱中、または照合保留が続いている間。現在地からの再検索を案内できる。 */
  reSearchAvailable: boolean;
  speechNotice: string | null;
  simulation: (SimulationSettings & { running: boolean }) | null;
}

export interface NavigationDeps {
  geolocation?: Pick<Geolocation, "watchPosition" | "clearWatch">;
  wakeLock?: WakeLockApi;
  speech?: SpeechAdapter | null;
  clock?: RealClock;
}

export interface NavigationSessionOptions {
  route: RouteCandidate;
  selection: NavigationSelection;
  mode: Mode;
  kind: NavigationKind;
  simulation?: SimulationSettings;
  startMessage: string;
  deps?: NavigationDeps;
  /** 試験用。模擬の位置源へaccuracyの悪化や欠測を注入する。画面からは設定できない。 */
  inject?: SimulatorOptions["inject"];
  onChange(snapshot: NavigationSnapshot): void;
}

const TICK_MS = 250;
/** 照合保留が続いたとみなす実時間。開始直後の位置確認（約5秒）では案内しない。 */
const HOLD_RE_SEARCH_MS = 15_000;
const EMPTY_RESULT: MatchResult = {
  status: "waiting", p: null, lastConfirmedP: null, lastConfirmedTimestamp: null,
  lastAccepted: null, distanceToRouteM: null, accuracy: null, timestamp: null,
  accepted: null, updated: false,
};
const ARRIVED: Remaining = { distanceM: 0, durationS: 0, displayDistanceM: 0, displayMinutes: 0 };

/**
 * 1回のナビ（開始から終了まで）の状態機械。位置源・共通判定器・接近・逸脱・音声を結線し、
 * UIには不変のsnapshotだけを渡す。位置・進捗・通知済みIDはこのオブジェクトのメモリにだけ置く。
 */
export class NavigationSession {
  readonly route: RouteCandidate;
  readonly selection: NavigationSelection;
  readonly kind: NavigationKind;
  private readonly geometry;
  private readonly clock: RealClock;
  private readonly source: PositionSource;
  private readonly simulator: NavigationSimulator | null;
  private readonly matcher: NavigationMatcher;
  private readonly alerts: Alerts;
  private readonly deviation: Deviation = createDeviation();
  private readonly speech: NavigationSpeech;
  private readonly wake: ScreenWakeLock;
  private readonly simulationSettings: SimulationSettings | null;
  private timer: ReturnType<typeof setInterval> | null = null;
  private lifecycle: "running" | "paused" | "arrived" | "stopped" = "running";
  private pauseReason: PauseReason | null = null;
  private match: MatchResult = EMPTY_RESULT;
  private deviated = false;
  private remaining: Remaining | null = null;
  private remainingStale = false;
  private location: NavigationSnapshot["location"] = null;
  private lastFix: PositionFix | null = null;
  private sourceError: DeviceError | null = null;
  private wakeState: NavigationSnapshot["wakeLock"] = "unknown";
  private notified = new Set<string>();
  private passed = new Set<string>();
  private alertMessage: AlertMessage | null = null;
  private speechNotice: string | null = null;
  private phaseBefore: NavPhase = "waiting";
  private holdSince: number | null = null;
  private holdLong = false;
  private cached: NavigationSnapshot | null = null;
  private lastRawFix: PositionFix | null = null;

  constructor(private readonly options: NavigationSessionOptions) {
    const { route, deps = {} } = options;
    this.route = route;
    this.selection = options.selection;
    this.kind = options.kind;
    this.clock = deps.clock ?? new RealClock();
    this.geometry = createRouteGeometry(route.geometry.coordinates, route.navigation!);
    this.alerts = createAlerts(options.mode, route.navigation!.sections);
    this.speech = createSpeech({
      adapter: deps.speech === undefined ? createBrowserSpeechAdapter() : deps.speech,
      onUnavailable: (message) => {
        this.speechNotice = message;
        this.publish();
      },
    });
    this.wake = createScreenWakeLock(
      deps.wakeLock ?? (typeof navigator !== "undefined"
        ? (navigator as Navigator & { wakeLock?: WakeLockApi }).wakeLock
        : undefined),
      () => {
        this.wakeState = "unavailable";
        this.publish();
      },
    );
    this.simulationSettings = options.kind === "simulation" ? { ...options.simulation! } : null;
    if (options.kind === "simulation") {
      this.simulator = new NavigationSimulator(route, { ...this.simulationSettings!, inject: options.inject });
      this.source = this.simulator;
    } else {
      this.simulator = null;
      const geolocation = deps.geolocation ??
        (typeof navigator !== "undefined" ? navigator.geolocation : undefined);
      this.source = createDeviceSource(
        geolocation ?? {
          watchPosition: () => { throw new Error("unsupported"); },
          clearWatch: () => {},
        },
        { onError: (error) => this.deviceError(error) },
        this.clock,
      );
    }
    this.matcher = new NavigationMatcher(route.geometry, route.navigation!, this.source.clock);
  }

  /** 開始ボタンの操作から同期的に呼ぶ（音声のテスト発話が操作起点を必要とするため）。 */
  begin(): void {
    this.speech.enable(this.options.startMessage);
    this.source.start(this.clock.monotonicNow(), (fix) => this.handleFix(fix));
    this.startTimer();
    void this.acquireWakeLock();
    this.publish();
  }

  getSnapshot(): NavigationSnapshot {
    if (!this.cached) this.cached = this.build();
    return this.cached;
  }

  /** 明示的再検索用。最後に受信した補正前の測位（精度不足でも）。 */
  lastUncorrectedFix(): PositionFix | null {
    return this.lastRawFix;
  }

  /** 位置源の鮮度時計。再検索の「5秒以内」判定に使う。 */
  sourceNow(): number {
    return this.source.clock.now();
  }

  pause(reason: PauseReason = "user", speechReason: SpeechCancelReason = "pause"): void {
    if (this.lifecycle !== "running") return;
    this.lifecycle = "paused";
    this.pauseReason = reason;
    this.stopTimer();
    this.source.pause();
    this.match = this.matcher.pause();
    this.deviation.suspend();
    this.alerts.suspend();
    this.holdSince = null;
    this.holdLong = false;
    this.speech.cancel(speechReason);
    this.wake.release();
    this.wakeState = "unknown";
    this.publish();
  }

  /** 明示再開。全経路で位置を再確認し、通知済み集合は保持する。 */
  resume(): void {
    if (this.lifecycle !== "paused") return;
    this.lifecycle = "running";
    this.pauseReason = null;
    this.sourceError = null;
    this.source.resume(this.clock.monotonicNow());
    this.match = this.matcher.resume();
    this.phaseBefore = "waiting";
    this.startTimer();
    void this.acquireWakeLock();
    this.publish();
  }

  /** hidden / pagehide。自動再開しない。 */
  suspendForLifecycle(kind: "hidden" | "pagehide"): void {
    this.pause("hidden", kind);
  }

  stop(reason: SpeechCancelReason = "stop"): void {
    if (this.lifecycle === "stopped") return;
    this.lifecycle = "stopped";
    this.stopTimer();
    this.source.stop();
    this.matcher.stop();
    this.speech.cancel(reason);
    this.wake.release();
  }

  setMultiplier(value: SimulationMultiplier): void {
    if (!this.simulator || !this.simulationSettings) return;
    // 倍率変更の直前に旧倍率で進める。
    this.simulator.pump(this.clock.monotonicNow());
    this.simulator.setMultiplier(value);
    this.simulationSettings.multiplier = value;
    this.publish();
  }

  setJitter(value: boolean): void {
    if (!this.simulator || !this.simulationSettings) return;
    this.simulator.setJitter(value);
    this.simulationSettings.jitter = value;
    this.publish();
  }

  tryDeviation(): void {
    this.simulator?.tryDeviation();
  }

  private startTimer(): void {
    this.stopTimer();
    this.timer = setInterval(() => this.tick(), TICK_MS);
  }

  private stopTimer(): void {
    if (this.timer !== null) clearInterval(this.timer);
    this.timer = null;
  }

  private tick(): void {
    if (this.lifecycle !== "running") return;
    this.simulator?.pump(this.clock.monotonicNow());
    if (this.lifecycle !== "running") return;
    if (this.simulator) {
      if (this.simulator.state === "paused") return this.pause("timer-gap");
      if (this.simulator.state === "stopped") return this.pause("sim-limit");
    }
    this.apply(this.matcher.poll(), true);
    if (
      this.lifecycle === "running" && !this.holdLong && this.holdSince !== null &&
      this.clock.monotonicNow() - this.holdSince >= HOLD_RE_SEARCH_MS
    ) {
      this.holdLong = true;
      this.publish();
    }
  }

  private handleFix(fix: PositionFix): void {
    if (this.lifecycle !== "running") return;
    this.lastRawFix = fix;
    this.sourceError = null;
    this.apply(this.matcher.update(fix), false);
    if (this.simulator && this.lifecycle === "running") {
      this.simulator.report(this.match.status, this.deviated ? "逸脱" : undefined);
      if (this.simulator.state === "stopped") this.pause("sim-limit");
    }
  }

  private deviceError(error: DeviceError): void {
    if (this.lifecycle !== "running") return;
    if (error === "denied") {
      this.sourceError = error;
      this.pause("permission-denied");
      return;
    }
    this.sourceError = error;
    this.publish();
  }

  private async acquireWakeLock(): Promise<void> {
    const held = await this.wake.acquire();
    if (this.lifecycle !== "running") {
      if (held) this.wake.release();
      return;
    }
    this.wakeState = held ? "held" : "unavailable";
    this.publish();
  }

  private apply(result: MatchResult, fromPoll: boolean): void {
    // 新しい受理位置のない定期確認は、状態が変わったときだけ反映する。
    if (fromPoll && result.status === this.match.status) return;
    this.match = result;
    if (fromPoll || result.accepted === null) {
      if (result.status !== "tracking") {
        this.deviation.suspend();
        this.alerts.suspend();
      }
    }
    if (result.accepted) {
      const accepted = result.accepted;
      this.lastFix = result.lastAccepted ?? accepted;
      if (result.distanceToRouteM !== null) {
        this.deviated = this.deviation.update({
          timestampMs: accepted.timestamp,
          accuracyM: accepted.accuracy,
          distanceToRouteM: result.distanceToRouteM,
          matchConfirmed: result.status === "tracking",
        }).deviated;
      }
      const progressing = result.status === "tracking" && result.updated &&
        result.p !== null && !this.deviated;
      const alert = this.alerts.update({
        timestampMs: accepted.timestamp,
        progressM: result.p,
        progressing,
      });
      this.notified = new Set(alert.notifiedIds);
      this.passed = new Set(alert.passedIds);
      if (alert.notification) this.speech.request(alert.notification);
    }
    if (result.updated && result.p !== null) {
      this.remaining = result.status === "arrived"
        ? ARRIVED
        : calculateRemaining({ ...this.route, navigation: this.route.navigation! }, result.p);
    }
    this.remainingStale =
      (result.status !== "tracking" && result.status !== "arrived") || this.deviated;
    this.updateLocation();
    this.updateAlertMessage();
    this.afterPhase(result.status);
    if (result.status === "arrived") this.finishArrival();
    this.publish();
  }

  private afterPhase(status: MatchStatus): void {
    const phase = this.phase(status);
    if (phase === this.phaseBefore) return;
    this.phaseBefore = phase;
    this.holdSince = phase === "matching-hold" ? this.clock.monotonicNow() : null;
    this.holdLong = false;
    if (phase === "poor-accuracy") this.speech.cancel("accuracy");
    else if (phase === "matching-hold" || phase === "waiting") this.speech.cancel("pending");
    else if (phase === "deviated") this.speech.cancel("deviation");
  }

  private finishArrival(): void {
    if (this.lifecycle !== "running") return;
    this.lifecycle = "arrived";
    this.stopTimer();
    this.source.stop();
    this.speech.cancel("stop");
    this.wake.release();
    this.wakeState = "unknown";
    this.remaining = ARRIVED;
    this.remainingStale = false;
    this.updateLocation();
  }

  private updateLocation(): void {
    const result = this.match;
    const confirmed = (result.status === "tracking" || result.status === "arrived") && !this.deviated;
    const progress = result.lastConfirmedP;
    if (confirmed && progress !== null) {
      const at = unprojectPoint(this.geometry, pointAtProgress(this.geometry, progress).point);
      this.location = { lon: at.lon, lat: at.lat, onRoute: true };
    } else if (this.lastFix) {
      this.location = { lon: this.lastFix.lon, lat: this.lastFix.lat, onRoute: false };
    }
  }

  private updateAlertMessage(): void {
    const active = this.route.navigation!.sections.filter(
      (section) => this.notified.has(section.id) && !this.passed.has(section.id),
    );
    if (!active.length) {
      this.alertMessage = null;
      return;
    }
    const progress = this.match.lastConfirmedP;
    const inside = progress !== null &&
      active.some((section) => section.start_m <= progress && progress <= section.end_m);
    this.alertMessage = inside
      ? "過去事故の参考区間にいます"
      : active.length > 1
        ? "この先に、過去事故の参考区間が続きます"
        : "この先に、過去事故の参考区間があります。周囲に注意してください";
  }

  private phase(status: MatchStatus = this.match.status): NavPhase {
    if (this.lifecycle === "arrived" || status === "arrived") return "arrived";
    if (this.lifecycle === "paused") return "paused";
    if (this.deviated) return "deviated";
    switch (status) {
      case "tracking":
      case "poor-accuracy":
      case "matching-hold":
        return status;
      default:
        return "waiting";
    }
  }

  private publish(): void {
    this.cached = this.build();
    if (this.lifecycle !== "stopped") this.options.onChange(this.cached);
  }

  private build(): NavigationSnapshot {
    const active = this.route.navigation!.sections
      .filter((section) => this.notified.has(section.id) && !this.passed.has(section.id))
      .map((section) => section.id);
    return Object.freeze({
      routeId: this.route.id,
      selection: this.selection,
      kind: this.kind,
      phase: this.phase(),
      pauseReason: this.pauseReason,
      simStopReason: this.pauseReason === "sim-limit" ? this.simulator?.stopReason ?? null : null,
      match: this.match,
      deviated: this.deviated,
      remaining: this.remaining,
      remainingStale: this.remainingStale,
      location: this.location,
      lastFix: this.lastFix,
      sourceError: this.sourceError,
      wakeLock: this.wakeState,
      alertIds: active,
      alertMessage: this.alertMessage,
      reSearchAvailable: this.deviated || this.holdLong,
      speechNotice: this.speechNotice,
      simulation: this.simulationSettings
        ? { ...this.simulationSettings, running: this.lifecycle === "running" }
        : null,
    });
  }
}
