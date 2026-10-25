// @vitest-environment node
import { describe, expect, it, vi } from "vitest";
import { installWorker, type WorkerScope } from "./worker";
import { isShellRequest, oldShellCaches } from "./policy";

function setup(fail = false) {
  const handlers = new Map<string, (event: never) => void>();
  const entries = new Map<string, Response>();
  const deleted: string[] = [];
  const put = vi.fn(async (url: string, response: Response) => { entries.set(url, response); });
  const scope = {
    location: { origin: "https://app.example" },
    addEventListener: (type: string, listener: (event: never) => void) => handlers.set(type, listener),
    caches: {
      open: async () => ({ put,
        match: async (request: Request) => entries.get(new URL(request.url).pathname) }),
      keys: async () => ["acciless-navi-shell-old", "acciless-navi-shell-new", "unrelated"],
      delete: async (name: string) => { deleted.push(name); return true; },
    },
    fetch: vi.fn(async () => fail ? new Response("failed", { status: 503 }) : new Response("shell")),
  };
  installWorker(scope as unknown as WorkerScope, "acciless-navi-shell-new", ["/", "/assets/app.js", "/assets/acciless-navi.webm"], isShellRequest, oldShellCaches);
  function dispatch(type: string, event: object) { handlers.get(type)!((event as never)); }
  async function lifecycle(type: string) {
    let result: Promise<unknown> | undefined;
    dispatch(type, { waitUntil: (promise: Promise<unknown>) => { result = promise; } });
    await result;
  }
  return { scope, entries, deleted, dispatch, lifecycle, handlers, put };
}

describe("SWのライフサイクル", () => {
  it("シェルだけを保存し、activateまでは旧キャッシュを残す", async () => {
    const fake = setup();
    await fake.lifecycle("install");
    expect([...fake.entries.keys()]).toEqual(["/", "/assets/app.js", "/assets/acciless-navi.webm"]);
    expect(fake.deleted).toEqual([]);
    await fake.lifecycle("activate");
    expect(fake.deleted).toEqual(["acciless-navi-shell-old"]);
  });
  it("取得失敗はインストール失敗とし不完全な新cacheを消す", async () => {
    const fake = setup(true);
    await expect(fake.lifecycle("install")).rejects.toThrow();
    expect(fake.deleted).toEqual(["acciless-navi-shell-new"]);
  });
  it.each(["https://app.example/api/health", "https://tiles.example/tile", "https://app.example/?address=secret"])("%s はSWで応答も保存もしない", (url) => {
    const fake = setup();
    const respondWith = vi.fn();
    fake.dispatch("fetch", { request: new Request(url), respondWith });
    expect(respondWith).not.toHaveBeenCalled();
    expect(fake.entries.size).toBe(0);
    expect(fake.scope.fetch).not.toHaveBeenCalled();
  });
  it("オフラインでは同じversionのシェルを返し、実行時の書込みをしない", async () => {
    const fake = setup();
    await fake.lifecycle("install");
    fake.scope.fetch.mockRejectedValue(new Error("offline"));
    let response: Promise<Response> | undefined;
    fake.dispatch("fetch", { request: navigationRequest(),
      respondWith: (value: Promise<Response>) => { response = value; } });
    expect(await (await response)!.text()).toBe("shell");
    expect(fake.scope.fetch).toHaveBeenCalledTimes(4);
    expect(fake.entries.size).toBe(3);
    expect(fake.put).toHaveBeenCalledTimes(3);
    expect([...fake.handlers.keys()]).toEqual(["install", "activate", "fetch"]);
  });
});

// Nodeではnavigateモードを構築できないため、ブラウザのnavigation要求を再現する。
function navigationRequest(): Request {
  const request = new Request("https://app.example/");
  Object.defineProperty(request, "mode", { value: "navigate" });
  return request;
}

function fetchResponse(fake: ReturnType<typeof setup>, request: Request): Promise<Response> {
  let response: Promise<Response> | undefined;
  fake.dispatch("fetch", { request, respondWith: (value: Promise<Response>) => { response = value; } });
  if (!response) throw new Error("SWが要求を処理しませんでした");
  return response;
}

describe("navigationの再認証", () => {
  it.each([
    ["認証成功", () => new Response("online shell")],
    ["認証リダイレクト", () => Response.redirect("https://accounts.example/login", 302)],
    ["認証リダイレクトの不透明応答", () => {
      const response = new Response(null, { status: 302 });
      Object.defineProperty(response, "type", { value: "opaqueredirect" });
      return response;
    }],
    ["期限切れ401", () => new Response("unauthorized", { status: 401 })],
    ["アクセス拒否403", () => new Response("forbidden", { status: 403 })],
    ["HTMLのログイン画面", () => new Response("<html>login</html>", { headers: { "Content-Type": "text/html" } })],
    ["サーバ障害", () => new Response("unavailable", { status: 503 })],
  ] as const)("%sをシェルに置き換えず返し、キャッシュには保存しない", async (_, makeResponse) => {
    const fake = setup();
    await fake.lifecycle("install");
    const cached = fake.entries.get("/");
    const online = makeResponse();
    fake.scope.fetch.mockResolvedValue(online);
    const request = navigationRequest();
    expect(await fetchResponse(fake, request)).toBe(online);
    expect(fake.scope.fetch).toHaveBeenLastCalledWith(request, { cache: "no-store" });
    expect(fake.entries.get("/")).toBe(cached);
    expect(fake.put).toHaveBeenCalledTimes(3);
  });
  it("オフラインでシェルもない場合は通信失敗を返す", async () => {
    const fake = setup();
    const error = new Error("offline");
    fake.scope.fetch.mockRejectedValue(error);
    await expect(fetchResponse(fake, navigationRequest())).rejects.toBe(error);
    expect(fake.put).not.toHaveBeenCalled();
  });
});

describe("動画のRange要求", () => {
  it.each([
    ["bytes=0-1", 206, "bytes 0-1/10", "ab"],
    ["bytes=10-", 416, "bytes */10", ""],
  ] as const)("%sをネットワークへ通し、サーバの%dをそのまま返す", async (range, status, contentRange, body) => {
    const fake = setup();
    await fake.lifecycle("install");
    const partial = new Response(body, { status, headers: { "Content-Range": contentRange } });
    fake.scope.fetch.mockResolvedValue(partial);
    const request = new Request("https://app.example/assets/acciless-navi.webm", { headers: { Range: range } });
    const response = await fetchResponse(fake, request);
    expect(response.status).toBe(status);
    expect(response.headers.get("Content-Range")).toBe(contentRange);
    expect(await response.text()).toBe(body);
    expect(fake.scope.fetch).toHaveBeenLastCalledWith(request);
    expect(fake.put).toHaveBeenCalledTimes(3);
  });
  it("オフラインのRange要求には保存済み全文200を代用しない", async () => {
    const fake = setup();
    await fake.lifecycle("install");
    const error = new Error("offline");
    fake.scope.fetch.mockRejectedValue(error);
    const request = new Request("https://app.example/assets/acciless-navi.webm", { headers: { Range: "bytes=0-1" } });
    await expect(fetchResponse(fake, request)).rejects.toBe(error);
    expect(fake.put).toHaveBeenCalledTimes(3);
  });
  it("Rangeなしならオフラインでも保存済み動画を返す", async () => {
    const fake = setup();
    await fake.lifecycle("install");
    fake.scope.fetch.mockRejectedValue(new Error("offline"));
    const response = await fetchResponse(fake, new Request("https://app.example/assets/acciless-navi.webm"));
    expect(await response.text()).toBe("shell");
    expect(fake.put).toHaveBeenCalledTimes(3);
  });
});
