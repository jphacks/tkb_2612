import { readFileSync } from "node:fs";
import { act, renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { RouteResponse } from "../lib/types";
import { useRouteSearch } from "./useRouteSearch";

const read = (path: string) =>
  JSON.parse(readFileSync(new URL(path, import.meta.url), "utf8"));
const response = read("../../../backend/tests/fixtures/route-responses.json")
  .examples.twoSelected as RouteResponse;
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((accept, decline) => {
    resolve = accept;
    reject = decline;
  });
  return { promise, resolve, reject };
}

describe("route response generations (frontend-spec 3, 11)", () => {
  it.each(["success", "error"])(
    "discards an old %s after an input edit without releasing the new search lock",
    async (outcome) => {
      const { result } = renderHook(() => useRouteSearch());
      const old = deferred<RouteResponse>();
      const current = deferred<RouteResponse>();
      const onSuccess = vi.fn();
      let oldSignal!: AbortSignal;
      let oldRun!: Promise<void>;
      let currentRun!: Promise<void>;
      act(() => {
        oldRun = result.current.run((signal) => {
          oldSignal = signal;
          return old.promise;
        }, onSuccess);
      });
      act(() => result.current.invalidate());
      expect(oldSignal.aborted).toBe(true);
      act(() => {
        currentRun = result.current.run(() => current.promise, onSuccess);
      });
      await act(async () => {
        // 通信先がabortを無視しても、古い世代の成功・エラーは反映しない。
        if (outcome === "success") old.resolve(response);
        else old.reject(new Error("古い要求の失敗"));
        await oldRun;
      });
      expect(result.current.searching).toBe(true);
      expect(result.current.result).toBeNull();
      expect(result.current.error).toBeNull();
      expect(onSuccess).not.toHaveBeenCalled();
      await act(async () => {
        current.resolve(response);
        await currentRun;
      });
      expect(result.current.searching).toBe(false);
      expect(result.current.result).toEqual(response);
      expect(result.current.selectedRoute).toBe(response.safest_id);
      expect(onSuccess).toHaveBeenCalledExactlyOnceWith(response);
    },
  );
});
