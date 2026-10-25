import { ChevronDown } from "lucide-react";
import { useMemo, useState } from "react";
import type { RouteCandidate, RouteResponse } from "../lib/types";
import { checkNavigationStart, type NavigationSelection } from "../navigation/preflight";
import type { NavigationKind, SimulationSettings } from "../navigation/session";
import type { SimulationMultiplier } from "../navigation/simulator";
import { SimulationSettingsFields, parseSeed } from "./SimulationControls";

export interface NavigationStartProps {
  result: RouteResponse;
  mapAvailable: boolean;
  /** 開始操作の拒否理由（開始前検査・位置情報の非対応など）。 */
  error: string;
  onStart: (request: {
    selection: NavigationSelection;
    kind: NavigationKind;
    simulation?: SimulationSettings;
  }) => void;
}

export const SELECTION_LABEL: Record<NavigationSelection, string> = {
  fastest: "最短時間",
  safest: "事故履歴を考慮",
};

/** 選定2経路（fastest_idとnullでないsafest_id）だけに開始操作を置く。 */
export function NavigationStart({ result, mapAvailable, error, onStart }: NavigationStartProps) {
  const [multiplier, setMultiplier] = useState<SimulationMultiplier>(1);
  const [jitter, setJitter] = useState(true);
  const [seedText, setSeedText] = useState("1");
  const seed = parseSeed(seedText);
  const rows = useMemo(() => {
    const selections: [NavigationSelection, RouteCandidate | undefined][] = [
      ["fastest", result.candidates.find((route) => route.id === result.fastest_id)],
      ...(result.safest_id
        ? [
            [
              "safest",
              result.candidates.find((route) => route.id === result.safest_id),
            ] as [NavigationSelection, RouteCandidate | undefined],
          ]
        : []),
    ];
    return selections.flatMap(([selection, route]) =>
      route
        ? [
            {
              selection,
              check: checkNavigationStart({
                route,
                selection,
                fastestId: result.fastest_id,
                safestId: result.safest_id,
                responseGeneration: 0,
                currentGeneration: 0,
                mapAvailable,
              }),
            },
          ]
        : [],
    );
  }, [result, mapAvailable]);
  const same = result.safest_id !== null && result.safest_id === result.fastest_id;

  return (
    <section className="result-content nav-start" aria-label="ナビ開始">
      <div className="result-section-heading">
        <h2>ナビ開始</h2>
      </div>
      {rows.map(({ selection, check }) => (
        <div className="nav-start-row" key={selection} role="group" aria-label={`${SELECTION_LABEL[selection]}経路`}>
          <p className="nav-start-label">
            {SELECTION_LABEL[selection]}
            {same && <span className="result-demo-label">同一経路</span>}
          </p>
          <div className="nav-start-actions">
            <button
              type="button"
              className="primary"
              disabled={!check.ok}
              onClick={() => onStart({ selection, kind: "device" })}
            >
              実機GPSで開始
            </button>
            <button
              type="button"
              className="secondary"
              disabled={!check.ok || seed === null}
              onClick={() =>
                onStart({ selection, kind: "simulation", simulation: { seed: seed!, multiplier, jitter } })
              }
            >
              模擬走行で開始
            </button>
          </div>
          {!check.ok && <p className="result-muted">{check.reason}</p>}
        </div>
      ))}
      {error && (
        <p className="notice warning" role="alert">
          {error}
        </p>
      )}
      <details className="result-details">
        <summary>
          <span>模擬走行の設定</span>
          <ChevronDown size={18} aria-hidden="true" />
        </summary>
        <div className="result-details-body">
          <SimulationSettingsFields
            multiplier={multiplier}
            jitter={jitter}
            seedText={seedText}
            seedValid={seed !== null}
            onMultiplier={setMultiplier}
            onJitter={setJitter}
            onSeedText={setSeedText}
          />
        </div>
      </details>
    </section>
  );
}
