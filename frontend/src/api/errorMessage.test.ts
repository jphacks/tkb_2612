import { describe, expect, it } from "vitest";
import { describeError, ROUTE_INPUT_HINT } from "./errorMessage";
import { ApiError } from "./errors";

const fallback = "ルートを検索できませんでした。";
const text = (code: string, message = "サーバーの説明", status?: number) =>
  describeError(
    new ApiError(message, code, status),
    fallback,
    ROUTE_INPUT_HINT,
  );

describe("describeError (frontend-spec 5 error table)", () => {
  it("keeps the 400 message and separates the 30km straight-line limit from the 80km route limit", () => {
    const shown = text(
      "invalid_request",
      "出発地と目的地の直線距離は20m〜30kmにしてください。",
      400,
    );
    expect(shown).toContain(
      "出発地と目的地の直線距離は20m〜30kmにしてください。",
    );
    expect(shown).toContain("入力を見直してください");
    expect(shown).toMatch(
      /直線距離の上限30km.*候補経路の長さの上限80km.*別の制限/,
    );
  });

  it("does not add the route distance limits to other operations' input errors", () => {
    const shown = describeError(
      new ApiError(
        "住所や地点名は2〜200文字で入力してください。",
        "invalid_request",
        400,
      ),
      fallback,
    );
    expect(shown).toBe(
      "住所や地点名は2〜200文字で入力してください。 入力を見直してください。",
    );
  });

  it("guides each known code by code, not by the message text", () => {
    expect(text("route_too_long", "x", 422)).toMatch(/80km.*近づけて/);
    expect(text("no_route", "x", 502)).toMatch(/道路に近い地点/);
    expect(text("external_api_error", "x", 502)).toMatch(
      /外部サービス.*再試行/,
    );
    expect(text("service_busy", "x", 503)).toMatch(/混み合って.*再試行/);
    expect(text("timeout")).toMatch(/タイムアウト.*再試行/);
    for (const code of [
      "route_too_long",
      "no_route",
      "external_api_error",
      "service_busy",
      "timeout",
    ])
      expect(text(code, "サーバーの説明")).not.toContain("サーバーの説明");
  });

  it("asks for a reload after non-JSON or authentication failures", () => {
    expect(text("invalid_response")).toMatch(
      /読み取れませんでした.*再読み込み/,
    );
    expect(text("auth_required", "x", 401)).toMatch(/ログイン.*再読み込み/);
  });

  it("mentions both a network failure and an expired login when they cannot be told apart", () => {
    const shown = text("network_error");
    expect(shown).toMatch(/接続を確認/);
    expect(shown).toMatch(/ログインの有効期限/);
  });

  it("shows the server message with a retry hint for unknown codes", () => {
    expect(text("brand_new_code", "新しい失敗です。")).toBe(
      "新しい失敗です。 時間をおいて再試行してください。",
    );
  });

  it("treats inherited property names as unknown codes", () => {
    for (const code of [
      "__proto__",
      "constructor",
      "toString",
      "hasOwnProperty",
    ])
      expect(text(code, "新しい失敗です。")).toBe(
        "新しい失敗です。 時間をおいて再試行してください。",
      );
  });

  it("uses the message of plain errors and the fallback otherwise", () => {
    expect(
      describeError(new Error("出発日時を入力してください。"), fallback),
    ).toBe("出発日時を入力してください。");
    expect(describeError("x", fallback)).toBe(fallback);
  });
});
