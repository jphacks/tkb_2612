import { Check, ChevronDown } from "lucide-react";
import type { RouteResponse } from "../lib/types";
import {
  formatExtraMinutes,
  formatKilometres,
  formatMinutes,
} from "../lib/format";
import "./results.css";

export interface ResultsPanelProps {
  result: RouteResponse;
  selectedRouteId: string;
  onSelectRoute: (id: string) => void;
}

const number = new Intl.NumberFormat("ja-JP", { maximumFractionDigits: 2 });
const precise = new Intl.NumberFormat("ja-JP", { maximumFractionDigits: 3 });

/** Aggregation settings and the table of every candidate, collapsed by default. */
export function ResultsPanel({
  result,
  selectedRouteId,
  onSelectRoute,
}: ResultsPanelProps) {
  const selected =
    result.candidates.find((candidate) => candidate.id === selectedRouteId) ??
    result.candidates.find((candidate) => candidate.id === result.fastest_id)!;

  return (
    <div className="result-content">
      <details className="result-details">
        <summary>
          <span>集計条件と全候補</span>
          <ChevronDown size={18} aria-hidden="true" />
        </summary>
        <div className="result-details-body">
          <h3>集計条件</h3>
          <dl className="result-metrics">
            <div>
              <dt>集計半径</dt>
              <dd>{result.radius_m}m</dd>
            </div>
            <div>
              <dt>距離重みのσ</dt>
              <dd>{result.distance_weight_sigma_m}m</dd>
            </div>
            <div>
              <dt>経路候補</dt>
              <dd>{result.candidates.length}本</dd>
            </div>
            <div>
              <dt>データ年</dt>
              <dd>{result.data_years.join("・")}年</dd>
            </div>
            <div>
              <dt>サーバーでの集計・選定時間（通信時間を含みません）</dt>
              <dd>{Math.round(result.aggregation_ms)}ms</dd>
            </div>
          </dl>

          <h3>全候補の比較</h3>
          <div
            className="result-table-scroll"
            role="region"
            aria-label="全候補の比較表（横スクロール）"
            tabIndex={0}
          >
            <table className="result-table">
              <thead>
                <tr>
                  <th scope="col">経路</th>
                  <th scope="col">選定</th>
                  <th scope="col">時間</th>
                  <th scope="col">距離</th>
                  <th scope="col">追加</th>
                  <th scope="col">事故</th>
                  <th scope="col">天候一致</th>
                  <th scope="col">重み</th>
                  <th scope="col">重み/km</th>
                  <th scope="col">許容内</th>
                </tr>
              </thead>
              <tbody>
                {result.candidates.map((candidate, index) => (
                  <tr
                    key={candidate.id}
                    className={
                      candidate.id === selected.id
                        ? "result-table-selected"
                        : ""
                    }
                  >
                    <th scope="row">
                      <button
                        type="button"
                        aria-pressed={candidate.id === selected.id}
                        onClick={() => onSelectRoute(candidate.id)}
                      >
                        候補{index + 1}
                        {candidate.id === selected.id && (
                          <Check size={13} aria-hidden="true" />
                        )}
                      </button>
                    </th>
                    <td>
                      {[
                        candidate.id === result.fastest_id && "最短",
                        candidate.id === result.safest_id && "事故考慮",
                      ]
                        .filter(Boolean)
                        .join(" / ") || "—"}
                    </td>
                    <td>{formatMinutes(candidate.duration)}</td>
                    <td>{formatKilometres(candidate.distance)}</td>
                    <td>{formatExtraMinutes(candidate.extra_minutes)}</td>
                    <td>{candidate.accident_count}件</td>
                    <td>
                      {candidate.weather_match
                        ? `${candidate.weather_match.accident_count}件`
                        : "指定なし"}
                    </td>
                    <td>{number.format(candidate.weighted_risk)}</td>
                    <td>{precise.format(candidate.risk_per_km)}</td>
                    <td>{candidate.within_detour ? "内" : "外"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      </details>
    </div>
  );
}

export default ResultsPanel;
