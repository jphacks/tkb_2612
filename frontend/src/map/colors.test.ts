import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it } from "vitest";
import { mapColor, TOKEN_DEFAULTS } from "./colors";
import { buildMapLayers } from "./layers";

const css = readFileSync("src/styles.css", "utf8");
const root = css.slice(css.indexOf(":root {"), css.indexOf("\n}", css.indexOf(":root {")));
const token = (name: string) => new RegExp(`${name}:\\s*([^;]+);`).exec(root)?.[1].trim();

afterEach(() => {
  for (const name of Object.keys(TOKEN_DEFAULTS)) document.documentElement.style.removeProperty(name);
});

describe("地図アダプタの色トークン", () => {
  it("既定値はstyles.cssの:rootのトークンと同じ（直書き色との乖離を防ぐ）", () => {
    for (const [name, value] of Object.entries(TOKEN_DEFAULTS)) expect(token(name)).toBe(value);
  });

  it("ナビ用の色は新しい直書きではなく既存トークンを再利用する", () => {
    expect(token("--blue")).toBe("#2563eb");
    for (const name of ["--caution", "--caution-bg", "--caution-border", "--caution-text", "--danger", "--ink", "--surface"])
      expect(token(name)).toBeDefined();
  });

  it("CSSのトークンが変わると、次に作る層の色もそれに従う", () => {
    document.documentElement.style.setProperty("--caution", "#112233");
    document.documentElement.style.setProperty("--blue", "#445566");
    expect(mapColor("--caution")).toBe("#112233");
    const layers = buildMapLayers();
    const paint = (id: string) => (layers.find((layer) => layer.id === id) as { paint: Record<string, unknown> }).paint;
    expect(paint("route-high-risk-line")["line-color"]).toBe("#112233");
    expect(paint("navigation-alert-line")["line-color"]).toBe("#112233");
    expect(paint("navigation-alert-point")["circle-color"]).toBe("#112233");
    expect(paint("route-accidents-point")["circle-stroke-color"]).toBe("#112233");
    expect(paint("navigation-route-line")["line-color"]).toBe("#445566");
    expect(paint("current-accuracy-fill")["fill-color"]).toBe("#445566");
  });

  it("トークンが読めない環境では、これまでと同じ色を使う", () => {
    const layers = buildMapLayers();
    const paint = (id: string) => (layers.find((layer) => layer.id === id) as { paint: Record<string, unknown> }).paint;
    expect(paint("route-high-risk-line")["line-color"]).toBe("#d97706");
    expect(paint("navigation-route-line")["line-color"]).toBe("#2563eb");
  });
});
