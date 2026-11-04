import { X } from "lucide-react";
import type { SafetyReport } from "../community/reports";
import { SafetyIcon } from "./icons/SafetyIcon";

export function RecordedAt({ value }: { value: string | null }) {
  return value ? (
    <time dateTime={value}>
      {new Intl.DateTimeFormat("ja-JP", {
        timeZone: "Asia/Tokyo",
        year: "numeric",
        month: "2-digit",
        day: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
        hour12: false,
      }).format(new Date(value))}
    </time>
  ) : (
    <span>日時不明</span>
  );
}

export function CommunityReview({
  report,
  onClose,
  onSupport,
  error,
  supported,
}: {
  report: SafetyReport;
  onClose: () => void;
  onSupport: () => void;
  error: string;
  supported: boolean;
}) {
  const comments = [
    ...(report.text
      ? [{ id: report.id, text: report.text, createdAt: report.createdAt }]
      : []),
    ...report.comments,
  ];
  return (
    <section className="community-review" aria-label="口コミ">
      <div className="community-review-meta">
        <SafetyIcon kind={report.kind} size={32} count={report.supportCount} />
        <div>
          <span>記入日時</span>
          <RecordedAt value={report.createdAt} />
        </div>
      </div>
      <button
        type="button"
        className="icon-button community-close"
        aria-label="口コミを閉じる"
        onClick={onClose}
      >
        <X size={18} />
      </button>
      {comments.length > 0 && (
        <div className="community-review-detail">
          <h2>詳細情報</h2>
          <ol className="community-comment-list">
            {comments.map((comment) => (
              <li key={comment.id}>
                {comment.id !== report.id && (
                  <RecordedAt value={comment.createdAt} />
                )}
                <p>{comment.text}</p>
              </li>
            ))}
          </ol>
        </div>
      )}
      {error && <p role="alert">{error}</p>}
      <button
        type="button"
        className="community-support-button"
        onClick={onSupport}
        disabled={supported}
        aria-pressed={supported}
      >
        共感
      </button>
    </section>
  );
}
