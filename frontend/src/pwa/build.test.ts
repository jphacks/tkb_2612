// @vitest-environment node
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { runInNewContext } from "node:vm";
import { build } from "vite";
import { expect, it } from "vitest";
import { pwaBuild } from "../../scripts/pwa-build";

it("生成SWが出力一覧を保存し、同一ビルドでは同じversion、publicアセット変更では別versionになる", async () => {
  const root = await mkdtemp(join(tmpdir(), "acciless-pwa-"));
  try {
    await mkdir(join(root, "public/assets"), { recursive: true });
    await writeFile(join(root, "index.html"), '<title>AcciLessNavi — ルート比較</title><script type="module" src="/app.js"></script>');
    await writeFile(join(root, "app.js"), 'import "./app.css"; document.title = "AcciLessNavi";');
    await writeFile(join(root, "app.css"), 'body { color: black; }');
    await writeFile(join(root, "public/assets/poster.jpg"), "old");
    const compile = async () => {
      await build({ root, configFile: false, logLevel: "silent", plugins: [pwaBuild()] });
      const source = await readFile(join(root, "dist/sw.js"), "utf8");
      const handlers: Record<string, (event: object) => void> = {};
      const cached: string[] = [];
      let version = "";
      const self = {
        location: { origin: "https://app.example" },
        caches: { open: async (name: string) => {
          version = name;
          return { put: async (path: string) => { cached.push(path); } };
        } },
        addEventListener: (name: string, callback: (event: object) => void) => { handlers[name] = callback; },
        fetch: async (url: URL, options: RequestInit) => {
          expect(url.origin).toBe("https://app.example");
          expect(options).toMatchObject({ redirect: "error", credentials: "same-origin" });
          const path = url.pathname === "/" ? "index.html" : url.pathname.slice(1);
          return new Response(await readFile(join(root, "dist", path)));
        },
      };
      runInNewContext(source, { self, URL });
      let installed: Promise<unknown> | undefined;
      handlers.install({ waitUntil: (promise: Promise<unknown>) => { installed = promise; } });
      await installed;
      expect(cached).toContain("/");
      expect(cached).toContain("/assets/poster.jpg");
      expect(cached).toContain("/manifest.webmanifest");
      expect(cached.some((path) => /^\/assets\/.*\.js$/.test(path))).toBe(true);
      expect(cached.some((path) => /^\/assets\/.*\.css$/.test(path))).toBe(true);
      expect(cached).not.toContain("/sw.js");
      return version;
    };
    const first = await compile();
    expect(await compile()).toBe(first);
    await writeFile(join(root, "public/assets/poster.jpg"), "new");
    expect(await compile()).not.toBe(first);
    expect(JSON.parse(await readFile(join(root, "dist/manifest.webmanifest"), "utf8")))
      .toEqual({ name: "AcciLessNavi — ルート比較", short_name: "AcciLessNavi", start_url: "/", scope: "/", display: "standalone" });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
