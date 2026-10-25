import { useCallback, useEffect, useRef, useState } from "react";
import type { RouteCandidate, RouteResponse } from "../lib/types";
import { checkNavigationStart, type NavigationSelection } from "../navigation/preflight";
import {
  NavigationSession,
  type NavigationKind,
  type NavigationSnapshot,
  type SimulationSettings,
} from "../navigation/session";
import type { SimulationMultiplier } from "../navigation/simulator";
import type { PositionFix } from "../navigation/source";

export const START_MESSAGE = {
  device: "ナビを開始します",
  get simulation() {
    return import.meta.env.VITE_ENABLE_SIMULATION === "true"
      ? "模擬走行を開始します。実際の現在地ではありません" : "";
  },
} as const;

export interface StartRequest {
  /** ボタンを描画した時点の検索結果。古い結果からの開始を拒否するために使う。 */
  forResult: RouteResponse;
  selection: NavigationSelection;
  kind: NavigationKind;
  simulation?: SimulationSettings;
}

/** 7.6: 再検索のために停止したナビから引き継ぐ内容。位置は補正前の最後の測位。 */
export interface ReSearchSource {
  kind: NavigationKind;
  selection: NavigationSelection;
  simulation: SimulationSettings | null;
  fix: PositionFix | null;
  /** 精度30m以下かつ5秒以内（実機は端末時計、模擬は仮想時計）。 */
  fixFresh: boolean;
}

export interface NavigationControl {
  snapshot: NavigationSnapshot | null;
  /** 選定経路。ナビ中だけ非null。 */
  route: RouteCandidate | null;
  startError: string;
  start(request: StartRequest): void;
  stop(): void;
  pause(): void;
  resume(): void;
  setMultiplier(value: SimulationMultiplier): void;
  setJitter(value: boolean): void;
  tryDeviation(): void;
  /** 再検索の開始。位置源・音声を止め、最後の未補正測位と選定種別を返す。 */
  takeForReSearch(): ReSearchSource | null;
}

interface Options {
  result: RouteResponse | null;
  mapAvailable: boolean;
}

/**
 * ナビ状態機械のReact側。位置・進捗・通知済みIDはNavigationSessionのメモリだけにあり、
 * ストレージ・URL・historyへは出さない。
 */
export function useNavigation({ result, mapAvailable }: Options): NavigationControl {
  const [snapshot, setSnapshot] = useState<NavigationSnapshot | null>(null);
  const [route, setRoute] = useState<RouteCandidate | null>(null);
  const [startError, setStartError] = useState("");
  const session = useRef<NavigationSession | null>(null);
  const sessionResult = useRef<RouteResponse | null>(null);
  const generations = useRef(new WeakMap<RouteResponse, number>());
  const counter = useRef(0);
  const latest = useRef(result);
  latest.current = result;
  if (result && !generations.current.has(result)) generations.current.set(result, ++counter.current);

  const end = useCallback((reason: "stop" | "unmount" = "stop") => {
    session.current?.stop(reason);
    session.current = null;
    sessionResult.current = null;
    setSnapshot(null);
    setRoute(null);
  }, []);

  const start = useCallback(
    ({ forResult, selection, kind, simulation }: StartRequest) => {
      if (kind === "simulation" && import.meta.env.VITE_ENABLE_SIMULATION !== "true") return;
      const current = latest.current;
      setStartError("");
      if (!current || session.current) return;
      const candidate = current.candidates.find(
        (item) => item.id === (selection === "fastest" ? current.fastest_id : current.safest_id),
      );
      if (!candidate) {
        setStartError("選定された経路から開始してください");
        return;
      }
      const check = checkNavigationStart({
        route: candidate,
        selection,
        fastestId: current.fastest_id,
        safestId: current.safest_id,
        responseGeneration: generations.current.get(forResult) ?? -1,
        currentGeneration: generations.current.get(current) ?? -2,
        mapAvailable,
      });
      if (!check.ok) {
        setStartError(check.reason);
        return;
      }
      if (kind === "device") {
        if (typeof window !== "undefined" && window.isSecureContext === false) {
          setStartError(import.meta.env.VITE_ENABLE_SIMULATION === "true"
            ? "HTTPSで開いたページでのみ実機GPSを利用できます。模擬走行を使えます。"
            : "HTTPSで開いたページでのみ実機GPSを利用できます。");
          return;
        }
        if (typeof navigator === "undefined" || !navigator.geolocation) {
          setStartError(import.meta.env.VITE_ENABLE_SIMULATION === "true"
            ? "このブラウザーでは位置情報を利用できません。模擬走行を使えます。"
            : "このブラウザーでは位置情報を利用できません。");
          return;
        }
      }
      let next: NavigationSession;
      try {
        next = new NavigationSession({
          route: check.route,
          selection,
          mode: current.effective_conditions.mode,
          kind,
          simulation,
          startMessage: START_MESSAGE[kind],
          onChange: (value) => {
            if (session.current === next) setSnapshot(value);
          },
        });
      } catch (failure) {
        setStartError(failure instanceof Error ? failure.message : "ナビを開始できません");
        return;
      }
      session.current = next;
      sessionResult.current = current;
      setRoute(check.route);
      next.begin();
      setSnapshot(next.getSnapshot());
    },
    [mapAvailable],
  );

  // 結果の解除・差替え（経路切替を含む）でナビ世代を進め、旧セッションを破棄する。
  useEffect(() => {
    if (session.current && sessionResult.current !== result) end();
  }, [result, end]);

  useEffect(() => {
    if (!mapAvailable && snapshot && snapshot.phase !== "paused" && snapshot.phase !== "arrived")
      session.current?.pause("map-unavailable");
  }, [mapAvailable, snapshot]);

  useEffect(() => {
    const hidden = () => {
      if (document.visibilityState === "hidden") session.current?.suspendForLifecycle("hidden");
    };
    const pagehide = () => session.current?.suspendForLifecycle("pagehide");
    document.addEventListener("visibilitychange", hidden);
    window.addEventListener("pagehide", pagehide);
    return () => {
      document.removeEventListener("visibilitychange", hidden);
      window.removeEventListener("pagehide", pagehide);
    };
  }, []);

  useEffect(() => () => {
    session.current?.stop("unmount");
    session.current = null;
  }, []);

  const takeForReSearch = useCallback((): ReSearchSource | null => {
    const active = session.current;
    if (!active) return null;
    const fix = active.lastUncorrectedFix();
    const snap = active.getSnapshot();
    const age = fix
      ? (active.kind === "device" ? Date.now() : active.sourceNow()) - fix.timestamp
      : Infinity;
    const taken: ReSearchSource = {
      kind: active.kind,
      selection: active.selection,
      simulation: snap.simulation
        ? { seed: snap.simulation.seed, multiplier: snap.simulation.multiplier, jitter: snap.simulation.jitter }
        : null,
      fix,
      fixFresh: !!fix && fix.accuracy <= 30 && age >= 0 && age <= 5000,
    };
    end();
    return taken;
  }, [end]);

  return {
    snapshot,
    route,
    startError,
    start,
    stop: () => end(),
    pause: () => session.current?.pause("user"),
    resume: () => {
      if (mapAvailable) session.current?.resume();
    },
    setMultiplier: (value) => session.current?.setMultiplier(value),
    setJitter: (value) => session.current?.setJitter(value),
    tryDeviation: () => session.current?.tryDeviation(),
    takeForReSearch,
  };
}
