import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { validateRouteRequest, validateRouteResponse } from "./validate";

// Backend and frontend must agree on the public route contract (docs/api).
// The backend fixture is generated from the OpenAPI 200 examples; the request
// of each example comes from the shared inputs fixture.
const read = (path: string) =>
  JSON.parse(readFileSync(new URL(path, import.meta.url), "utf8"));

const responses = read("../../../backend/tests/fixtures/route-responses.json")
  .examples as Record<string, unknown>;
const inputs = read("../../../docs/api/fixtures/route-example-inputs.json")
  .examples as { name: string; request: unknown }[];

describe("shared route fixtures (backend contract) pass frontend validation", () => {
  it("covers every backend example with a matching request", () => {
    expect(Object.keys(responses).sort()).toEqual(
      inputs.map((example) => example.name).sort(),
    );
  });

  for (const { name, request } of inputs) {
    it(`accepts ${name}`, () => {
      const validRequest = validateRouteRequest(request);
      const response = validateRouteResponse(responses[name], validRequest);
      expect(response.candidates.length).toBeGreaterThan(0);
      // Only the selected routes carry navigation data.
      for (const route of response.candidates) {
        const selected =
          route.id === response.fastest_id || route.id === response.safest_id;
        expect(route.navigation !== null).toBe(selected);
      }
    });
  }
});
