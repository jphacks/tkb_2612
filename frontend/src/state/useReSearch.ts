import { useCallback, useEffect, useRef, useState } from "react";
import { searchRoutes } from "../api/client";
import { describeError, ROUTE_INPUT_HINT } from "../api/errorMessage";
import type { Place, RouteResponse, Scenario } from "../lib/types";
import type { NavigationSelection } from "../navigation/preflight";
import type { NavigationKind, SimulationSettings } from "../navigation/session";
import type { PositionFix } from "../navigation/source";
import type { NavigationControl } from "./useNavigation";
import type { useConditions } from "./useConditions";
import type { useRouteSearch } from "./useRouteSearch";

export type ReSearchStage = "locating" | "conditions" | "route";
export type ReSearchState =
  | { status: "idle" }
  | { status: "searching"; stage: ReSearchStage; kind: NavigationKind }
  | { status: "interrupted"; kind: NavigationKind }
  | { status: "failed"; kind: NavigationKind; message: string };

export const RESEARCH_INTERRUPTED =
  "再検索を中断しました。安全な場所で再試行してください";
export const TOO_CLOSE =
  "現在地と目的地の直線距離が20m未満のため、再検索できません";
export const NO_SAFEST =
  "安全ルートがありません。最短ルートから開始する場合は、開始操作を行ってください";
export const STAGE_TEXT: Record<ReSearchStage, string> = {
  locating: "現在地を取得しています",
  conditions: "条件を取得しています",
  route: "ルートを検索しています",
};
export const SIMULATION_RESEARCH_NOTE = "模擬位置から条件と経路を取得します";

/** 再試行用のメモリ内snapshot。入力編集・unmountで破棄する。 */
interface Snapshot {
  kind: NavigationKind;
  selection: NavigationSelection;
  simulation: SimulationSettings | null;
  /** 位置の再取得前は最後の未補正測位。使い切ったらnull。 */
  lastFix: { fix: PositionFix; fresh: boolean } | null;
  /** 確定済みの新origin。 */
  origin: { place: Place; fix: PositionFix } | null;
}

class Failure extends Error {}

function straightDistanceM(a: { lat: number; lon: number }, b: { lat: number; lon: number }): number {
  const radians = Math.PI / 180;
  return (
    radians * 6_371_008.8 *
    Math.hypot(a.lat - b.lat, (a.lon - b.lon) * Math.cos(((a.lat + b.lat) / 2) * radians))
  );
}

function geolocationFailure(failure: GeolocationPositionError): string {
  switch (failure.code) {
    case failure.PERMISSION_DENIED:
      return "位置情報の利用が許可されていません。設定を変更して再試行してください";
    case failure.TIMEOUT:
      return "現在地の取得がタイムアウトしました。再試行してください";
    default:
      return "現在地を取得できませんでした。再試行してください";
  }
}

function measure(): Promise<PositionFix> {
  return new Promise((resolve, reject) => {
    if (typeof navigator === "undefined" || !navigator.geolocation) {
      reject(new Failure("このブラウザーでは現在地を利用できません"));
      return;
    }
    navigator.geolocation.getCurrentPosition(
      (position) =>
        resolve({
          lon: position.coords.longitude,
          lat: position.coords.latitude,
          accuracy: position.coords.accuracy,
          timestamp: position.timestamp,
        }),
      (failure) => reject(new Failure(geolocationFailure(failure))),
      // 7.1と同じ取得オプション。
      { enableHighAccuracy: true, timeout: 10_000, maximumAge: 0 },
    );
  });
}

function freshTimestamp(fix: PositionFix, now: number): boolean {
  const age = now - fix.timestamp;
  return Number.isFinite(fix.timestamp) && age >= 0 && age <= 5000;
}

function validated(fix: PositionFix, now: number): PositionFix {
  const finite = [fix.lat, fix.lon, fix.accuracy, fix.timestamp].every(Number.isFinite) && fix.accuracy > 0;
  if (!finite || !freshTimestamp(fix, now)) throw new Failure("現在地を正しく取得できませんでした。再試行してください");
  if (fix.lat < 20 || fix.lat > 46 || fix.lon < 122 || fix.lon > 154)
    throw new Failure("日本国内の地点ではないため、再検索できません");
  if (fix.accuracy > 30)
    throw new Failure(
      `位置の精度が低い（約${Math.round(fix.accuracy)}m）ため、再検索できません。見通しのよい場所で再試行してください`,
    );
  return fix;
}

interface Params {
  navigation: NavigationControl;
  route: ReturnType<typeof useRouteSearch>;
  conditions: ReturnType<typeof useConditions>;
  dest: Place | null;
  scenario: Scenario;
  /** 確定した新originを入力状態へ反映する（ページメモリのみ）。 */
  onAdoptOrigin: (place: Place) => void;
  onRouteSuccess: (response: RouteResponse) => void;
}

/**
 * frontend-spec 7.6: 現在地（模擬は模擬位置）からの明示的な再検索。
 * 位置取得 → conditions（必ず1回）→ route（1回）。自動リトライ・自動再開はしない。
 */
export function useReSearch(params: Params) {
  const deps = useRef(params);
  deps.current = params;
  const [state, setState] = useState<ReSearchState>({ status: "idle" });
  const [notice, setNotice] = useState("");
  const [resume, setResume] = useState<{
    generation: number;
    forResult: RouteResponse;
    selection: NavigationSelection;
    kind: NavigationKind;
    simulation: SimulationSettings | null;
  } | null>(null);
  const snapshot = useRef<Snapshot | null>(null);
  const generation = useRef(0);
  const controller = useRef<AbortController | null>(null);
  const searching = useRef<NavigationKind | null>(null);

  const run = useCallback(async () => {
    const snap = snapshot.current;
    if (!snap || searching.current) return;
    controller.current?.abort();
    const abort = new AbortController();
    controller.current = abort;
    const mine = ++generation.current;
    const alive = () => mine === generation.current && !abort.signal.aborted;
    searching.current = snap.kind;
    setNotice("");
    setState({ status: "searching", stage: "locating", kind: snap.kind });
    // 旧結果とナビデータを解除し、旧要求をabortする。
    deps.current.route.invalidate();
    const finish = (next: ReSearchState) => {
      if (!alive()) return;
      searching.current = null;
      setState(next);
    };
    try {
      let fix: PositionFix;
      const existing = snap.origin;
      if (existing && (snap.kind === "simulation" || freshTimestamp(existing.fix, Date.now()))) {
        fix = existing.fix;
      } else if (snap.lastFix && ((snap.lastFix.fresh && freshTimestamp(snap.lastFix.fix, Date.now())) || snap.kind === "simulation")) {
        fix = snap.lastFix.fix;
      } else if (import.meta.env.VITE_ENABLE_SIMULATION === "true" && snap.kind === "simulation") {
        throw new Failure("模擬位置を取得できませんでした");
      } else {
        fix = await measure();
      }
      if (!alive()) return;
      snap.lastFix = null;
      validated(fix, snap.kind === "device" ? Date.now() : fix.timestamp);
      const place: Place = {
        label: import.meta.env.VITE_ENABLE_SIMULATION === "true" && snap.kind === "simulation" ? "模擬位置" : "現在地",
        lat: fix.lat,
        lon: fix.lon,
      };
      snap.origin = { place, fix };
      const dest = deps.current.dest;
      if (!dest) throw new Failure("目的地が確定していません");
      if (straightDistanceM(place, dest) < 20) throw new Failure(TOO_CLOSE);
      setState({ status: "searching", stage: "conditions", kind: snap.kind });
      const pending = deps.current.conditions.refreshForReSearch(
        place,
        dest,
        abort.signal,
        deps.current.scenario,
      );
      deps.current.onAdoptOrigin(place);
      const values = await pending;
      if (!alive()) return;
      setState({ status: "searching", stage: "route", kind: snap.kind });
      const scenario = deps.current.scenario;
      const outcome = await deps.current.route.run(
        (signal) =>
          searchRoutes(
            {
              origin: { lat: place.lat, lon: place.lon },
              dest: { lat: dest.lat, lon: dest.lon },
              ...values,
            },
            signal,
            scenario,
          ),
        (response) => deps.current.onRouteSuccess(response),
      );
      if (!alive() || outcome.status === "stale") return;
      if (outcome.status === "failed") {
        deps.current.route.setError(null);
        finish({
          status: "failed",
          kind: snap.kind,
          message: describeError(outcome.error, "ルートを検索できませんでした。", ROUTE_INPUT_HINT),
        });
        return;
      }
      // 再開effectが完了するまでは、snapshotと中断対象の世代を保持する。
      if (snap.selection === "safest" && outcome.response.safest_id === null) {
        snapshot.current = null;
        finish({ status: "idle" });
        setNotice(NO_SAFEST);
        return;
      }
      setResume({
        generation: mine,
        forResult: outcome.response,
        selection: snap.selection,
        kind: snap.kind,
        simulation: snap.simulation,
      });
    } catch (failure) {
      if (!alive()) return;
      finish({
        status: "failed",
        kind: snap.kind,
        message:
          failure instanceof Failure
            ? failure.message
            : describeError(failure, "再検索できませんでした。"),
      });
    }
  }, []);

  /** 逸脱/保留の表示中に押す。ナビを止めて現在地から検索する。 */
  const start = useCallback(() => {
    const taken = deps.current.navigation.takeForReSearch();
    if (!taken) return;
    snapshot.current = {
      kind: taken.kind,
      selection: taken.selection,
      simulation: taken.simulation,
      lastFix: taken.fix ? { fix: taken.fix, fresh: taken.fixFresh } : null,
      origin: null,
    };
    void run();
  }, [run]);

  const retry = useCallback(() => {
    if (snapshot.current) void run();
  }, [run]);

  /** 入力編集・unmount。再試行snapshotを破棄し、通常の入力へ戻る。 */
  const discard = useCallback(() => {
    generation.current++;
    controller.current?.abort();
    searching.current = null;
    snapshot.current = null;
    setResume(null);
    setNotice("");
    setState((current) => (current.status === "idle" ? current : { status: "idle" }));
  }, []);

  useEffect(() => {
    // hidden / pagehide：要求を中止し、世代を無効化して再検索中断へ一度だけ遷移する。
    const interrupt = () => {
      const kind = searching.current;
      if (!kind) return;
      generation.current++;
      controller.current?.abort();
      searching.current = null;
      setResume(null);
      deps.current.route.invalidate();
      setState({ status: "interrupted", kind });
    };
    const onVisibility = () => {
      if (document.visibilityState === "hidden") interrupt();
    };
    document.addEventListener("visibilitychange", onVisibility);
    window.addEventListener("pagehide", interrupt);
    return () => {
      document.removeEventListener("visibilitychange", onVisibility);
      window.removeEventListener("pagehide", interrupt);
      generation.current++;
      controller.current?.abort();
      snapshot.current = null;
    };
  }, []);

  useEffect(() => {
    if (!resume || params.route.result !== resume.forResult) return;
    if (resume.generation !== generation.current) return;
    if (document.visibilityState === "hidden") {
      generation.current++;
      controller.current?.abort();
      searching.current = null;
      setResume(null);
      deps.current.route.invalidate();
      setState({ status: "interrupted", kind: resume.kind });
      return;
    }
    setResume(null);
    searching.current = null;
    snapshot.current = null;
    setState({ status: "idle" });
    deps.current.navigation.start({
      forResult: resume.forResult,
      selection: resume.selection,
      kind: resume.kind,
      simulation: resume.simulation ?? undefined,
    });
  }, [resume, params.route.result]);

  return { state, notice, start, retry, discard };
}
