import type { RouteCandidate, RouteResponse } from "../lib/types";
import { FASTEST_LABEL, MIDDLE_LABEL, SAFE_LABEL } from "./RouteCallouts";

type Props = {
  result: RouteResponse;
};

export const COMPARISON_NOTICE =
  "過去の事故に基づく参考指標です。事故に遭う確率や安全性を保証するものではありません。";

const HEAVY_WEATHER = new Set(["雨", "霧", "雪"]);

function choiceSentence(
  result: RouteResponse,
  safest: RouteCandidate | undefined,
): string | null {
  if (result.safest_id === null || safest === undefined) return null;
  if (result.safest_id === result.fastest_id)
    return `遠回りの上限までの経路の中では、${FASTEST_LABEL}の経路が推定リスクもいちばん低い経路です。`;
  const minutes = result.extra_minutes ?? safest.extra_minutes;
  const longer =
    minutes < 1 ? "1分未満" : `${Math.round(minutes)}分`;
  if (result.risk_reduction_percent === null)
    return `${SAFE_LABEL}の経路は${FASTEST_LABEL}の経路より${longer}長くかかります。`;
  return `${SAFE_LABEL}の経路は${FASTEST_LABEL}の経路より${longer}長くかかりますが、推定リスクは${result.risk_reduction_percent.toFixed(1)}%低くなります。`;
}

/**
 * 推定リスクを、最短時間経路を100とした相対値で表す（最短時間経路は常に「100」、ほかは小数1桁）。
 * 最短時間経路の推定リスクが0以下で比べられないときは null を返す。
 */
export function relativeRisk(
  route: RouteCandidate,
  fastest: RouteCandidate,
): string | null {
  if (route.id === fastest.id) return "100";
  if (!(fastest.weighted_risk > 0)) return null;
  return ((100 * route.weighted_risk) / fastest.weighted_risk).toFixed(1);
}

export function RouteResultNotes({ result }: Props) {
  const safest = result.candidates.find(
    (route) => route.id === result.safest_id,
  );
  const fastest = result.candidates.find(
    (route) => route.id === result.fastest_id,
  );
  const middle = result.candidates.find(
    (route) => route.id === result.middle_id,
  );
  const accidentCountReversed =
    safest !== undefined &&
    fastest !== undefined &&
    safest.accident_count > fastest.accident_count;
  const choice = choiceSentence(result, safest);
  const rows: { label: string; route: RouteCandidate }[] = [];
  if (fastest !== undefined) {
    rows.push({
      label:
        fastest.id === result.safest_id
          ? `${FASTEST_LABEL}・${SAFE_LABEL}`
          : FASTEST_LABEL,
      route: fastest,
    });
  }
  if (
    middle !== undefined &&
    middle.id !== result.fastest_id &&
    middle.id !== result.safest_id
  ) {
    rows.push({ label: MIDDLE_LABEL, route: middle });
  }
  if (safest !== undefined && safest.id !== result.fastest_id) {
    rows.push({ label: SAFE_LABEL, route: safest });
  }
  const { mode, weather } = result.effective_conditions;

  return (
    <section className="route-result-notes" aria-label="検索結果の説明">
      <p>{COMPARISON_NOTICE}</p>
      {result.safest_id === null && (
        <p>全候補で該当する事故がないため、{SAFE_LABEL}の経路は選んでいません。</p>
      )}
      {accidentCountReversed && (
        <p>
          {SAFE_LABEL}の経路は、過去事故の件数が{FASTEST_LABEL}の経路より多くなっています。件数は道路の種類による偶然のばらつきを含むため、推定リスクでは低い経路を選んでいます。
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
        {choice && <p>{choice}</p>}
        <table className="route-result-table">
          <thead>
            <tr>
              <th scope="col">経路</th>
              <th scope="col">推定リスク（{FASTEST_LABEL}の経路=100）</th>
              <th scope="col">事故件数</th>
            </tr>
          </thead>
          <tbody>
            {rows.map(({ label, route }) => (
              <tr key={route.id}>
                <th scope="row">{label}</th>
                <td>
                  {(fastest && relativeRisk(route, fastest)) || "—"}
                </td>
                <td>
                  {route.total_accident_count === null
                    ? "—"
                    : `${route.total_accident_count}件`}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        <h3 className="route-result-heading">推定リスクの求め方</h3>
        <p>
          過去の事故を、起きた道路や交差点ごとに数えています。件数には偶然の多い少ないがあるため、同じ種類の道路の平均を使ってならしています。死者や負傷者の多い事故ほど重く数えています。
        </p>
        {mode === "car" && (
          <p>
            生活道路は交通量が少ないぶん件数が少なく出るため、重く数えています。
          </p>
        )}
        {HEAVY_WEATHER.has(weather) && (
          <p>
            雨・霧・雪を指定したときは、同じ天候で起きた事故をさらに重く数えています。
          </p>
        )}
      </details>
    </section>
  );
}
