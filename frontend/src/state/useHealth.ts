import { useEffect, useRef, useState } from "react";
import { ApiError, getHealth } from "../api/client";
import { describeError } from "../api/errorMessage";
import type { HealthResponse } from "../lib/types";

/** frontend-spec 3: one /api/health on first display, manual re-check, dataset_id comparison. */
export function useHealth() {
  const [info, setInfo] = useState<HealthResponse | null>(null);
  const [warning, setWarning] = useState("");
  const [blocked, setBlocked] = useState(false);
  const [revision, setRevision] = useState(0);
  // dataset_id of the last fetched /api/health and of the latest route result.
  const healthDataset = useRef<string | null>(null);
  const routeDataset = useRef<string | null>(null);
  const datasetRechecked = useRef<string | null>(null);

  useEffect(() => {
    const abort = new AbortController();
    setWarning("");
    getHealth(abort.signal)
      .then((health) => {
        healthDataset.current = health.dataset_id;
        setInfo(health);
        checkDatasetMismatch();
        setBlocked(false);
      })
      .catch((failure) => {
        if (abort.signal.aborted) return;
        setBlocked(
          failure instanceof ApiError && failure.code === "data_unavailable",
        );
        setWarning(
          describeError(failure, "データの準備状況を確認できませんでした。"),
        );
      });
    return () => abort.abort();
  }, [revision]);

  // frontend-spec 3: a route dataset_id differing from health re-checks /api/health once,
  // whichever of the two responses completes first.
  function checkDatasetMismatch() {
    const route = routeDataset.current;
    if (
      route === null ||
      healthDataset.current === null ||
      route === healthDataset.current ||
      datasetRechecked.current === route
    )
      return;
    datasetRechecked.current = route;
    setRevision((count) => count + 1);
  }

  return {
    info,
    warning,
    blocked,
    recheck: () => setRevision((count) => count + 1),
    noteRouteDataset(datasetId: string) {
      routeDataset.current = datasetId;
      checkDatasetMismatch();
    },
  };
}
