import { expect, it } from "vitest";
import { createFetchCounter } from "./fetchCounter";

it("counts all fetches and filters API paths across a checkpoint", async () => {
  const counter = createFetchCounter(async () => new Response("{}"));
  await counter.fetch("/api/health");
  const checkpoint = counter.mark();
  await counter.fetch("/api/conditions?datetime=2026-10-24");
  await counter.fetch(new URL("https://tiles.example/style.json"));
  await counter.fetch(
    new Request("https://example.test/api/route", { method: "POST" }),
  );
  expect(counter.count()).toBe(4);
  expect(counter.count(undefined, checkpoint)).toBe(3);
  expect(counter.count("/api/health", checkpoint)).toBe(0);
  expect(counter.count("/api/conditions", checkpoint)).toBe(1);
  expect(counter.count("/api/route", checkpoint)).toBe(1);
});

it("counts failed calls as well as successful calls", async () => {
  const counter = createFetchCounter(async () => {
    throw new TypeError("offline");
  });
  await expect(counter.fetch("/api/route")).rejects.toThrow("offline");
  expect(counter.count("/api/route")).toBe(1);
});
