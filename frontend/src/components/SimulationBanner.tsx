import type { NavigationSnapshot } from "../navigation/session";

export const SIMULATION_NOTICE = "模擬走行中（実際の現在地ではありません）";

/** 模擬走行の状態。倍率と、進行/一時停止/停止/到着を常に示す。 */
export function simulationState(snapshot: NavigationSnapshot): string {
  if (!snapshot.simulation) return "";
  const state =
    snapshot.phase === "arrived"
      ? "到着"
      : snapshot.pauseReason === "sim-limit"
        ? "停止"
        : snapshot.phase === "paused"
          ? "一時停止"
          : "進行中";
  return `倍率 ${snapshot.simulation.multiplier}倍・${state}`;
}

export function SimulationBanner({ snapshot }: { snapshot: NavigationSnapshot }) {
  if (!snapshot.simulation) return null;
  return (
    <div className="nav-sim-banner">
      <strong>{SIMULATION_NOTICE}</strong>
      <span>{simulationState(snapshot)}</span>
    </div>
  );
}

export const SIM_STOP_CAUSE = {
  照合保留: "重複経路等で位置を確定できないため模擬走行を停止しました",
  精度不足: "位置精度が低いため模擬走行を停止しました",
  逸脱: "経路から外れたため模擬走行を停止しました",
} as const;

