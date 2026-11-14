import { canReloadForUpdate } from "./policy";

export type UpdateSnapshot = Readonly<{
  waiting: boolean;
  blockedReasons: readonly string[];
  canReload: boolean;
}>;

/**
 * reloading: 切替後に再読込する。blocked: 検索・GPS取得・ナビ中のため再読込しない。
 * other-windows: ほかのタブがあり切り替えない。unavailable: 待機中の版が置き換わった。
 * timeout: 待機中の版が応答しない（切替に対応する前の版など）。
 */
export type UpdateOutcome = "reloading" | "blocked" | "other-windows" | "unavailable" | "timeout";
type Apply = (reload: () => void, canReload: () => boolean) => Promise<UpdateOutcome>;

/** 待機中の版がこの時間内に切り替わらなければ諦める。 */
export const SWITCH_TIMEOUT_MS = 10_000;

export function createUpdateStore() {
  const reasons = new Set<string>();
  const listeners = new Set<() => void>();
  let apply: Apply | null = null;
  let snapshot: UpdateSnapshot = Object.freeze({
    waiting: false, blockedReasons: Object.freeze([]), canReload: false,
  });
  function publish(waiting: boolean) {
    const blockedReasons = Object.freeze([...reasons]);
    if (snapshot.waiting === waiting &&
      snapshot.blockedReasons.length === blockedReasons.length &&
      snapshot.blockedReasons.every((reason, index) => reason === blockedReasons[index])) return;
    snapshot = Object.freeze({ waiting, blockedReasons,
      canReload: canReloadForUpdate(waiting, blockedReasons) });
    listeners.forEach((listener) => listener());
  }
  return {
    getSnapshot: (): UpdateSnapshot => snapshot,
    subscribe(listener: () => void): () => void {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
    setUpdateBlocked(reason: string, blocked: boolean): void {
      if (blocked) reasons.add(reason);
      else reasons.delete(reason);
      publish(snapshot.waiting);
    },
    setWaiting(waiting: boolean): void { publish(waiting); },
    bindApply(next: Apply): void { apply = next; },
    // 検索・GPS取得・ナビ中は切り替えない（frontend-spec 8章）。
    async applyUpdate(reload: () => void = () => window.location.reload()): Promise<UpdateOutcome> {
      if (!snapshot.canReload || !apply) return "blocked";
      // 切替中はwaitingがnullになりcanReloadもfalseになるため、禁止理由だけを見る。
      return apply(reload, () => reasons.size === 0);
    },
  };
}

const updates = createUpdateStore();
// 禁止理由が解除されても自動では切り替えず、applyUpdateの呼出しまでwaitingを維持する。
export const getUpdateSnapshot = updates.getSnapshot;
export const subscribeUpdates = updates.subscribe;
export const setUpdateBlocked = updates.setUpdateBlocked;
export const applyUpdate = updates.applyUpdate;

function waitingApplier(registration: ServiceWorkerRegistration, container: ServiceWorkerContainer): Apply {
  return (reload, canReload) => new Promise((resolve) => {
    const waiting = registration.waiting;
    if (!waiting) {
      reload();
      resolve("reloading");
      return;
    }
    const finish = (outcome: UpdateOutcome) => {
      clearTimeout(timer);
      container.removeEventListener("controllerchange", onControllerChange);
      container.removeEventListener("message", onMessage);
      waiting.removeEventListener("statechange", onStateChange);
      if (outcome === "reloading") reload();
      resolve(outcome);
    };
    // 要求したタブだけが再読込する。ほかのタブのcontrollerchangeでは何もしない。
    // 要求後に検索・GPS取得・ナビが始まっていれば、切替だけ受け入れて再読込しない。
    const onControllerChange = () => finish(canReload() ? "reloading" : "blocked");
    const onMessage = (event: MessageEvent) => {
      if ((event.data as { type?: unknown } | null)?.type === "skip-waiting-refused") finish("other-windows");
    };
    // より新しい版に置き換わった場合は、その版のwaitingで改めて案内する。
    const onStateChange = () => { if (waiting.state === "redundant") finish("unavailable"); };
    const timer = setTimeout(() => finish("timeout"), SWITCH_TIMEOUT_MS);
    container.addEventListener("controllerchange", onControllerChange);
    container.addEventListener("message", onMessage);
    waiting.addEventListener("statechange", onStateChange);
    waiting.postMessage({ type: "skip-waiting" });
  });
}

// 登録時は状態を公開するだけで、切替・再読込はapplyUpdateの呼出しまで行わない。
export async function registerServiceWorker(
  production: boolean = import.meta.env.PROD,
  serviceWorker: ServiceWorkerContainer | undefined =
    typeof navigator !== "undefined" ? navigator.serviceWorker : undefined,
  store = updates,
): Promise<void> {
  if (!production || !serviceWorker) return;
  try {
    const registration = await serviceWorker.register("/sw.js", {
      scope: "/", updateViaCache: "none",
    });
    store.bindApply(waitingApplier(registration, serviceWorker));
    const syncWaiting = () => store.setWaiting(registration.waiting !== null);
    const watched = new WeakSet<ServiceWorker>();
    const watch = (worker: ServiceWorker | null) => {
      if (!worker || watched.has(worker)) return;
      watched.add(worker);
      worker.addEventListener("statechange", syncWaiting);
    };
    const watchInstallation = () => {
      watch(registration.installing);
      watch(registration.waiting);
      syncWaiting();
    };
    registration.addEventListener("updatefound", watchInstallation);
    watchInstallation();
  } catch {
    // SWが使えなくても通常Webとして起動する。入力や位置情報を記録しない。
  }
}
