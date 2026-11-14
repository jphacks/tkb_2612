import { describe, expect, it, vi } from "vitest";
import { createUpdateStore, registerServiceWorker } from "./register";

function fakeServiceWorker() {
  const registration = Object.assign(new EventTarget(), {
    waiting: null as ServiceWorker | null,
    installing: null as ServiceWorker | null,
  });
  const register = vi.fn().mockResolvedValue(registration);
  const container = Object.assign(new EventTarget(), { register, controller: {} }) as unknown as ServiceWorkerContainer;
  const worker = Object.assign(new EventTarget(), {
    state: "installing", postMessage: vi.fn(),
  });
  return { registration, register, container, worker };
}

describe("SW登録と更新待機", () => {
  it("dev と非対応環境では登録しない", async () => {
    const fake = fakeServiceWorker();
    await registerServiceWorker(false, fake.container, createUpdateStore());
    await registerServiceWorker(true, undefined, createUpdateStore());
    expect(fake.register).not.toHaveBeenCalled();
  });
  it("本番ではルートscopeで登録し、既存のwaitingを通知する", async () => {
    const fake = fakeServiceWorker();
    fake.registration.waiting = fake.worker as unknown as ServiceWorker;
    const store = createUpdateStore();
    await registerServiceWorker(true, fake.container, store);
    expect(fake.register).toHaveBeenCalledWith("/sw.js", { scope: "/", updateViaCache: "none" });
    expect(store.getSnapshot()).toEqual({ waiting: true, blockedReasons: [], canReload: true });
    expect(fake.worker.postMessage).not.toHaveBeenCalled();
  });
  it("初回インストールは更新通知せず、後のwaitingを購読者に通知する", async () => {
    const fake = fakeServiceWorker();
    const store = createUpdateStore();
    const listener = vi.fn();
    const unsubscribe = store.subscribe(listener);
    await registerServiceWorker(true, fake.container, store);
    fake.registration.installing = fake.worker as unknown as ServiceWorker;
    fake.registration.dispatchEvent(new Event("updatefound"));
    fake.worker.state = "installed";
    fake.worker.dispatchEvent(new Event("statechange"));
    expect(store.getSnapshot().waiting).toBe(false);
    fake.registration.waiting = fake.worker as unknown as ServiceWorker;
    fake.worker.dispatchEvent(new Event("statechange"));
    expect(store.getSnapshot().waiting).toBe(true);
    expect(listener).toHaveBeenCalledTimes(1);
    unsubscribe();
    fake.worker.state = "redundant";
    fake.registration.waiting = null;
    fake.worker.dispatchEvent(new Event("statechange"));
    expect(store.getSnapshot().waiting).toBe(false);
    expect(listener).toHaveBeenCalledTimes(1);
    expect(fake.worker.postMessage).not.toHaveBeenCalled();
  });
  it("禁止理由を独立して管理し、解除しても自動切替しない", async () => {
    const store = createUpdateStore();
    store.setUpdateBlocked("navigation", true);
    store.setUpdateBlocked("gps", true);
    store.setWaiting(true);
    expect(store.getSnapshot().canReload).toBe(false);
    store.setUpdateBlocked("navigation", false);
    expect(store.getSnapshot().blockedReasons).toEqual(["gps"]);
    expect(store.getSnapshot().canReload).toBe(false);
    store.setUpdateBlocked("gps", false);
    expect(store.getSnapshot()).toEqual({ waiting: true, blockedReasons: [], canReload: true });
  });
  it("登録失敗を吸収し、通常Webの起動を妨げない", async () => {
    const fake = fakeServiceWorker();
    fake.register.mockRejectedValue(new Error("offline"));
    const store = createUpdateStore();
    await expect(registerServiceWorker(true, fake.container, store)).resolves.toBeUndefined();
    expect(store.getSnapshot().waiting).toBe(false);
  });
});

describe("同意した再読込での切替", () => {
  async function waitingSetup() {
    const fake = fakeServiceWorker();
    fake.worker.state = "installed";
    fake.registration.waiting = fake.worker as unknown as ServiceWorker;
    const store = createUpdateStore();
    await registerServiceWorker(true, fake.container, store);
    return { ...fake, store, reload: vi.fn() };
  }
  it("waitingへskip-waitingを送り、controllerchange後に要求元だけ再読込する", async () => {
    const fake = await waitingSetup();
    const result = fake.store.applyUpdate(fake.reload);
    expect(fake.worker.postMessage).toHaveBeenCalledWith({ type: "skip-waiting" });
    expect(fake.reload).not.toHaveBeenCalled();
    fake.container.dispatchEvent(new Event("controllerchange"));
    await expect(result).resolves.toBe("reloading");
    expect(fake.reload).toHaveBeenCalledTimes(1);
    fake.container.dispatchEvent(new Event("controllerchange"));
    expect(fake.reload).toHaveBeenCalledTimes(1);
  });
  it("検索・ナビ中は送らない", async () => {
    const fake = await waitingSetup();
    fake.store.setUpdateBlocked("navigation", true);
    await expect(fake.store.applyUpdate(fake.reload)).resolves.toBe("blocked");
    expect(fake.worker.postMessage).not.toHaveBeenCalled();
    expect(fake.reload).not.toHaveBeenCalled();
  });
  it("ほかのタブがあるとSWに断られ、再読込しない", async () => {
    const fake = await waitingSetup();
    const result = fake.store.applyUpdate(fake.reload);
    fake.container.dispatchEvent(new MessageEvent("message", { data: { type: "skip-waiting-refused" } }));
    await expect(result).resolves.toBe("other-windows");
    fake.container.dispatchEvent(new Event("controllerchange"));
    expect(fake.reload).not.toHaveBeenCalled();
  });
  it("waitingが置き換わったら切替を諦める", async () => {
    const fake = await waitingSetup();
    const result = fake.store.applyUpdate(fake.reload);
    fake.worker.state = "redundant";
    fake.worker.dispatchEvent(new Event("statechange"));
    await expect(result).resolves.toBe("unavailable");
    expect(fake.reload).not.toHaveBeenCalled();
  });
});
