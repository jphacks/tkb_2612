import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

/** `brand/` の原本と、配信名・用途。配信名は内容ハッシュ付き（/assets は immutable で配信される）。 */
const SOURCES = {
  favicon: { file: "acciless-navi-icon-32.png", sizes: "32x32" },
  appleTouch: { file: "acciless-navi-icon-180.png", sizes: "180x180" },
  icon192: { file: "acciless-navi-icon-192.png", sizes: "192x192" },
  icon512: { file: "acciless-navi-icon-512.png", sizes: "512x512" },
  ogp: { file: "acciless-navi-ogp.png", sizes: "1280x720" },
} as const;

export type BrandAsset = { path: string; sizes: string; source: Buffer };
export type BrandAssets = Record<keyof typeof SOURCES, BrandAsset>;

export async function loadBrandAssets(root: string): Promise<BrandAssets> {
  const entries = await Promise.all(Object.entries(SOURCES).map(async ([key, { file, sizes }]) => {
    const source = await readFile(resolve(root, "brand", file));
    const hash = createHash("sha256").update(source).digest("hex").slice(0, 8);
    return [key, { path: `/assets/${file.replace(/\.png$/, `-${hash}.png`)}`, sizes, source }];
  }));
  return Object.fromEntries(entries) as BrandAssets;
}

export function manifestIcons(brand: BrandAssets) {
  return [brand.icon192, brand.icon512].map(({ path, sizes }) => ({ src: path, sizes, type: "image/png" }));
}

/** `VITE_PUBLIC_ORIGIN` を末尾スラッシュなしのoriginに正規化する。未設定なら undefined。 */
export function publicOrigin(value: string | undefined): string | undefined {
  if (!value) return undefined;
  const url = new URL(value);
  if (url.pathname !== "/" || url.search || url.hash) {
    throw new Error("VITE_PUBLIC_ORIGIN はパスを含まない origin（例：https://example.run.app）にしてください");
  }
  return url.origin;
}

type Tag = { tag: string; attrs: Record<string, string>; injectTo: "head" };

/** index.html の head へ入れるアイコンとOGP画像のタグ。og:url・og:image は絶対URLが必要なので、originがあるときだけ出す。 */
export function brandTags(brand: BrandAssets, origin: string | undefined): Tag[] {
  const link = (attrs: Record<string, string>): Tag => ({ tag: "link", attrs, injectTo: "head" });
  const meta = (attrs: Record<string, string>): Tag => ({ tag: "meta", attrs, injectTo: "head" });
  const tags = [
    link({ rel: "icon", type: "image/png", sizes: brand.favicon.sizes, href: brand.favicon.path }),
    link({ rel: "apple-touch-icon", sizes: brand.appleTouch.sizes, href: brand.appleTouch.path }),
  ];
  if (!origin) return tags;
  const [width, height] = brand.ogp.sizes.split("x");
  return [
    ...tags,
    meta({ property: "og:url", content: `${origin}/` }),
    meta({ property: "og:image", content: `${origin}${brand.ogp.path}` }),
    meta({ property: "og:image:type", content: "image/png" }),
    meta({ property: "og:image:width", content: width }),
    meta({ property: "og:image:height", content: height }),
    meta({ property: "og:image:alt", content: "AcciLessNaviのロゴ" }),
    meta({ name: "twitter:card", content: "summary_large_image" }),
  ];
}
