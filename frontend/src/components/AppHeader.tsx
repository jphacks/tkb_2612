import { isDemo } from "../api/client";

export function AppHeader({
  onRestart,
  showBadge = false,
}: {
  onRestart: () => void;
  showBadge?: boolean;
}) {
  return (
    <header className="app-header">
      <button
        className="wordmark"
        type="button"
        onClick={onRestart}
        aria-label="AcciLessNavi ホームへ"
      >
        AcciLess<span>Navi</span>
      </button>
      {showBadge && (
        <span className={`mode-badge ${isDemo ? "" : "live"}`}>
          {isDemo ? "デモデータ" : "API接続"}
        </span>
      )}
    </header>
  );
}
