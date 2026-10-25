import { canReloadForUpdate } from "./policy";

export type UpdateSnapshot = Readonly<{
  waiting: boolean;
  blockedReasons: readonly string[];
  canReload: boolean;
}>;

export function createUpdateStore() {
  const reasons = new Set<string>();
  const listeners = new Set<() => void>();
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
  };
}

const updates = createUpdateStore();
// canReloadは将来の同意UI向けの判定値。解除してもSWはwaitingのまま維持する。
export const getUpdateSnapshot = updates.getSnapshot;
export const subscribeUpdates = updates.subscribe;
export const setUpdateBlocked = updates.setUpdateBlocked;

// 状態を公開するだけで、切替・再読込・利用者への表示は行わない。
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
