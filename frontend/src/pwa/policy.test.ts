import { describe, expect, it } from "vitest";
import { cacheName, oldShellCaches, isShellRequest, canReloadForUpdate } from "./policy";

const origin = "https://app.example";
const assets = ["/", "/assets/app-123.js", "/api/health", "https://tiles.example/tile"];

describe("シェルのキャッシュ境界", () => {
  it.each([
    ["/", "GET", true],
    ["/assets/app-123.js", "GET", true],
    ["/api/health", "GET", false],
    ["/api/conditions", "GET", false],
    ["https://tiles.example/tile", "GET", false],
    ["https://other.example/assets/app-123.js", "GET", false],
    ["/assets/unknown.js", "GET", false],
    ["/?address=secret", "GET", false],
    ["/assets/app-123.js?position=secret", "GET", false],
    ["/", "POST", false],
  ])("%s %s の対象判定は %s", (url, method, expected) => {
    expect(isShellRequest(new URL(url, origin).href, method, origin, assets)).toBe(expected);
  });
  it("version ごとに分離し、自アプリの旧キャッシュだけを削除する", () => {
    const current = cacheName("v2");
    expect(current).not.toBe(cacheName("v1"));
    expect(current).toContain("v2");
    expect(oldShellCaches([cacheName("v1"), current, "another-app-v1"], current))
      .toEqual([cacheName("v1")]);
  });
  it.each([[false, [], false], [true, [], true], [true, ["navigation"], false],
    [true, ["gps", "search"], false]] as const)("更新と禁止理由から再読込可否を判定する", (waiting, reasons, expected) => {
    expect(canReloadForUpdate(waiting, reasons)).toBe(expected);
  });
});
