// @vitest-environment node
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, it } from "vitest";
import { brandTags, loadBrandAssets, publicOrigin } from "../../scripts/brand-assets";

async function withBrand(test: (root: string) => Promise<void>) {
  const root = await mkdtemp(join(tmpdir(), "acciless-brand-"));
  try {
    await mkdir(join(root, "brand"));
    for (const name of ["icon-32", "icon-180", "icon-192", "icon-512", "ogp"]) {
      await writeFile(join(root, `brand/acciless-navi-${name}.png`), name);
    }
    await test(root);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

it("配信名は内容ハッシュ付きで、内容が変わると名前も変わる", () => withBrand(async (root) => {
  const before = await loadBrandAssets(root);
  expect(before.favicon.path).toMatch(/^\/assets\/acciless-navi-icon-32-[0-9a-f]{8}\.png$/);
  await writeFile(join(root, "brand/acciless-navi-icon-32.png"), "changed");
  expect((await loadBrandAssets(root)).favicon.path).not.toBe(before.favicon.path);
}));

it("originがないときはog:url・og:imageを出さず、あるときは絶対URLで出す", () => withBrand(async (root) => {
  const brand = await loadBrandAssets(root);
  const content = (tags: ReturnType<typeof brandTags>, property: string) =>
    tags.find(({ attrs }) => attrs.property === property)?.attrs.content;
  const local = brandTags(brand, undefined);
  expect(content(local, "og:url")).toBeUndefined();
  expect(content(local, "og:image")).toBeUndefined();
  expect(local.map(({ attrs }) => attrs.rel)).toEqual(["icon", "apple-touch-icon"]);
  const deployed = brandTags(brand, "https://app.example");
  expect(content(deployed, "og:url")).toBe("https://app.example/");
  expect(content(deployed, "og:image")).toBe(`https://app.example${brand.ogp.path}`);
}));

it("VITE_PUBLIC_ORIGINはoriginだけを受ける", () => {
  expect(publicOrigin(undefined)).toBeUndefined();
  expect(publicOrigin("")).toBeUndefined();
  expect(publicOrigin("https://app.example/")).toBe("https://app.example");
  expect(() => publicOrigin("https://app.example/sub")).toThrow();
  expect(() => publicOrigin("app.example")).toThrow();
});
