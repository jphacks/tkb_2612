import { ALERT_MESSAGES } from "./alerts";
import type { AlertKind, AlertNotification } from "./alerts";

export const SPEECH_UNAVAILABLE = "音声を利用できません。画面通知を使用します";
export type SpeechRequest = AlertKind | AlertNotification;
export type SpeechRequestResult = "requested" | "screen-only";
export type SpeechCancelReason = "accuracy" | "pending" | "deviation" | "stop" | "pause" | "hidden" | "pagehide" | "unmount";
export interface SpeechVoice { lang: string; localService: boolean; }
export interface SpeechEvents { onStart(): void; onEnd(): void; onError(): void; }
export interface SpeechAdapter {
  getVoices(): readonly SpeechVoice[];
  speak(text: string, voice: SpeechVoice, events: SpeechEvents): void;
  cancel(): void;
}
export interface SpeechClock {
  /** 倍率非依存の実時間の単調時計。タイマーも同じ時間軸を使う。 */
  now(): number;
  setTimeout(callback: () => void, delayMs: number): unknown;
  clearTimeout(handle: unknown): void;
}
export interface SpeechOptions {
  adapter: SpeechAdapter | null;
  clock?: SpeechClock;
  onUnavailable?: (message: typeof SPEECH_UNAVAILABLE) => void;
}
export interface NavigationSpeech {
  /** 開始・停車後の明示操作から同期的に呼ぶ。開始用の承認済み定型文は呼出側が指定する。 */
  enable(startMessage: string): SpeechRequestResult;
  request(notification: SpeechRequest): SpeechRequestResult;
  cancel(reason: SpeechCancelReason): void;
}

const realClock: SpeechClock = {
  now: () => performance.now(),
  setTimeout: (callback, delay) => globalThis.setTimeout(callback, delay),
  clearTimeout: handle => globalThis.clearTimeout(handle as ReturnType<typeof setTimeout>),
};

/** 音声失敗は位置処理へ例外を渡さず画面通知へ切り替える。キューは保持しない。 */
export function createSpeech({ adapter, clock = realClock, onUnavailable }: SpeechOptions): NavigationSpeech {
  let enabled = false;
  let voice: SpeechVoice | undefined;
  let lastStart: number | null = null;
  let active: { started: boolean; deadline: number; timer: unknown } | null = null;
  function cancel() {
    const old = active;
    active = null;
    if (!old) return;
    clock.clearTimeout(old.timer);
    try { adapter?.cancel(); } catch { /* 取消失敗でも位置処理を続ける。 */ }
  }
  function fail() {
    enabled = false;
    cancel();
    onUnavailable?.(SPEECH_UNAVAILABLE);
  }
  function speak(text: string): SpeechRequestResult {
    if (!enabled || !adapter || !voice || active || (lastStart !== null && clock.now() - lastStart < 10000)) return "screen-only";
    const current = { started: false, deadline: clock.now() + 3000, timer: undefined as unknown };
    active = current;
    current.timer = clock.setTimeout(() => { if (active === current) fail(); }, 3000);
    try {
      adapter.speak(text, voice, {
        onStart() {
          if (active !== current || current.started) return;
          if (clock.now() > current.deadline) { fail(); return; }
          clock.clearTimeout(current.timer);
          current.started = true;
          lastStart = clock.now();
          current.deadline = lastStart + 15000;
          current.timer = clock.setTimeout(() => { if (active === current) fail(); }, 15000);
        },
        onEnd() {
          if (active !== current) return;
          if (!current.started || clock.now() > current.deadline) { fail(); return; }
          active = null;
          clock.clearTimeout(current.timer);
        },
        onError() { if (active === current) fail(); },
      });
    } catch { fail(); }
    return enabled ? "requested" : "screen-only";
  }
  return {
    enable(startMessage) {
      cancel();
      try {
        const japanese = adapter?.getVoices().filter(v => /^ja(?:-JP)?$/i.test(v.lang)) ?? [];
        // 言語を優先し、同じ言語なら端末内音声を優先する。
        voice = [...japanese].sort((a, b) =>
          Number(/^ja-JP$/i.test(b.lang)) - Number(/^ja-JP$/i.test(a.lang)) ||
          Number(b.localService) - Number(a.localService),
        )[0];
      } catch { voice = undefined; }
      if (!voice) { fail(); return "screen-only"; }
      enabled = true;
      return speak(startMessage);
    },
    request(notification) {
      const kind = typeof notification === "string" ? notification : notification.kind;
      return speak(ALERT_MESSAGES[kind]);
    },
    cancel,
  };
}

/** DOM依存はこの薄いアダプタに閉じ込め、判定器にはフェイクを注入できる。 */
export function createBrowserSpeechAdapter(
  synthesis: Pick<SpeechSynthesis, "getVoices" | "speak" | "cancel"> | undefined = globalThis.speechSynthesis,
  makeUtterance: ((text: string) => SpeechSynthesisUtterance) | undefined =
    typeof globalThis.SpeechSynthesisUtterance === "function" ? text => new SpeechSynthesisUtterance(text) : undefined,
): SpeechAdapter | null {
  if (!synthesis || !makeUtterance) return null;
  return {
    getVoices: () => synthesis.getVoices(),
    speak(text, voice, events) {
      const utterance = makeUtterance(text);
      utterance.lang = "ja-JP";
      utterance.voice = synthesis.getVoices().find(candidate => candidate === voice) ?? null;
      if (!utterance.voice) throw new Error(SPEECH_UNAVAILABLE);
      utterance.onstart = () => events.onStart();
      utterance.onend = () => events.onEnd();
      utterance.onerror = () => events.onError();
      synthesis.speak(utterance);
    },
    cancel: () => synthesis.cancel(),
  };
}
