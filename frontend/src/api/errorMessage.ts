import { ApiError } from "./errors";

/** Route-search hint for 400: the 30km straight-line limit is not the 80km route limit. */
export const ROUTE_INPUT_HINT =
  "入力を見直してください。出発地と目的地の直線距離の上限30kmは、候補経路の長さの上限80kmとは別の制限です。";

const RELOAD =
  "安全な場所で止まってから、ページを再読み込みしてログインし直してください。";

/** Messages for codes the frontend handles itself (frontend-spec 5, loading/empty/error table). */
const messages: Record<string, string> = {
  route_too_long:
    "候補経路の長さが80kmを超えました。出発地と目的地を近づけて再検索してください。",
  no_route:
    "経路が見つかりませんでした。出発地や目的地を道路に近い地点へ変更してください。",
  external_api_error:
    "経路などの外部サービスから正しい応答を得られませんでした。入力はそのままです。時間をおいて再試行してください。",
  service_busy:
    "サーバーが混み合っています。入力はそのままです。時間をおいて再試行してください。",
  data_unavailable:
    "事故データを準備しています。時間をおいて再試行してください。",
  internal_error:
    "サーバーで処理に失敗しました。入力はそのままです。時間をおいて再試行してください。",
  timeout:
    "通信がタイムアウトしました。入力はそのままです。時間をおいて再試行してください。",
  invalid_response: `応答を読み取れませんでした。ログインの有効期限が切れた可能性もあります。${RELOAD}`,
  auth_required: `ログインの有効期限が切れた可能性があります。${RELOAD}`,
  // A login redirect can surface as a CORS failure that looks like a network error (U3).
  network_error: `サーバーに接続できません。接続を確認して再試行してください。接続に問題がない場合は、ログインの有効期限が切れた可能性があります。${RELOAD}`,
};

/**
 * Text for a failed request. Branches on error.code only, never on message contents;
 * unknown codes show the server message with a retry hint.
 */
export function describeError(
  failure: unknown,
  fallback: string,
  /** Operation-specific guidance appended to 400 invalid_request. */
  inputHint = "入力を見直してください。",
): string {
  if (!(failure instanceof ApiError))
    return failure instanceof Error && failure.message
      ? failure.message
      : fallback;
  if (failure.code === "invalid_request")
    return `${failure.message} ${inputHint}`;
  return (
    messages[failure.code] ??
    `${failure.message} 時間をおいて再試行してください。`
  );
}
