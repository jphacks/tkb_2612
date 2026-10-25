import { useId, type ReactNode } from "react";
import type { SimulationSettings } from "../navigation/session";
import type { SimulationMultiplier } from "../navigation/simulator";

const MULTIPLIERS: SimulationMultiplier[] = [1, 2, 4, 8];

export const SEED_MAX = 4294967295;

/** 0〜2^32−1の整数だけをseedとして受け付ける。 */
export function parseSeed(text: string): number | null {
  if (!/^\d{1,10}$/.test(text)) return null;
  const value = Number(text);
  return value <= SEED_MAX ? value : null;
}

function MultiplierField({
  value,
  onChange,
}: {
  value: SimulationMultiplier;
  onChange: (value: SimulationMultiplier) => void;
}) {
  return (
    <fieldset>
      <legend>倍率</legend>
      <div className="segmented" role="radiogroup" aria-label="模擬走行の倍率">
        {MULTIPLIERS.map((multiplier) => (
          <button
            key={multiplier}
            type="button"
            role="radio"
            aria-checked={value === multiplier}
            onClick={() => onChange(multiplier)}
          >
            {multiplier}倍
          </button>
        ))}
      </div>
    </fieldset>
  );
}

function JitterField({
  value,
  onChange,
}: {
  value: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <label className="result-toggle-row">
      <span>位置の揺らぎ（半径8m）</span>
      <input
        type="checkbox"
        checked={value}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span className="result-switch-track" aria-hidden="true" />
    </label>
  );
}

export interface SimulationSettingsProps {
  multiplier: SimulationMultiplier;
  jitter: boolean;
  seedText: string;
  seedValid: boolean;
  onMultiplier: (value: SimulationMultiplier) => void;
  onJitter: (value: boolean) => void;
  onSeedText: (value: string) => void;
}

/** 開始前の設定。seedは開始時に決まり、走行中は変更できない。 */
export function SimulationSettingsFields(props: SimulationSettingsProps) {
  const id = useId();
  return (
    <div className="sim-settings">
      <MultiplierField value={props.multiplier} onChange={props.onMultiplier} />
      <div>
        <label className="sim-seed-label" htmlFor={id}>
          seed（0〜{SEED_MAX}の整数）
        </label>
        <input
          id={id}
          className="date-input"
          inputMode="numeric"
          value={props.seedText}
          aria-invalid={!props.seedValid}
          onChange={(event) => props.onSeedText(event.target.value)}
        />
      </div>
      <JitterField value={props.jitter} onChange={props.onJitter} />
    </div>
  );
}

export interface SimulationControlsProps {
  settings: SimulationSettings & { running: boolean };
  /** 到着後は一時停止・再開・逸脱操作を出さない。 */
  arrived: boolean;
  onMultiplier: (value: SimulationMultiplier) => void;
  onJitter: (value: boolean) => void;
  onPause: () => void;
  onResume: () => void;
  onTryDeviation: () => void;
  /** 一時停止・逸脱の操作の直後に置く内容（ナビの停止ボタン）。 */
  afterActions?: ReactNode;
}

/** 走行中の操作。倍率と揺らぎは走行中も変更でき、逸脱を試せる（seedは開始時のまま）。 */
export function SimulationControls({
  settings,
  arrived,
  onMultiplier,
  onJitter,
  onPause,
  onResume,
  onTryDeviation,
  afterActions,
}: SimulationControlsProps) {
  return (
    <section className="sim-controls" aria-label="模擬走行の操作">
      {!arrived && (
        <div className="nav-actions">
          <button
            type="button"
            className="secondary"
            onClick={settings.running ? onPause : onResume}
          >
            {settings.running ? "一時停止" : "再開"}
          </button>
          <button
            type="button"
            className="secondary"
            disabled={!settings.running}
            onClick={onTryDeviation}
          >
            逸脱を試す
          </button>
        </div>
      )}
      {afterActions}
      <MultiplierField value={settings.multiplier} onChange={onMultiplier} />
      <JitterField value={settings.jitter} onChange={onJitter} />
      <dl className="nav-metrics">
        <div>
          <dt>seed</dt>
          <dd>{settings.seed}</dd>
        </div>
      </dl>
    </section>
  );
}
