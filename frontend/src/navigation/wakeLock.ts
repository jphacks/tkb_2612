export interface WakeLockSentinelLike {
  addEventListener(type: "release", listener: () => void): void;
  release(): Promise<void>;
}
export interface WakeLockApi {
  request(type: "screen"): Promise<WakeLockSentinelLike>;
}
export interface ScreenWakeLock {
  /** 明示的な開始/再開のときだけ呼ぶ。未対応・拒否はfalse。自動の再取得はしない。 */
  acquire(): Promise<boolean>;
  release(): void;
}

/** frontend-spec 7.8: Screen Wake Lockは機能検出し、失敗しても例外を投げない。 */
export function createScreenWakeLock(
  api: WakeLockApi | undefined,
  onReleased: () => void,
): ScreenWakeLock {
  let sentinel: WakeLockSentinelLike | null = null;
  let generation = 0;
  return {
    async acquire() {
      const mine = ++generation;
      if (!api) return false;
      try {
        const next = await api.request("screen");
        if (mine !== generation) {
          void next.release().catch(() => {});
          return false;
        }
        sentinel = next;
        next.addEventListener("release", () => {
          if (sentinel === next && mine === generation) {
            sentinel = null;
            onReleased();
          }
        });
        return true;
      } catch {
        return false;
      }
    },
    release() {
      generation++;
      const old = sentinel;
      sentinel = null;
      if (old) void old.release().catch(() => {});
    },
  };
}
