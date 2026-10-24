import {
  AlertTriangle,
  ArrowUpRight,
  Check,
  ChevronDown,
  Info,
} from "lucide-react";
import type { RouteCandidate, RouteResponse } from "../lib/types";
import {
  formatExtraMinutes,
  formatKilometres,
  formatMinutes,
} from "../lib/format";
import type { AutoFields } from "./ConditionFields";
import "./results.css";

/** Inputs as they were when the displayed result was requested (frontend-spec 5). */
export type SearchSnapshot = {
  departure: "now" | "custom";
  /** Asia/Tokyo "YYYY-MM-DDTHH:mm" used as the departure time of the request. */
  dateTime: string;
  auto: AutoFields;
};

export interface ResultsPanelProps {
  result: RouteResponse;
  snapshot: SearchSnapshot;
  selectedRouteId: string;
  onSelectRoute: (id: string) => void;
  selectedSectionId: string | null;
  onSelectSection: (id: string | null) => void;
  showAccidents: boolean;
  onShowAccidents: (show: boolean) => void;
  isDemo: boolean;
}

const number = new Intl.NumberFormat("ja-JP", { maximumFractionDigits: 2 });
const precise = new Intl.NumberFormat("ja-JP", { maximumFractionDigits: 3 });
const modeLabels = { walk: "徒歩", bike: "自転車", car: "自動車" };
const FASTEST = "最短時間経路";
const CONSIDERED = "事故考慮経路（取得候補内）";

const dateLabel = (value: string) =>
  `${value.slice(0, 4)}/${Number(value.slice(5, 7))}/${Number(value.slice(8, 10))} ${value.slice(11, 16)}`;
const source = (auto: boolean) => (auto ? "自動" : "手動");

function reductionText(result: RouteResponse, fastest: RouteCandidate) {
  if (result.safest_id === null)
    return "算出できません（事故考慮経路がないため）";
  // A zero baseline never yields a meaningful percentage, whatever the field holds.
  if (fastest.weighted_risk === 0 || result.risk_reduction_percent === null)
    return "算出できません（最短時間経路の基準リスクが0のため）";
  return `${result.risk_reduction_percent.toFixed(1)}%`;
}

function RouteCard({
  route,
  title,
  selected,
  onClick,
}: {
  route: RouteCandidate;
  title: string;
  selected: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`result-route-card${selected ? " result-is-selected" : ""}`}
      aria-pressed={selected}
      onClick={onClick}
    >
      <span className="result-route-card-title">
        {title}
        <span className="result-check-slot">
          {selected && <Check size={16} aria-hidden="true" />}
        </span>
      </span>
      <strong className="result-duration">
        {formatMinutes(route.duration)}
      </strong>
      <span className="result-route-meta">
        <span>{formatKilometres(route.distance)}</span>
        <span>{formatExtraMinutes(route.extra_minutes)}</span>
      </span>
      <span className="result-route-count">
        過去事故 <b>{route.accident_count.toLocaleString("ja-JP")}</b> 件
      </span>
    </button>
  );
}

/** Rows of the side-by-side table for the one or two selected routes. */
const metricRows: [string, (route: RouteCandidate) => string][] = [
  ["所要時間", (route) => formatMinutes(route.duration)],
  ["距離", (route) => formatKilometres(route.distance)],
  ["最短比の追加時間", (route) => formatExtraMinutes(route.extra_minutes)],
  ["過去事故", (route) => `${route.accident_count.toLocaleString("ja-JP")}件`],
  [
    "死亡事故 / 負傷事故",
    (route) => `${route.fatal_accidents} / ${route.injury_accidents}件`,
  ],
  ["死者 / 負傷者", (route) => `${route.deaths} / ${route.injuries}人`],
  // weather_match=null (weather all) is not the same as zero matching accidents.
  [
    "指定天候と一致する事故",
    (route) =>
      route.weather_match
        ? `${route.weather_match.accident_count}件`
        : "天候指定なし",
  ],
  [
    "うち死亡事故 / 負傷事故",
    (route) =>
      route.weather_match
        ? `${route.weather_match.fatal_accidents} / ${route.weather_match.injury_accidents}件`
        : "天候指定なし",
  ],
  [
    "うち死者 / 負傷者",
    (route) =>
      route.weather_match
        ? `${route.weather_match.deaths} / ${route.weather_match.injuries}人`
        : "天候指定なし",
  ],
  ["重み", (route) => `${number.format(route.weighted_risk)}点`],
  ["重み / km", (route) => `${precise.format(route.risk_per_km)}点/km`],
  ["許容追加時間", (route) => (route.within_detour ? "範囲内" : "範囲外")],
  [
    "地図の事故点",
    (route) => `${route.points_shown} / ${route.accident_count}件`,
  ],
  [
    "注意区間",
    (route) =>
      `${route.high_risk_sections.features.length} / ${route.high_risk_total}区間`,
  ],
];

export function ResultsPanel({
  result,
  snapshot,
  selectedRouteId,
  onSelectRoute,
  selectedSectionId,
  onSelectSection,
  showAccidents,
  onShowAccidents,
  isDemo,
}: ResultsPanelProps) {
  const fastest = result.candidates.find(
    (candidate) => candidate.id === result.fastest_id,
  )!;
  const considered = result.candidates.find(
    (candidate) => candidate.id === result.safest_id,
  );
  const selected =
    result.candidates.find((candidate) => candidate.id === selectedRouteId) ??
    fastest;
  const same = considered?.id === fastest.id;
  const featured = considered ? [fastest, considered] : [fastest];
  const sections = selected.high_risk_sections.features;
  const conditions = result.effective_conditions;
  const thinned = featured.some(
    (route) => route.points_shown < route.accident_count,
  );
  const truncated = featured.some(
    (route) => route.high_risk_sections.features.length < route.high_risk_total,
  );
  const autoWeather = snapshot.auto.weather && conditions.weather !== "全天候";

  return (
    <div className="result-content">
      <section
        className="result-block"
        aria-labelledby="result-compare-heading"
      >
        <div className="result-section-heading">
          <h2 id="result-compare-heading">ルートを比較</h2>
          {isDemo && <span className="result-demo-label">デモデータ</span>}
        </div>
        <dl className="result-meanings">
          <div>
            <dt>{FASTEST}</dt>
            <dd>取得した候補の中で所要時間が最も短い経路です。</dd>
          </div>
          <div>
            <dt>{CONSIDERED}</dt>
            <dd>
              取得した候補の中から、過去の事故記録を考慮して選んだ経路です。全道路網の中から選んだ最適な経路ではありません。
            </dd>
          </div>
        </dl>
        <div
          className={`result-route-cards${considered && !same ? "" : " result-single-card"}`}
          role="group"
          aria-label="比較結果の経路"
        >
          {considered && !same && (
            <RouteCard
              route={considered}
              title={CONSIDERED}
              selected={selected.id === considered.id}
              onClick={() => onSelectRoute(considered.id)}
            />
          )}
          <RouteCard
            route={fastest}
            title={same ? `${FASTEST}・${CONSIDERED}` : FASTEST}
            selected={selected.id === fastest.id}
            onClick={() => onSelectRoute(fastest.id)}
          />
        </div>
        {same && (
          <p className="result-same">
            <Check size={15} aria-hidden="true" />
            今回は{FASTEST}と{CONSIDERED}が同じ経路です。
          </p>
        )}
        {result.candidates.length === 1 && (
          <p className="result-notice">
            <Info size={16} aria-hidden="true" />
            <span>経路候補が1本だけのため、他の経路と比較できません。</span>
          </p>
        )}
        {!considered && (
          <p className="result-notice">
            <Info size={16} aria-hidden="true" />
            <span>
              全候補で該当する事故が0件のため、事故を考慮した経路は選んでいません。事故0件は安全の証明ではありません。
            </span>
          </p>
        )}
        <p className="result-reason">{result.reason}</p>
        {result.warnings.length > 0 && (
          <ul className="result-warnings" aria-label="検索結果の注意事項">
            {result.warnings.map((warning, index) => (
              <li key={`${index}-${warning}`}>
                <AlertTriangle size={15} aria-hidden="true" />
                <span>{warning}</span>
              </li>
            ))}
          </ul>
        )}
        <p className="result-risk-note">{result.risk_note}</p>
      </section>

      <section
        className="result-block"
        aria-labelledby="result-metrics-heading"
      >
        <div className="result-section-heading">
          <h2 id="result-metrics-heading">2経路の指標</h2>
        </div>
        <div
          className="result-table-scroll"
          role="region"
          aria-label="2経路の指標（横スクロール）"
          tabIndex={0}
        >
          <table className="result-table result-compare-table">
            <thead>
              <tr>
                <th scope="col">項目</th>
                <th scope="col">{FASTEST}</th>
                {considered && <th scope="col">{CONSIDERED}</th>}
              </tr>
            </thead>
            <tbody>
              {metricRows.map(([label, value]) => (
                <tr key={label}>
                  <th scope="row">{label}</th>
                  {featured.map((route, index) => (
                    <td key={index}>{value(route)}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <dl className="result-metrics">
          <div>
            <dt>最短比の重み減少率</dt>
            <dd>{reductionText(result, fastest)}</dd>
          </div>
          <div>
            <dt>事故考慮経路の追加時間</dt>
            <dd>
              {result.extra_minutes === null
                ? "対象なし"
                : formatExtraMinutes(result.extra_minutes)}
            </dd>
          </div>
        </dl>
        {thinned && (
          <p className="result-notice">
            <Info size={16} aria-hidden="true" />
            <span>
              地図の事故点は各経路で死亡事故を優先して最大250件まで表示しています（表示数/該当件数は上の表）。候補間で重複する事故は地図上で1つにまとめています。
            </span>
          </p>
        )}
        {truncated && (
          <p className="result-notice">
            <Info size={16} aria-hidden="true" />
            <span>
              注意区間は各経路で上位100区間だけを表示しています（表示数/全区間数は上の表）。
            </span>
          </p>
        )}
      </section>

      <section
        className="result-block"
        aria-labelledby="result-sections-heading"
      >
        <div className="result-section-heading">
          <h2 id="result-sections-heading">注意箇所</h2>
          <span className="result-muted">{sections.length}区間</span>
        </div>
        {sections.length ? (
          <ul className="result-sections-list">
            {sections.map((section) => (
              <li key={section.id}>
                <button
                  type="button"
                  className={`result-section-button${selectedSectionId === section.id ? " result-section-active" : ""}`}
                  aria-pressed={selectedSectionId === section.id}
                  onClick={() =>
                    onSelectSection(
                      selectedSectionId === section.id ? null : section.id,
                    )
                  }
                >
                  <AlertTriangle size={18} aria-hidden="true" />
                  <span>
                    <b>区間 {section.properties.segment_index + 1}</b>
                  </span>
                  <ArrowUpRight size={16} aria-hidden="true" />
                </button>
              </li>
            ))}
          </ul>
        ) : (
          <p className="result-no-sections">
            表示対象の注意区間はありません。安全であることを示すものではありません。
          </p>
        )}
        <label className="result-toggle-row">
          <span>事故点を地図に表示</span>
          <input
            type="checkbox"
            role="switch"
            checked={showAccidents}
            onChange={(event) => onShowAccidents(event.target.checked)}
          />
          <span className="result-switch-track" aria-hidden="true" />
        </label>
      </section>

      <section
        className="result-block"
        aria-labelledby="result-applied-heading"
      >
        <div className="result-section-heading">
          <h2 id="result-applied-heading">適用された条件</h2>
        </div>
        <dl className="result-metrics">
          <div>
            <dt>出発日時</dt>
            <dd>
              {snapshot.departure === "now"
                ? `現在（${dateLabel(snapshot.dateTime)}の検索時点）`
                : `指定：${dateLabel(snapshot.dateTime)}`}
            </dd>
          </div>
          <div>
            <dt>移動手段</dt>
            <dd>{modeLabels[conditions.mode]}</dd>
          </div>
          <div>
            <dt>時間帯</dt>
            <dd>
              {conditions.period}（{source(snapshot.auto.period)}）
            </dd>
          </div>
          <div>
            <dt>曜日区分</dt>
            <dd>
              {conditions.day_type}（{source(snapshot.auto.day_type)}）
            </dd>
          </div>
          <div>
            <dt>天候</dt>
            <dd>
              {conditions.weather}（{source(snapshot.auto.weather)}）
            </dd>
          </div>
        </dl>
        <p className="result-muted">
          条件は自動で緩和しません。天候を指定しても集計する事故は減りません。雨・霧・雪を指定した場合だけ同じ天候の事故を重く数え、晴・曇では「指定天候と一致する事故」の集計だけが変わります。
        </p>
        {autoWeather && (
          <p className="result-muted">
            事故記録の天候は発生時の現場での判定で、自動取得した天候は気象モデルの推定値です。両者は同じ尺度ではありません。
          </p>
        )}
      </section>

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
