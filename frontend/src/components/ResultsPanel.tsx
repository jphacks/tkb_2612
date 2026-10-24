import {
  AlertTriangle,
  ArrowUpRight,
  Check,
  ChevronDown,
  Info,
} from "lucide-react";
import type { RouteCandidate, RouteResponse } from "../lib/types";
import { formatDistance, formatDuration } from "../lib/format";
import "./results.css";

export interface ResultsPanelProps {
  result: RouteResponse;
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

function extraTime(minutes: number) {
  return minutes === 0 ? "+0分" : `+${precise.format(minutes)}分`;
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
        {formatDuration(route.duration)}
      </strong>
      <span className="result-route-meta">
        <span>{formatDistance(route.distance)}</span>
        <span>{extraTime(route.extra_minutes)}</span>
      </span>
      <span className="result-route-count">
        過去事故 <b>{route.accident_count.toLocaleString("ja-JP")}</b> 件
      </span>
    </button>
  );
}

export function ResultsPanel({
  result,
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
  const sections = selected.high_risk_sections.features;
  const conditions = result.effective_conditions;
  const reduction =
    fastest.weighted_risk > 0 && result.risk_reduction_percent !== null
      ? `${number.format(result.risk_reduction_percent)}%`
      : "算出不可";

  return (
    <div className="result-content">
      <section className="result-block" aria-label="経路の比較">
        <div className="result-section-heading">
          <h2>ルートを比較</h2>
          {isDemo && <span className="result-demo-label">デモデータ</span>}
        </div>
        <div
          className={`result-route-cards${considered ? "" : " result-single-card"}`}
          role="group"
          aria-label="表示する経路"
        >
          {considered && (
            <RouteCard
              route={considered}
              title="事故履歴を考慮"
              selected={selected.id === considered.id}
              onClick={() => onSelectRoute(considered.id)}
            />
          )}
          <RouteCard
            route={fastest}
            title="最短時間"
            selected={selected.id === fastest.id}
            onClick={() => onSelectRoute(fastest.id)}
          />
        </div>
        {same && (
          <p className="result-same">
            <Check size={15} aria-hidden="true" />
            同一経路
          </p>
        )}
        {!considered && (
          <p className="result-notice">
            <Info size={16} aria-hidden="true" />
            <span>
              全候補の該当事故が0件のため、事故履歴から優先経路を判断できません。事故0件は安全の証明ではありません。
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
                    <span>この区間付近に過去の事故記録があります</span>
                  </span>
                  <ArrowUpRight size={16} aria-hidden="true" />
                </button>
              </li>
            ))}
          </ul>
        ) : (
          <p className="result-no-sections">
            表示対象の注意区間はありません。事故記録が存在しないことを示すものではありません。
          </p>
        )}
        {selected.high_risk_total > sections.length && (
          <p className="result-muted result-cap-note">
            全{selected.high_risk_total}区間中、重みの高い{sections.length}
            区間を表示しています。
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

      <p className="result-risk-note">{result.risk_note}</p>

      <details className="result-details">
        <summary>
          <span>指標と詳細</span>
          <ChevronDown size={18} aria-hidden="true" />
        </summary>
        <div className="result-details-body">
          <h3>適用された条件</h3>
          <p className="result-applied-conditions">
            {modeLabels[conditions.mode]} / {conditions.period} /{" "}
            {conditions.day_type} / {conditions.weather}
          </p>
          <p className="result-muted">
            条件の自動緩和は行っていません。天候は事故件数を絞らず、重み付けに使用します。
          </p>

          <h3>選択経路の集計</h3>
          <dl className="result-metrics">
            <div>
              <dt>過去事故</dt>
              <dd>{selected.accident_count.toLocaleString("ja-JP")}件</dd>
            </div>
            <div>
              <dt>死亡事故 / 負傷事故</dt>
              <dd>
                {selected.fatal_accidents} / {selected.injury_accidents}件
              </dd>
            </div>
            <div>
              <dt>死者 / 負傷者</dt>
              <dd>
                {selected.deaths} / {selected.injuries}人
              </dd>
            </div>
            <div>
              <dt>重み</dt>
              <dd>{number.format(selected.weighted_risk)}点</dd>
            </div>
            <div>
              <dt>重み / km</dt>
              <dd>{precise.format(selected.risk_per_km)}点/km</dd>
            </div>
            <div>
              <dt>許容追加時間</dt>
              <dd>{selected.within_detour ? "範囲内" : "範囲外"}</dd>
            </div>
            <div>
              <dt>地図の事故点</dt>
              <dd>
                {selected.points_shown} / {selected.accident_count}件
              </dd>
            </div>
            <div>
              <dt>注意区間</dt>
              <dd>
                {sections.length} / {selected.high_risk_total}区間
              </dd>
            </div>
          </dl>
          <p className="result-muted">
            事故点は最大250件、注意区間は重みの高い順に最大100区間を表示します。集計値は表示上限で間引く前の値です。
          </p>
          <p className="result-muted">
            区間の重みは死者×10＋負傷者の合計です。経路の重みは、さらに距離と天候で重み付けしています。
          </p>

          <h3>指定天候と一致する事故</h3>
          {selected.weather_match ? (
            <dl className="result-metrics">
              <div>
                <dt>過去事故</dt>
                <dd>{selected.weather_match.accident_count}件</dd>
              </div>
              <div>
                <dt>死亡事故 / 負傷事故</dt>
                <dd>
                  {selected.weather_match.fatal_accidents} /{" "}
                  {selected.weather_match.injury_accidents}件
                </dd>
              </div>
              <div>
                <dt>死者 / 負傷者</dt>
                <dd>
                  {selected.weather_match.deaths} /{" "}
                  {selected.weather_match.injuries}人
                </dd>
              </div>
            </dl>
          ) : (
            <p className="result-muted">
              天候指定なしのため、天候別の一致集計はありません。
            </p>
          )}

          <h3>比較と集計条件</h3>
          <dl className="result-metrics">
            <div>
              <dt>最短比の重み減少率</dt>
              <dd>{reduction}</dd>
            </div>
            <div>
              <dt>事故考慮経路の追加時間</dt>
              <dd>
                {result.extra_minutes === null
                  ? "対象なし"
                  : extraTime(result.extra_minutes)}
              </dd>
            </div>
            <div>
              <dt>集計半径</dt>
              <dd>{result.radius_m}m</dd>
            </div>
            <div>
              <dt>距離重みのσ</dt>
              <dd>{result.distance_weight_sigma_m}m</dd>
            </div>
            <div>
              <dt>データ年</dt>
              <dd>{result.data_years.join("・")}年</dd>
            </div>
            <div>
              <dt>経路候補</dt>
              <dd>{result.candidates.length}本</dd>
            </div>
            <div>
              <dt>集計・選定時間</dt>
              <dd>{number.format(result.aggregation_ms)}ms</dd>
            </div>
          </dl>
          <p className="result-muted">
            重み減少率は事故考慮経路と最短時間経路の比較です。基準の重みが0、または事故考慮経路がない場合は算出不可です。集計・選定時間には外部通信と転送を含みません。
          </p>

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
                    <td>{formatDuration(candidate.duration)}</td>
                    <td>{formatDistance(candidate.distance)}</td>
                    <td>{extraTime(candidate.extra_minutes)}</td>
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
      {!isDemo && (
        <footer className="result-attribution">
          事故データ：
          <a
            href="https://www.npa.go.jp/publications/statistics/koutsuu/opendata/index_opendata.html"
            target="_blank"
            rel="noreferrer"
          >
            警察庁交通事故統計オープンデータ
          </a>
          （{result.data_years.join("・")}年）
          <br />
          <a href="https://open-meteo.com/" target="_blank" rel="noreferrer">
            Weather data by Open-Meteo.com
          </a>
          （CC BY 4.0）
          <br />
          祝日データ：
          <a
            href="https://www8.cao.go.jp/chosei/shukujitsu/gaiyou.html"
            target="_blank"
            rel="noreferrer"
          >
            内閣府「国民の祝日」
          </a>
          <br />
          経路：
          <a href="https://project-osrm.org/" target="_blank" rel="noreferrer">
            OSRM
          </a>{" "}
          /{" "}
          <a
            href="https://www.openstreetmap.org/copyright"
            target="_blank"
            rel="noreferrer"
          >
            OpenStreetMap contributors
          </a>
        </footer>
      )}
    </div>
  );
}

export default ResultsPanel;
