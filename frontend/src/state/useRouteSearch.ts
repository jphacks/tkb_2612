import { useEffect, useRef, useState } from "react";
import type { RouteResponse } from "../lib/types";

/**
 * frontend-spec 3: a route result belongs to the generation that requested it.
 * Late responses (success or failure) of older generations are dropped.
 */
export function useRouteSearch() {
  const [result, setResult] = useState<RouteResponse | null>(null);
  const [selectedRoute, setSelectedRoute] = useState<string | null>(null);
  const [selectedSection, setSelectedSection] = useState<string | null>(null);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const request = useRef<AbortController | null>(null);
  const version = useRef(0);

  useEffect(() => () => request.current?.abort(), []);

  function invalidate() {
    ++version.current;
    request.current?.abort();
    setSearching(false);
    setResult(null);
    setSelectedRoute(null);
    setSelectedSection(null);
    setError(null);
  }

  /**
   * Starts a new generation. `execute` resolves with the validated response;
   * `onSuccess` runs only for the current generation.
   */
  async function run(
    execute: (signal: AbortSignal) => Promise<RouteResponse>,
    onSuccess: (response: RouteResponse) => void,
  ) {
    request.current?.abort();
    const abort = new AbortController();
    request.current = abort;
    const current = ++version.current;
    setSearching(true);
    setResult(null);
    setSelectedSection(null);
    setError(null);
    try {
      const response = await execute(abort.signal);
      if (current !== version.current || abort.signal.aborted) return;
      setResult(response);
      setSelectedRoute(response.safest_id ?? response.fastest_id);
      onSuccess(response);
    } catch (failure) {
      if (abort.signal.aborted || current !== version.current) return;
      setError(failure ?? new Error());
    } finally {
      if (current === version.current) setSearching(false);
    }
  }

  return {
    result,
    selectedRoute,
    setSelectedRoute,
    selectedSection,
    setSelectedSection,
    searching,
    error,
    setError,
    invalidate,
    run,
  };
}
