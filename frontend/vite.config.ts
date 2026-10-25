import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import { pwaBuild } from "./scripts/pwa-build";

export default defineConfig({
  plugins: [react(), pwaBuild()],
  server: { port: 5173, strictPort: true, proxy: { "/api": "http://127.0.0.1:8000" } },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    restoreMocks: true,
  },
});
