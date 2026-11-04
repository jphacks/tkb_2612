import { useId, useState } from "react";
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
  const reasonId = useId();
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const [reasonPinned, setReasonPinned] = useState(false);
  const showReason = supported && (hovered || focused || reasonPinned);
  const comments = [
    ...(report.text
      ? [{ id: report.id, text: report.text, createdAt: report.createdAt }]
      : []),
    ...report.comments,
  ];
  return (
    <div className="community-review-shell">
      <section className="community-review" aria-label="口コミ">
        <div className="community-review-meta">
          <SafetyIcon
            kind={report.kind}
            size={32}
            count={report.supportCount}
          />
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
        <div className="community-vote-action">
          <button
            type="button"
            className="community-support-button"
            onPointerEnter={() => setHovered(true)}
            onPointerLeave={() => setHovered(false)}
            onFocus={() => setFocused(true)}
            onBlur={() => setFocused(false)}
            onClick={() => {
              if (supported) setReasonPinned(true);
              else onSupport();
            }}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                setReasonPinned(false);
                setHovered(false);
                setFocused(false);
              }
            }}
            aria-disabled={supported}
            aria-pressed={supported}
            aria-describedby={showReason ? reasonId : undefined}
          >
            投票
          </button>
        </div>
      </section>
      {showReason && (
        <p
          id={reasonId}
          className="community-vote-notice"
          role="tooltip"
          aria-live="polite"
        >
          ※ 同一地点には一度しか入力できません
        </p>
      )}
    </div>
  );
}
