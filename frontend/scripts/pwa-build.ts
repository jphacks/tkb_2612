import { createHash } from "node:crypto";
import { readFile, readdir, writeFile } from "node:fs/promises";
import { resolve, relative } from "node:path";
import type { Plugin } from "vite";
import { brandTags, loadBrandAssets, manifestIcons, publicOrigin, type BrandAssets } from "./brand-assets";
import { cacheName, isShellRequest, oldShellCaches } from "../src/pwa/policy";
import { installWorker } from "../src/pwa/worker";

async function listFiles(directory: string): Promise<string[]> {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = await Promise.all(entries.map((entry) => {
    const path = resolve(directory, entry.name);
    return entry.isDirectory() ? listFiles(path) : Promise.resolve([path]);
  }));
  return files.flat().sort();
}

export function pwaBuild(): Plugin {
  let root: string;
  let output: string;
  let origin: string | undefined;
  let brandAssets: Promise<BrandAssets> | undefined;
  const brand = () => brandAssets ??= loadBrandAssets(root);
  return {
    name: "acciless-navi-pwa",
    configResolved(config) {
      root = config.root;
      output = resolve(root, config.build.outDir);
      origin = publicOrigin(config.env.VITE_PUBLIC_ORIGIN);
    },
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        brand().then((assets) => {
          const asset = Object.values(assets).find(({ path }) => path === request.url);
          if (!asset) return next();
          response.setHeader("Content-Type", "image/png");
          response.end(asset.source);
        }, next);
      });
    },
    transformIndexHtml: {
      order: "post",
      handler: async () => brandTags(await brand(), origin),
    },
    async generateBundle() {
      for (const { path, source } of Object.values(await brand())) {
        this.emitFile({ type: "asset", fileName: path.slice(1), source });
      }
    },
    async writeBundle() {
      const assets = await brand();
      const html = await readFile(resolve(root, "index.html"), "utf8");
      const title = html.match(/<title>([^<]+)<\/title>/)?.[1];
      if (!title) throw new Error("既存のアプリ名が見つかりません");
      await writeFile(resolve(output, "manifest.webmanifest"), JSON.stringify({
        name: title, short_name: title.split(" — ")[0],
        start_url: "/", scope: "/", display: "standalone", icons: manifestIcons(assets),
      }, null, 2) + "\n");
      const files = (await listFiles(output)).filter((path) => {
        const name = relative(output, path).replaceAll("\\", "/");
        // OGP画像はクローラー向けなので、利用者の端末へは事前取得しない。
        return name === "index.html" || name === "manifest.webmanifest" ||
          (name.startsWith("assets/") && !name.endsWith(".map") && `/${name}` !== assets.ogp.path);
      });
      const digest = createHash("sha256");
      // SW自体の変更でも別cacheにし、稼働中のシェルを上書きしない。
      digest.update(installWorker.toString()).update(isShellRequest.toString())
        .update(oldShellCaches.toString());
      const shell: string[] = [];
      for (const path of files) {
        const name = relative(output, path).replaceAll("\\", "/");
        digest.update(name).update("\0").update(await readFile(path)).update("\0");
        shell.push(name === "index.html" ? "/" : `/${name}`);
      }
      const current = cacheName(digest.digest("hex"));
      const source = `// 自動生成：アプリシェルのみ。更新は同意した再読込まで待機する。\n` +
        `(${installWorker.toString()})(self, ${JSON.stringify(current)}, ` +
        `${JSON.stringify(shell)}, ${isShellRequest.toString()}, ${oldShellCaches.toString()});\n`;
      await writeFile(resolve(output, "sw.js"), source);
    },
  };
}
