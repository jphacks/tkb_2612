import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  validateConditions,
  validateError,
  validateHealth,
  validatePlaces,
} from "./validate";

const validators = {
  validateConditions,
  validateError,
  validateHealth,
  validatePlaces,
};
const read = (path: string) =>
  JSON.parse(readFileSync(new URL(path, import.meta.url), "utf8"));
const fixture = read("../../../backend/tests/fixtures/api-responses.json") as {
  responses: Record<string, { validator: string; body: unknown }>;
};

// C2の例の追加・削除は、検証側でも確認してから一覧を更新する。
const expectedNames = [
  "conditions.currentOk",
  "conditions.forecastOk",
  "conditions.holidayTableNotCovered",
  "conditions.noLocation",
  "conditions.outOfRange",
  "conditions.past",
  "conditions.unavailable",
  "error.dataUnavailable.health",
  "error.dataUnavailable.route",
  "error.externalApi.geocode",
  "error.externalApi.route",
  "error.internal.route",
  "error.invalidRequest.conditions",
  "error.invalidRequest.geocode",
  "error.invalidRequest.route",
  "error.methodNotAllowed.health",
  "error.methodNotAllowed.route",
  "error.noRoute.route",
  "error.notFound.api",
  "error.routeTooLong.route",
  "error.serviceBusy.geocode",
  "error.serviceBusy.route",
  "geocode.empty",
  "geocode.multiple",
  "health.ready",
  "health.status",
];

describe("C2 real handler responses pass frontend validation", () => {
  it("detects added or removed examples", () => {
    expect(Object.keys(fixture.responses).sort()).toEqual(
      [...expectedNames].sort(),
    );
  });

  for (const [name, entry] of Object.entries(fixture.responses)) {
    it(`accepts ${name} with ${entry.validator}`, () => {
      expect(Object.hasOwn(validators, entry.validator)).toBe(true);
      const validate = validators[entry.validator as keyof typeof validators];
      expect(validate(entry.body)).toEqual(entry.body);
    });
  }
});
