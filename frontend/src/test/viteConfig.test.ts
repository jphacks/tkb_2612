// @vitest-environment node
import { beforeEach, expect, it, vi } from "vitest";
import { loadEnv } from "vite";
import type { UserConfigFnObject } from "vite";
import viteConfig from "../../vite.config";

vi.mock("vite", async (importOriginal) => ({
  ...(await importOriginal<typeof import("vite")>()),
  loadEnv: vi.fn(),
}));

const config = viteConfig as UserConfigFnObject;
// プロセス環境変数と.env*の有無に依存せず、設定への入力を固定する。
beforeEach(() => vi.mocked(loadEnv).mockReturnValue({}));

it("defaults to the local backend without rewriting /api paths", () => {
  expect(config({ mode: "test", command: "serve" }).server?.proxy).toEqual({
    "/api": "http://127.0.0.1:8000",
  });
  expect(loadEnv).toHaveBeenCalledWith(
    "test",
    process.cwd(),
    "DEV_API_PROXY_TARGET",
  );
});

it("uses the server-only proxy override without exposing it through envPrefix or define", () => {
  vi.mocked(loadEnv).mockReturnValue({
    DEV_API_PROXY_TARGET: "http://127.0.0.1:18000",
  });
  const result = config({ mode: "test", command: "serve" });
  expect(result.server?.proxy).toEqual({ "/api": "http://127.0.0.1:18000" });
  expect(result.envPrefix).toBeUndefined();
  expect(result.define).toBeUndefined();
});
