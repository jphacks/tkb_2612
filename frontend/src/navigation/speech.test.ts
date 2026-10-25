import { afterEach, describe, expect, it, vi } from "vitest";
import { createAlerts, ALERT_MESSAGES } from "./alerts";
import { createBrowserSpeechAdapter, createSpeech, SPEECH_UNAVAILABLE } from "./speech";
import { createDeviation } from "./deviation";
import type { SpeechAdapter, SpeechEvents, SpeechVoice, SpeechClock, SpeechCancelReason } from "./speech";

// 呼出側の文言をそのまま渡す検証用の値。画面や製品の文言としては使用しない。
const START_MESSAGE = "__caller_start_message__";

function setup(voices: SpeechVoice[] = [{ lang: "ja-JP", localService: true }]) {
  vi.useFakeTimers();
  let events: SpeechEvents | undefined;
  const clock: SpeechClock = { now: () => performance.now(), setTimeout: (cb, delay) => setTimeout(cb, delay), clearTimeout: handle => clearTimeout(handle as ReturnType<typeof setTimeout>) };
  // 実時間の時計とタイマーだけを進め、仮想時計は音声へ渡さない。
  let realTime = 0;
  clock.now = () => realTime;
  const advance = (ms: number) => { realTime += ms; vi.advanceTimersByTime(ms); };
  const adapter: SpeechAdapter = {
    getVoices: vi.fn(() => voices),
    speak: vi.fn((_text, _voice, callbacks) => { events = callbacks; }),
    cancel: vi.fn(),
  };
  const unavailable = vi.fn();
  const speech = createSpeech({ adapter, clock, onUnavailable: unavailable });
  return { speech, adapter, unavailable, advance, events: () => events! };
}
afterEach(() => vi.useRealTimers());

describe("音声の実時間制御", () => {
  it.each([false, true])("位置未確定でも開始用文言だけを発話する（区間なし=%s）", empty => {
    const f = setup();
    const sections = empty ? [] : [{ id: "a", start_m: 200, end_m: 250, start_segment_index: 0, end_segment_index: 0, severity_weight: 3 }];
    const alerts = createAlerts("walk", sections);
    expect(alerts.update({ timestampMs: 0, progressM: null, progressing: false }).notification).toBeNull();
    expect(f.speech.enable(START_MESSAGE)).toBe("requested");
    expect(f.adapter.speak).toHaveBeenCalledExactlyOnceWith(START_MESSAGE, expect.anything(), expect.anything());
  });
  it.each(["ahead", "inside", "multiple"] as const)("%sの通知結果をそのまま音声へ渡す", kind => {
    const f = setup();
    f.speech.enable(START_MESSAGE);
    f.events().onStart(); f.events().onEnd();
    f.advance(10000);
    const section = { id: "a", start_m: 200, end_m: 250, start_segment_index: 0, end_segment_index: 0, severity_weight: 3 };
    const alerts = createAlerts("walk", kind === "multiple" ? [section, { ...section, id: "b" }] : [section]);
    const progressM = kind === "inside" ? 200 : 120;
    alerts.update({ timestampMs: 0, progressM, progressing: true });
    const notification = alerts.update({ timestampMs: 1000, progressM, progressing: true }).notification;
    if (!notification) throw new Error("通知がありません");
    expect(notification.kind).toBe(kind);
    expect(f.speech.request(notification)).toBe("requested");
    expect(f.adapter.speak).toHaveBeenLastCalledWith(notification.message, expect.anything(), expect.anything());
    expect(notification.message).toBe(ALERT_MESSAGES[kind]);
  });
  it("開始操作中に定型テスト発話を同期要求しja-JPの端末内音声を優先する", () => {
    const remote = { lang: "ja-JP", localService: false };
    const local = { lang: "ja-JP", localService: true };
    const f = setup([remote, { lang: "ja", localService: true }, local]);
    expect(f.speech.enable(START_MESSAGE)).toBe("requested");
    expect(f.adapter.speak).toHaveBeenCalledWith(START_MESSAGE, local, expect.anything());
    f.events().onStart(); f.events().onEnd();
    f.advance(10000);
    f.speech.request("ahead");
    expect(f.adapter.speak).toHaveBeenLastCalledWith(ALERT_MESSAGES.ahead, local, expect.anything());
  });
  it.each([
    [[{ lang: "ja", localService: true }], true],
    [[{ lang: "ja-JP", localService: false }], true],
    [[{ lang: "en-US", localService: true }], false],
    [[], false],
  ] as [SpeechVoice[], boolean][])("日本語音声だけを採用する %j", (voices, supported) => {
    const f = setup(voices);
    expect(f.speech.enable(START_MESSAGE)).toBe(supported ? "requested" : "screen-only");
    if (!supported) expect(f.unavailable).toHaveBeenCalledWith(SPEECH_UNAVAILABLE);
  });
  it("非対応や例外は画面通知へ移行し呼出元へ例外を投げない", () => {
    const unavailable = vi.fn();
    expect(createSpeech({ adapter: null, onUnavailable: unavailable }).enable(START_MESSAGE)).toBe("screen-only");
    expect(unavailable).toHaveBeenCalledWith("音声を利用できません。画面通知を使用します");
    const f = setup();
    vi.mocked(f.adapter.speak).mockImplementation(() => { throw new Error("failed"); });
    expect(f.speech.enable(START_MESSAGE)).toBe("screen-only");
    expect(f.unavailable).toHaveBeenCalledOnce();
    expect(f.speech.request("ahead")).toBe("screen-only");
    expect(f.adapter.speak).toHaveBeenCalledTimes(1);
  });
  it("8倍でも開始待ちは実時間3秒で、0.375秒では取消しない", () => {
    const f = setup(); f.speech.enable(START_MESSAGE);
    f.advance(375);
    expect(f.adapter.cancel).not.toHaveBeenCalled();
    f.advance(2624);
    expect(f.unavailable).not.toHaveBeenCalled();
    f.advance(1);
    expect(f.adapter.cancel).toHaveBeenCalledOnce();
    expect(f.unavailable).toHaveBeenCalledOnce();
    f.advance(20000);
    expect(f.adapter.speak).toHaveBeenCalledOnce();
  });
  it("8倍でも終了待ちはonstartから実時間15秒で、1.875秒では取消しない", () => {
    const f = setup(); f.speech.enable(START_MESSAGE); f.advance(2000); f.events().onStart();
    f.advance(1875);
    expect(f.adapter.cancel).not.toHaveBeenCalled();
    f.advance(13124);
    expect(f.unavailable).not.toHaveBeenCalled();
    f.advance(1);
    expect(f.unavailable).toHaveBeenCalledOnce();
  });
  it("発話中・開始待機中はキューを積まず、実時間10秒未満の要求も捨てる", () => {
    const f = setup(); f.speech.enable(START_MESSAGE);
    expect(f.speech.request("multiple")).toBe("screen-only");
    f.advance(2000); f.events().onStart();
    expect(f.speech.request("ahead")).toBe("screen-only");
    f.events().onEnd();
    f.advance(9999);
    expect(f.speech.request("ahead")).toBe("screen-only");
    f.advance(1);
    expect(f.adapter.speak).toHaveBeenCalledOnce();
    expect(f.speech.request("multiple")).toBe("requested");
    expect(f.adapter.speak).toHaveBeenLastCalledWith(ALERT_MESSAGES.multiple, expect.anything(), expect.anything());
  });
  it.each<SpeechCancelReason>(["accuracy", "pending", "deviation", "stop", "pause", "hidden", "pagehide", "unmount"])("%sは待機・発話・タイマーを取消し古いイベントを無視する", reason => {
    for (const started of [false, true]) {
      const f = setup(); f.speech.enable(START_MESSAGE);
      const stale = f.events();
      if (started) stale.onStart();
      f.speech.cancel(reason);
      stale.onStart(); stale.onEnd(); stale.onError();
      f.advance(30000);
      expect(f.adapter.cancel).toHaveBeenCalledOnce();
      expect(f.unavailable).not.toHaveBeenCalled();
      expect(vi.getTimerCount()).toBe(0);
    }
  });
  it("error後や音声一覧の変更だけでは再有効化・自動再発話しない", () => {
    const voices: SpeechVoice[] = [];
    const f = setup(voices); f.speech.enable(START_MESSAGE);
    voices.push({ lang: "ja-JP", localService: true });
    expect(f.speech.request("ahead")).toBe("screen-only");
    expect(f.speech.enable(START_MESSAGE)).toBe("requested");
    f.events().onError();
    f.advance(20000);
    expect(f.speech.request("ahead")).toBe("screen-only");
    expect(f.adapter.speak).toHaveBeenCalledOnce();
  });
  it("同期onstart/onendと遅い二重イベントでもタイマーを残さない", () => {
    const f = setup();
    vi.mocked(f.adapter.speak).mockImplementation((_text, _voice, events) => {
      events.onStart(); events.onStart(); events.onEnd(); events.onError();
    });
    expect(f.speech.enable(START_MESSAGE)).toBe("requested");
    f.advance(20000);
    expect(f.unavailable).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });
  it("ブラウザアダプタはja-JPで設定しイベントを委譲する", () => {
    const voice = { lang: "ja-JP", localService: true } as SpeechSynthesisVoice;
    const synth = { getVoices: () => [voice], speak: vi.fn(), cancel: vi.fn() };
    const utterance = { lang: "", text: "" } as SpeechSynthesisUtterance;
    const adapter = createBrowserSpeechAdapter(synth, text => { utterance.text = text; return utterance; })!;
    const events = { onStart: vi.fn(), onEnd: vi.fn(), onError: vi.fn() };
    expect(adapter.getVoices()).toEqual([voice]);
    adapter.speak(ALERT_MESSAGES.ahead, voice, events);
    expect(utterance.lang).toBe("ja-JP"); expect(utterance.voice).toBe(voice);
    expect(utterance.text).toBe(ALERT_MESSAGES.ahead);
    utterance.onstart?.call(utterance, {} as SpeechSynthesisEvent);
    utterance.onend?.call(utterance, {} as SpeechSynthesisEvent);
    utterance.onerror?.call(utterance, {} as SpeechSynthesisErrorEvent);
    expect(events.onStart).toHaveBeenCalledOnce(); expect(events.onEnd).toHaveBeenCalledOnce(); expect(events.onError).toHaveBeenCalledOnce();
    adapter.cancel(); expect(synth.cancel).toHaveBeenCalledOnce();
  });
  it.each([1, 2, 4, 8])("%i倍でも位置の5秒確認は仮想時計、音声の失敗後も位置通知を続ける", speed => {
    const f = setup();
    f.speech.enable(START_MESSAGE);
    const deviation = createDeviation();
    [0, 5000, 10000].forEach(t => deviation.update({ timestampMs: t, accuracyM: 10, distanceToRouteM: 120 }));
    [11000, 13500].forEach(t => expect(deviation.update({ timestampMs: t, accuracyM: 10, distanceToRouteM: 0, matchConfirmed: true }).deviated).toBe(true));
    expect(deviation.update({ timestampMs: 16000, accuracyM: 10, distanceToRouteM: 0, matchConfirmed: true }).deviated).toBe(false);
    f.advance(5000 / speed);
    if (speed >= 2) expect(f.unavailable).not.toHaveBeenCalled();
    f.events().onError();
    const alerts = createAlerts("walk", [{ id: "a", start_m: 200, end_m: 250, start_segment_index: 0, end_segment_index: 0, severity_weight: 3 }]);
    alerts.update({ timestampMs: 16000, progressM: 120, progressing: true });
    const notification = alerts.update({ timestampMs: 17000, progressM: 120, progressing: true }).notification;
    expect(notification?.sectionIds).toEqual(["a"]);
    expect(f.speech.request("ahead")).toBe("screen-only");
    alerts.suspend();
    alerts.update({ timestampMs: 18000, progressM: 120, progressing: true });
    expect(alerts.update({ timestampMs: 19000, progressM: 120, progressing: true }).notification).toBeNull();
  });

});
