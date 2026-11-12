import { X } from "lucide-react";
import type { SafetyReport } from "../lib/types";
import { tallyLabel } from "../community/reports";
import { SAFETY_COLORS, SAFETY_LABELS } from "./icons/SafetyIcon";

/** ポップアップに出す口コミの本文の件数（新しい順）。残りは件数だけ出す。 */
export const REPORT_POPUP_COMMENTS = 3;

function tokyoParts(value: string) {
  const parts = new Intl.DateTimeFormat("ja-JP", {
    timeZone: "Asia/Tokyo",
    year: "numeric",
    month: "numeric",
    day: "numeric",
  }).formatToParts(new Date(value));
  const part = (type: Intl.DateTimeFormatPartTypes) =>
    parts.find((item) => item.type === type)?.value ?? "";
  return { year: part("year"), month: part("month"), day: part("day") };
}

/** 事故記録のポップアップと同じ「YYYY年M月D日」（日本時間）。 */
function dateLabel(value: string) {
  const { year, month, day } = tokyoParts(value);
  return `${year}年${month}月${day}日`;
}

function shortDateLabel(value: string) {
  const { month, day } = tokyoParts(value);
  return `${month}/${day}`;
}

/** 経路の地図で口コミの地点を押したときの表示（見るだけ。投票は共有モードで行う）。 */
export function ReportPopup({
  report,
  onClose,
}: {
  report: SafetyReport;
  onClose: () => void;
}) {
  // commentsは古い順に届く
  const latest = report.comments.slice(-REPORT_POPUP_COMMENTS).reverse();
  const rest = report.comments.length - latest.length;
  return (
    <section
      className="map-accident-popup map-report-popup"
      aria-label="口コミ"
    >
      <div className="map-popup-heading">
        <strong>口コミ</strong>
        <button type="button" onClick={onClose} aria-label="口コミを閉じる">
          <X size={18} />
        </button>
      </div>
      <dl>
        <dt>投稿日</dt>
        <dd>
          <time dateTime={report.created_at}>
            {dateLabel(report.created_at)}
          </time>
        </dd>
        <dt>票数</dt>
        <dd aria-label={tallyLabel(report)}>
          {(["safe", "danger"] as const).map((kind) => (
            <span
              key={kind}
              className="map-report-tally"
              style={{ color: SAFETY_COLORS[kind] }}
            >
              {SAFETY_LABELS[kind]} {kind === "safe" ? report.safe_count : report.danger_count}票
            </span>
          ))}
        </dd>
      </dl>
      {latest.length > 0 ? (
        <ol className="map-report-comments" aria-label="新しい口コミ">
          {latest.map((comment) => (
            <li key={comment.id}>
              <span
                className="map-report-kind"
                style={{ color: SAFETY_COLORS[comment.kind] }}
              >
                {SAFETY_LABELS[comment.kind]}
              </span>
              <time dateTime={comment.created_at}>
                {shortDateLabel(comment.created_at)}
              </time>
              <p>{comment.text}</p>
            </li>
          ))}
        </ol>
      ) : (
        <p className="map-report-empty">口コミの本文はありません</p>
      )}
      {rest > 0 && <p className="map-report-more">ほか {rest}件</p>}
    </section>
  );
}
