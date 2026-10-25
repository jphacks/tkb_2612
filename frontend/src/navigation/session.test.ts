import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { NavigationSession, type NavigationSnapshot } from "./session";
import type { SpeechAdapter, SpeechVoice } from "./speech";
import { localPosition, testRoute } from "./test-utils";

type Success = (position: GeolocationPosition) => void;
type Failure = (error: GeolocationPositionError) => void;

function setup(options: { sections?: { start_m: number; end_m: number }[]; voices?: SpeechVoice[]; hangAfterStart?: boolean } = {}) {
  const route = testRoute([0, 3000], 2000);
  route.navigation!.sections = (options.sections ?? []).map((section, index) => ({
    id: `route-1:nav-${index}-${index}`,
    start_segment_index: index,
    end_segment_index: index,
    severity_weight: 3,
    ...section,
  }));
  route.high_risk_total = route.navigation!.sections.length;
  let watch: { success: Success; failure: Failure } | null = null;
  const geolocation = {
    watchPosition: vi.fn((success: Success, failure: Failure) => {
      watch = { success, failure };
      return 1;
    }),
    clearWatch: vi.fn(() => {
      watch = null;
    }),
  };
  const voice: SpeechVoice = { lang: "ja-JP", localService: true };
  const spoken: string[] = [];
  const speech: SpeechAdapter = {
    getVoices: () => options.voices ?? [voice],
    speak: vi.fn((text: string, _voice: SpeechVoice, events) => {
      spoken.push(text);
      events.onStart();
      // 開始後に終了しない発話は、取消されるまで発話中として残る。
      if (!(options.hangAfterStart && spoken.length > 1)) events.onEnd();
    }),
    cancel: vi.fn(),
  };
  const snapshots: NavigationSnapshot[] = [];
  const session = new NavigationSession({
    route,
    selection: "fastest",
    mode: "car",
    kind: "device",
    startMessage: "ナビを開始します",
    deps: { geolocation, speech, wakeLock: undefined },
    onChange: (snapshot) => snapshots.push(snapshot),
  });
  return {
    route,
    session,
    geolocation,
    speech,
    spoken,
    snapshots,
    last: () => session.getSnapshot(),
    feed(progress: number, accuracy = 10, lateral = 0) {
      vi.advanceTimersByTime(1000);
      watch?.success({
        coords: { ...{ longitude: 0, latitude: 0 }, ...coordsOf(progress, lateral), accuracy },
        timestamp: Date.now(),
      } as GeolocationPosition);
    },
  };
}
function coordsOf(progress: number, lateral: number) {
  const p = localPosition(progress, lateral);
  return { longitude: p.lon, latitude: p.lat };
}

beforeEach(() => {
  vi.stubEnv("VITE_ENABLE_SIMULATION", "true");
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-10-25T03:00:00Z"));
});
afterEach(() => {
  vi.unstubAllEnvs();
  vi.useRealTimers();
});

describe("NavigationSession: 接近通知と音声の結線（手順10）", () => {
  it("開始ボタンの操作内で音声を有効化し、開始の定型文を1回だけ発話する", () => {
    const { session, spoken } = setup();
    session.begin();
    expect(spoken).toEqual(["ナビを開始します"]);
    session.stop();
  });

  it("同時に接近した複数区間は件数を示し、音声は1回にまとめる。通知済みは一時停止・再開で消さない", () => {
    const { session, feed, spoken, last } = setup({
      sections: [{ start_m: 600, end_m: 650 }, { start_m: 600, end_m: 680 }],
    });
    session.begin();
    vi.advanceTimersByTime(11_000); // 開始の発話から実時間10秒以上あける
    // 速い移動で接近範囲(300m)の手前まで進める。
    for (let p = 0; p <= 240; p += 40) feed(p);
    expect(last().alertIds).toEqual([]);
    feed(330);
    feed(340);
    expect(last().alertIds).toHaveLength(2);
    expect(last().alertMessage).toBe("この先に、過去事故の参考区間が続きます");
    expect(spoken.filter((text) => text.includes("この先に"))).toEqual([
      "この先に、過去事故の参考区間が続きます",
    ]);
    session.pause();
    expect(last().alertIds).toHaveLength(2);
    session.resume();
    expect(last().alertIds).toHaveLength(2);
    session.stop();
  });

  it("区間内で開始・確認したときは参考区間にいますと表示する", () => {
    const { session, feed, last } = setup({ sections: [{ start_m: 100, end_m: 400 }] });
    session.begin();
    for (let i = 0; i < 9; i++) feed(150 + i * 3);
    expect(last().alertMessage).toBe("過去事故の参考区間にいます");
    session.stop();
  });

  it("精度不足になると音声を取り消し、画面通知は残す", () => {
    const { session, feed, speech, last } = setup({ sections: [{ start_m: 100, end_m: 400 }], hangAfterStart: true });
    session.begin();
    vi.advanceTimersByTime(11_000); // 開始の発話から実時間10秒以上あける
    for (let i = 0; i < 9; i++) feed(150 + i * 3);
    expect(speech.speak).toHaveBeenCalledTimes(2);
    vi.mocked(speech.cancel).mockClear();
    feed(180, 80);
    expect(last().phase).toBe("poor-accuracy");
    expect(speech.cancel).toHaveBeenCalled();
    expect(last().alertMessage).not.toBeNull();
    session.stop();
  });

  it("日本語音声がなければ音声を使わず画面通知に切り替える旨を示す", () => {
    const { session, last } = setup({ voices: [{ lang: "en-US", localService: true }] });
    session.begin();
    expect(last().speechNotice).toBe("音声を利用できません。画面通知を使用します");
    session.stop();
  });
});

describe("NavigationSession: 模擬走行の停止期限と明示再開（7.7）", () => {
  function simulate(inject?: NonNullable<ConstructorParameters<typeof NavigationSession>[0]["inject"]>) {
    const route = testRoute([0, 600], 100);
    const session = new NavigationSession({
      route,
      selection: "fastest",
      mode: "walk",
      kind: "simulation",
      simulation: { seed: 1, multiplier: 1, jitter: true },
      startMessage: "模擬走行を開始します。実際の現在地ではありません",
      deps: { speech: null, wakeLock: undefined },
      inject,
      onChange: () => {},
    });
    return session;
  }
  const run = (seconds: number) => {
    for (let i = 0; i < seconds * 4; i++) vi.advanceTimersByTime(250);
  };

  it("精度不足が続くと基準位置が終点に着いて15仮想秒で理由付きに停止し、到着にはせず、明示再開で再び進む", () => {
    const session = simulate((fix, seconds) => (seconds >= 10 ? { ...fix, accuracy: 50 } : fix));
    session.begin();
    run(9);
    expect(session.getSnapshot().phase).toBe("tracking");
    run(110);
    const stopped = session.getSnapshot();
    expect(stopped.phase).toBe("paused");
    expect(stopped.pauseReason).toBe("sim-limit");
    expect(stopped.simStopReason).toBe("精度不足");
    expect(stopped.match.status).not.toBe("arrived");
    const frozen = stopped.lastFix;
    run(30);
    expect(session.getSnapshot().lastFix).toEqual(frozen);
    session.resume();
    expect(session.getSnapshot().phase).toBe("waiting");
    run(3);
    expect(session.getSnapshot().pauseReason).toBeNull();
    session.stop();
  });

  it("倍率変更・揺らぎ変更・一時停止・再開を受け付け、一時停止中は仮想時刻を進めない", () => {
    const session = simulate();
    session.begin();
    run(10);
    session.setMultiplier(4);
    session.setJitter(false);
    expect(session.getSnapshot().simulation).toMatchObject({ multiplier: 4, jitter: false, seed: 1 });
    run(3);
    session.pause();
    const paused = session.getSnapshot();
    expect(paused.phase).toBe("paused");
    expect(paused.pauseReason).toBe("user");
    run(60);
    expect(session.getSnapshot().lastFix).toEqual(paused.lastFix);
    session.resume();
    expect(session.getSnapshot().simulation?.running).toBe(true);
    session.stop();
  });
});

describe("NavigationSession: 現在地からの再検索の案内（7.5）", () => {
  it("開始直後の位置確認では案内せず、照合保留が15秒続いたときと逸脱中に案内する", () => {
    const { session, feed, last } = setup();
    session.begin();
    for (let i = 0; i < 4; i++) feed(100 + i);
    expect(last().phase).toBe("matching-hold");
    expect(last().reSearchAvailable).toBe(false);
    for (let i = 0; i < 8; i++) feed(110 + i);
    expect(last().phase).toBe("tracking");
    expect(last().reSearchAvailable).toBe(false);
    // 経路から25m離れると、位置を一意に確認できず保留になる。
    for (let i = 0; i < 12; i++) feed(120, 10, 25);
    expect(last().phase).toBe("matching-hold");
    expect(last().reSearchAvailable).toBe(false);
    for (let i = 0; i < 5; i++) feed(120, 10, 25);
    expect(last().reSearchAvailable).toBe(true);
    // 経路へ戻って進行中になれば案内は消える。
    for (let i = 0; i < 9; i++) feed(125 + i);
    expect(last().phase).toBe("tracking");
    expect(last().reSearchAvailable).toBe(false);
    // 逸脱は10秒・3測位で確定し、その間は案内する。
    for (let i = 0; i < 13; i++) feed(135, 10, 120);
    expect(last().phase).toBe("deviated");
    expect(last().reSearchAvailable).toBe(true);
    session.stop();
  });
});
