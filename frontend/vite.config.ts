import { defineConfig } from "vitest/config";
import { loadEnv } from "vite";
import react from "@vitejs/plugin-react";
import { pwaBuild } from "./scripts/pwa-build";

export default defineConfig(({ mode }) => {
  // 開発サーバーだけで使い、ブラウザーへ公開する環境変数には含めない。
  const env = loadEnv(mode, process.cwd(), "DEV_API_PROXY_TARGET");
  return {
    plugins: [react(), pwaBuild()],
    server: {
      port: 5173,
      strictPort: true,
      proxy: { "/api": env.DEV_API_PROXY_TARGET || "http://127.0.0.1:8000" },
    },
    test: {
      environment: "jsdom",
      setupFiles: ["./src/test/setup.ts"],
      restoreMocks: true,
    },
  };
});
