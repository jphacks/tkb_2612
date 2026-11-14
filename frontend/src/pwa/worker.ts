import type { isShellRequest, oldShellCaches } from "./policy";

type LifetimeEvent = { waitUntil(promise: Promise<unknown>): void };
type WorkerEvents = {
  install: LifetimeEvent;
  activate: LifetimeEvent;
  fetch: { request: Request; respondWith(response: Promise<Response>): void };
  message: LifetimeEvent & {
    data: unknown;
    source: { id: string; postMessage(message: unknown): void } | null;
  };
};
export interface WorkerScope {
  location: { origin: string };
  caches: CacheStorage;
  fetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response>;
  clients: {
    matchAll(options: { type: "window"; includeUncontrolled: boolean }): Promise<readonly { id: string }[]>;
  };
  skipWaiting(): Promise<void>;
  addEventListener<K extends keyof WorkerEvents>(type: K, listener: (event: WorkerEvents[K]) => void): void;
}

// ビルド時に関数本体を出力するため、依存はすべて引数で受け取る。
export function installWorker(
  scope: WorkerScope, current: string, assets: readonly string[],
  accepts: typeof isShellRequest, obsolete: typeof oldShellCaches,
): void {
  scope.addEventListener("install", (event) => {
    event.waitUntil((async () => {
      try {
        const cache = await scope.caches.open(current);
        // 認証リダイレクトや失敗応答をシェルとして保存しない。
        const responses = await Promise.all(assets.map(async (path) => {
          const response = await scope.fetch(new URL(path, scope.location.origin), {
            credentials: "same-origin", cache: "reload", redirect: "error",
          });
          if (!response.ok || response.redirected || response.type === "opaque") {
            throw new Error("シェルを取得できませんでした");
          }
          return response;
        }));
        await Promise.all(assets.map((path, index) => cache.put(path, responses[index])));
      } catch (error) {
        await scope.caches.delete(current);
        throw error;
      }
    })());
  });
  // 既存タブが閉じられ、自然にactivateされるまで旧シェルを維持する。
  scope.addEventListener("activate", (event) => {
    event.waitUntil((async () => {
      const names = obsolete(await scope.caches.keys(), current);
      await Promise.all(names.map((name) => scope.caches.delete(name)));
    })());
  });
  // 利用者が同意した再読込でだけ切り替える。ほかのタブがあると、ナビ中の画面まで
  // 強制切替してしまうため断り、要求元へ理由を返す。
  scope.addEventListener("message", (event) => {
    const data = event.data as { type?: unknown } | null;
    const source = event.source;
    if (data?.type !== "skip-waiting" || !source) return;
    event.waitUntil((async () => {
      const windows = await scope.clients.matchAll({ type: "window", includeUncontrolled: true });
      if (windows.some((client) => client.id !== source.id)) {
        source.postMessage({ type: "skip-waiting-refused" });
        return;
      }
      await scope.skipWaiting();
    })());
  });
  scope.addEventListener("fetch", (event) => {
    const request = event.request;
    if (!accepts(request.url, request.method, scope.location.origin, assets)) return;
    event.respondWith((async () => {
      if (request.mode === "navigate") {
        try {
          // 再読込をIAPへ届け、認証応答もそのまま返す。HTTPキャッシュも使わない。
          return await scope.fetch(request, { cache: "no-store" });
        } catch (error) {
          // HTTPエラーや認証応答ではなく、通信不能の場合だけシェルへ戻す。
          const cache = await scope.caches.open(current);
          const shell = await cache.match(request);
          if (shell) return shell;
          throw error;
        }
      }
      // 範囲処理は配信サーバに任せ、オフラインでも全文200で代用しない。
      if (request.headers.has("Range")) return scope.fetch(request);
      const cache = await scope.caches.open(current);
      // 実行時は書き込まず、ビルド時に列挙したシェルだけを読む。
      return await cache.match(request) ?? scope.fetch(request);
    })());
  });
}
