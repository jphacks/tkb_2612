import type { NavigationSnapshot } from "../navigation/session";
import { SELECTION_LABEL } from "./NavigationStart";

export interface NavigationPanelProps {
  snapshot: NavigationSnapshot;
  onStop: () => void;
}

/** ナビ中にサイドバーの結果・条件エディタを置き換える。 */
export function NavigationPanel({ snapshot, onStop }: NavigationPanelProps) {
  return (
    <div className="result-content nav-panel">
      <dl className="result-metrics">
        <div>
          <dt>経路</dt>
          <dd>{SELECTION_LABEL[snapshot.selection]}</dd>
        </div>
        <div>
          <dt>位置源</dt>
          <dd>{snapshot.kind === "simulation" ? "模擬走行" : "実機GPS"}</dd>
        </div>
      </dl>
      <button type="button" className="secondary full" onClick={onStop}>
        ナビを停止
      </button>
    </div>
  );
}
