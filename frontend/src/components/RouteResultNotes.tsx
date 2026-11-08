import type { RouteResponse } from "../lib/types";

type Props = {
  result: RouteResponse;
};

const RISK_REDUCTION_UNKNOWN = "算出できません";

export function RouteResultNotes({ result }: Props) {
  const safest = result.candidates.find(
    (route) => route.id === result.safest_id,
  );
  const fastest = result.candidates.find(
    (route) => route.id === result.fastest_id,
  );
  const accidentCountReversed =
    safest !== undefined &&
    fastest !== undefined &&
    safest.accident_count > fastest.accident_count;
  const riskReduction =
    result.risk_reduction_percent === null
      ? RISK_REDUCTION_UNKNOWN
      : `${result.risk_reduction_percent.toFixed(1)}%`;

  return (
    <section className="route-result-notes" aria-label="検索結果の説明">
      {result.safest_id === null && (
        <p>全候補で該当する事故がないため、安全ルートは選んでいません。</p>
      )}
      {accidentCountReversed && (
        <p>
          安全ルートは、過去事故の件数が最短ルートより多くなっています。件数は道路の種類による偶然のばらつきを含むため、推定リスクでは低い経路を選んでいます。
        </p>
      )}
      {result.warnings.length > 0 && (
        <ul>
          {result.warnings.map((warning, index) => (
            <li key={index}>{warning}</li>
          ))}
        </ul>
      )}
      <details>
        <summary>この結果について</summary>
        <p>{result.reason}</p>
        <p>{result.risk_note}</p>
        <dl>
          <dt>推定リスクの減少率</dt>
          <dd>{riskReduction}</dd>
          <dt>候補数</dt>
          <dd>{result.candidates.length}本</dd>
          <dt>帰属不明の事故を数える範囲</dt>
          <dd>経路から{result.unattributed_radius_m}m以内</dd>
          <dt>集計時間</dt>
          <dd>{Math.round(result.aggregation_ms)}ms</dd>
          {result.data_years.length > 0 && (
            <>
              <dt>対象データ年</dt>
              <dd>{result.data_years.join("・")}年</dd>
            </>
          )}
        </dl>
      </details>
    </section>
  );
}
