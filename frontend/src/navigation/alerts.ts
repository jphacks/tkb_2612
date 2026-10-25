import type { Mode, NavigationData } from "../lib/types";

export const ALERT_MESSAGES = {
  ahead: "この先に、過去事故の参考区間があります。周囲に注意してください",
  inside: "過去事故の参考区間にいます",
  multiple: "この先に、過去事故の参考区間が続きます",
} as const;
export type AlertKind = keyof typeof ALERT_MESSAGES;
export type AlertMessage = typeof ALERT_MESSAGES[keyof typeof ALERT_MESSAGES];
export interface AlertFix {
  /** 位置源の仮想時刻。受理済みの精度良好・一意照合だけを進行中として渡す。 */
  timestampMs: number;
  progressM: number | null;
  progressing: boolean;
}
export interface AlertNotification {
  kind: AlertKind;
  sectionIds: string[];
  message: AlertMessage;
}
export interface AlertResult {
  notification: AlertNotification | null;
  candidateIds: string[];
  notifiedIds: string[];
  passedIds: string[];
}
export interface Alerts {
  update(fix: AlertFix): AlertResult;
  /** 中断・保留・逸脱時に呼ぶ。通知済みと通過済みは保持する。 */
  suspend(): void;
}

/** 新規開始だけで作り直す。既存のサーバ区間をそのまま使う。 */
export function createAlerts(mode: Mode, sections: NavigationData["sections"]): Alerts {
  const distance = { walk: 80, bike: 150, car: 300 }[mode];
  const notified = new Set<string>();
  const passed = new Set<string>();
  const candidates = new Map<string, { since: number | null; inside: boolean }>();
  let previousTime: number | null = null;
  let previousProgress: number | null = null;
  let initialConfirmation = true;
  function suspend() {
    candidates.clear();
    previousProgress = null;
  }
  return {
    suspend,
    update(fix) {
      const result = (notification: AlertNotification | null = null): AlertResult => ({
        notification, candidateIds: [...candidates.keys()], notifiedIds: [...notified], passedIds: [...passed],
      });
      const { timestampMs: time, progressM: progress } = fix;
      if (!Number.isFinite(time) || (previousTime !== null && time <= previousTime)) {
        suspend();
        return result();
      }
      if (previousTime !== null && time - previousTime > 5000) suspend();
      previousTime = time;
      if (!fix.progressing || progress === null || !Number.isFinite(progress) || progress < 0) {
        suspend();
        return result();
      }
      const backwards = previousProgress !== null && previousProgress - progress > 5;
      previousProgress = progress;
      const ready: string[] = [];
      let inside = false;
      for (const section of sections) {
        const { id, start_m: start, end_m: end } = section;
        if ((initialConfirmation && progress > end) || progress > end + 30) passed.add(id);
        if (passed.has(id) || notified.has(id)) { candidates.delete(id); continue; }
        if (start - progress > distance + 30) {
          candidates.delete(id);
          continue;
        }
        const candidate = candidates.get(id);
        if (backwards || start - progress > distance || progress > end) {
          if (candidate) candidate.since = null;
          continue;
        }
        if (!candidate || candidate.since === null) {
          candidates.set(id, { since: time, inside: progress >= start });
          continue;
        }
        if (time - candidate.since >= 1000) {
          ready.push(id);
          inside ||= candidate.inside;
          notified.add(id);
          candidates.delete(id);
        }
      }
      initialConfirmation = false;
      const kind: AlertKind = ready.length > 1 ? "multiple" : inside ? "inside" : "ahead";
      return result(ready.length ? {
        kind,
        sectionIds: ready,
        message: ALERT_MESSAGES[kind],
      } : null);
    },
  };
}
