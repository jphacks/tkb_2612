import { vi } from "vitest";

/** ナビ開始などの前後で、API以外も含むfetchの増分を確認する。 */
export function createFetchCounter(implementation: typeof fetch) {
  const countedFetch = vi.fn(implementation);
  return {
    fetch: countedFetch,
    mark: () => countedFetch.mock.calls.length,
    count(pathname?: string, since = 0) {
      const calls = countedFetch.mock.calls.slice(since);
      if (pathname === undefined) return calls.length;
      return calls.filter(([input]) => {
        const target = input instanceof Request ? input.url : String(input);
        return new URL(target, "http://localhost").pathname === pathname;
      }).length;
    },
  };
}
