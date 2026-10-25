import { Flag, Info } from "lucide-react";
import {
  formatPositionTime,
  formatRemainingDistance,
} from "../lib/format";
import type { NavigationKind, NavigationSnapshot, NavPhase } from "../navigation/session";
import type { SimulationMultiplier } from "../navigation/simulator";
import { SELECTION_LABEL } from "./NavigationStart";
import {
  SIMULATION_RESEARCH_NOTE,
  STAGE_TEXT,
  type ReSearchStage,
} from "../state/useReSearch";
import { SimulationControls } from "./SimulationControls";

export interface NavigationPanelProps {
  snapshot: NavigationSnapshot;
  onStop: () => void;
  onPause: () => void;
  onResume: () => void;
  onMultiplier: (value: SimulationMultiplier) => void;
  onJitter: (value: boolean) => void;
  onTryDeviation: () => void;
  onReSearch: () => void;
}

export const SIMULATION_NOTICE = "模擬走行中（実際の現在地ではありません）";

/** 模擬走行の状態。倍率と、進行/一時停止/停止/到着を常に示す。 */
export function simulationState(snapshot: NavigationSnapshot): string {
  if (!snapshot.simulation) return "";
  const state =
    snapshot.phase === "arrived"
      ? "到着"
      : snapshot.pauseReason === "sim-limit"
        ? "停止"
        : snapshot.phase === "paused"
          ? "一時停止"
          : "進行中";
  return `倍率 ${snapshot.simulation.multiplier}倍・${state}`;
}

export function SimulationBanner({ snapshot }: { snapshot: NavigationSnapshot }) {
  if (!snapshot.simulation) return null;
  return (
    <div className="nav-sim-banner">
      <strong>{SIMULATION_NOTICE}</strong>
      <span>{simulationState(snapshot)}</span>
    </div>
  );
}

export const PHASE_LABEL: Record<NavPhase, string> = {
  waiting: "測位待ち",
  tracking: "進行中",
  "poor-accuracy": "精度不足",
  "matching-hold": "照合保留",
  deviated: "経路から外れています",
  paused: "一時停止",
  arrived: "到着",
};

const SIM_STOP_CAUSE = {
  照合保留: "重複経路等で位置を確定できないため模擬走行を停止しました",
  精度不足: "位置精度が低いため模擬走行を停止しました",
  逸脱: "経路から外れたため模擬走行を停止しました",
} as const;

/** 状態の原因。進行中は不要なので空。 */
export function phaseCause(snapshot: NavigationSnapshot): string {
  switch (snapshot.phase) {
    case "waiting":
      return "位置情報の受信を待っています";
    case "poor-accuracy":
      if (snapshot.sourceError === "timeout" || snapshot.sourceError === "unavailable")
        return "位置を取得できないため判定を停止中";
      return snapshot.match.accuracy !== null && snapshot.match.accuracy > 30
        ? "位置精度が低いため判定を停止中"
        : "位置を受信できないため判定を停止中";
    case "matching-hold":
      return "経路上の位置を確認中";
    case "deviated":
      return "安全な場所で停止してから、現在地から再検索できます";
    case "paused":
      switch (snapshot.pauseReason) {
        case "hidden":
          return "画面が非表示になったため一時停止しました。再開すると全経路で位置を確認し直します";
        case "timer-gap":
          return "処理の遅延が大きいため一時停止しました。再開すると全経路で位置を確認し直します";
        case "sim-limit":
          return SIM_STOP_CAUSE[snapshot.simStopReason ?? "照合保留"];
        case "permission-denied":
          return "位置情報の利用が許可されていません。設定を変更して再開するか、模擬走行を使ってください";
        case "map-unavailable":
          return "地図を描画できないため一時停止しました。回復後に再開できます";
        default:
          return "再開すると全経路で位置を確認し直します";
      }
    case "arrived":
      return "経路の終点に到着しました（目的地の入力地点ではなく経路終点への到着判定です）";
    default:
      return "";
  }
}

const TONE: Partial<Record<NavPhase, string>> = {
  waiting: "is-caution",
  "poor-accuracy": "is-caution",
  "matching-hold": "is-caution",
  paused: "is-caution",
  deviated: "is-danger",
};

/** ナビ中にサイドバーの結果・条件エディタを置き換える。 */
export function NavigationPanel({
  snapshot,
  onStop,
  onPause,
  onResume,
  onMultiplier,
  onJitter,
  onTryDeviation,
  onReSearch,
}: NavigationPanelProps) {
  const { phase, remaining, lastFix, kind } = snapshot;
  const cause = phaseCause(snapshot);
  const showFix = phase !== "tracking" && lastFix !== null;
  const stopButton = (
    <button type="button" className="secondary full" onClick={onStop}>
      {phase === "paused" || phase === "arrived" ? "ナビを終了" : "ナビを停止"}
    </button>
  );
  return (
    <div className="nav-panel">
      <SimulationBanner snapshot={snapshot} />
      <section className={`nav-status ${TONE[phase] ?? ""}`} aria-label="ナビの状態">
        <p className="nav-phase" role="status">
          {phase === "arrived" && <Flag size={18} aria-hidden="true" />}
          {PHASE_LABEL[phase]}
        </p>
        {cause && <p className="nav-cause">{cause}</p>}
        {showFix && (
          <dl className="nav-fix">
            <div>
              <dt>精度</dt>
              <dd>約{Math.round(lastFix.accuracy)}m</dd>
            </div>
            <div>
              <dt>{phase === "paused" ? "一時停止時の位置" : "最後の位置"}</dt>
              <dd>{formatPositionTime(kind, lastFix.timestamp)}</dd>
            </div>
          </dl>
        )}
      </section>
      {snapshot.reSearchAvailable && phase !== "paused" && phase !== "arrived" && (
        <div className="nav-research">
          <button type="button" className="primary full" onClick={onReSearch}>
            安全な場所で停止してから現在地から再検索
          </button>
          {snapshot.simulation && <p className="field-help">{SIMULATION_RESEARCH_NOTE}</p>}
        </div>
      )}
      <dl className="nav-metrics">
        <div>
          <dt>残り距離（概算）</dt>
          <dd>
            {remaining ? formatRemainingDistance(remaining.displayDistanceM) : "—"}
          </dd>
        </div>
        <div>
          <dt>残り時間（概算）</dt>
          <dd>{remaining ? `${remaining.displayMinutes}分` : "—"}</dd>
        </div>
        {snapshot.remainingStale && (
          <div>
            <dt>進捗</dt>
            <dd>更新停止</dd>
          </div>
        )}
        <div>
          <dt>経路</dt>
          <dd>{SELECTION_LABEL[snapshot.selection]}</dd>
        </div>
      </dl>
      {/* 進捗が止まっている間は、古い位置に基づく接近表示を出さない（通知済みの区間は保持）。 */}
      {snapshot.alertMessage && phase === "tracking" && (
        <section className="nav-alert" aria-label="接近通知">
          <Info size={16} aria-hidden="true" />
          <div>
            <p role="status">{snapshot.alertMessage}</p>
            {snapshot.alertIds.length > 1 && (
              <p className="nav-alert-count">参考区間 {snapshot.alertIds.length}件</p>
            )}
          </div>
        </section>
      )}
      <div className="nav-reference-note">
        <p>過去事故の参考情報です。現在の危険や安全を保証するものではありません</p>
        <p>条件と事故データは検索時点のものです</p>
      </div>
      {!snapshot.simulation && phase !== "arrived" && (
        <button
          type="button"
          className="secondary full"
          onClick={phase === "paused" ? onResume : onPause}
        >
          {phase === "paused" ? "再開" : "一時停止"}
        </button>
      )}
      {snapshot.simulation && (
        <SimulationControls
          settings={snapshot.simulation}
          arrived={phase === "arrived"}
          onMultiplier={onMultiplier}
          onJitter={onJitter}
          onPause={onPause}
          onResume={onResume}
          onTryDeviation={onTryDeviation}
          afterActions={stopButton}
        />
      )}
      {!snapshot.simulation && stopButton}
      {snapshot.speechNotice && (
        <p className="notice warning" role="status">
          {snapshot.speechNotice}
        </p>
      )}
      {snapshot.wakeLock === "unavailable" && phase !== "arrived" && (
        <p className="field-help">画面が消えるとナビは一時停止します</p>
      )}
    </div>
  );
}

/** 再検索中。再検索中は停止・一時停止・終了を含む中止操作を設けない。 */
export function ReSearchPanel({
  stage,
  kind,
}: {
  stage: ReSearchStage;
  kind: NavigationKind;
}) {
  return (
    <div className="nav-panel">
      <section className="nav-status is-caution" aria-label="再検索の状態">
        <p className="nav-phase" role="status">
          再検索中
        </p>
        <p className="nav-cause">{STAGE_TEXT[stage]}</p>
        {kind === "simulation" && <p className="nav-cause">{SIMULATION_RESEARCH_NOTE}</p>}
      </section>
    </div>
  );
}

/** 再検索の中断・失敗。手動の再試行だけを出し、入力を編集すると通常の入力へ戻る。 */
export function ReSearchNotice({
  message,
  kind,
  onRetry,
}: {
  message: string;
  kind: NavigationKind;
  onRetry: () => void;
}) {
  return (
    <section className="nav-research-notice" aria-label="再検索の結果">
      <p className="notice warning" role="alert">
        {message}
      </p>
      {kind === "simulation" && <p className="field-help">{SIMULATION_RESEARCH_NOTE}</p>}
      <button type="button" className="primary full" onClick={onRetry}>
        再試行
      </button>
      <p className="field-help">入力を編集すると、通常の検索に戻ります</p>
    </section>
  );
}
