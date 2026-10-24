import { useEffect, useRef, useState } from "react";
import type { RouteResponse } from "../lib/types";

/**
 * frontend-spec 3: a route result belongs to the input snapshot and generation that
 * requested it. Late responses (success or failure) of older generations are dropped.
 */
export function useRouteSearch<Snapshot>() {
  const [result, setResult] = useState<RouteResponse | null>(null);
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
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
    setSnapshot(null);
    setSelectedRoute(null);
    setSelectedSection(null);
    setError(null);
  }

  /**
   * Starts a new generation. `execute` resolves with the validated response and the
   * input snapshot it was requested with; `onSuccess` runs only for the current generation.
   */
  async function run(
    execute: (
      signal: AbortSignal,
    ) => Promise<{ response: RouteResponse; snapshot: Snapshot }>,
    onSuccess: (response: RouteResponse) => void,
  ) {
    request.current?.abort();
    const abort = new AbortController();
    request.current = abort;
    const current = ++version.current;
    setSearching(true);
    setResult(null);
    setSnapshot(null);
    setSelectedSection(null);
    setError(null);
    try {
      const done = await execute(abort.signal);
      if (current !== version.current || abort.signal.aborted) return;
      setResult(done.response);
      setSnapshot(done.snapshot);
      setSelectedRoute(done.response.safest_id ?? done.response.fastest_id);
      onSuccess(done.response);
    } catch (failure) {
      if (abort.signal.aborted || current !== version.current) return;
      setError(failure ?? new Error());
    } finally {
      if (current === version.current) setSearching(false);
    }
  }

  return {
    result,
    snapshot,
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
