import { useEffect, useId, useRef } from "react";
import { SafetyIcon, type SafetyKind } from "./icons/SafetyIcon";

export function CommunityDetailsForm({
  kind,
  count = 1,
  value,
  onChange,
  onSkip,
  onConfirm,
  error,
}: {
  kind: SafetyKind;
  count?: number;
  value: string;
  onChange: (text: string) => void;
  onSkip: () => void;
  onConfirm: () => void;
  error: string;
}) {
  const id = useId();
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    input.current?.focus();
  }, []);
  return (
    <form
      className="community-composer community-support-form"
      aria-label="詳細情報の入力"
      onSubmit={(event) => {
        event.preventDefault();
        onConfirm();
      }}
    >
      <SafetyIcon kind={kind} count={count} />
      <div className="community-detail">
        <label htmlFor={id}>詳細情報を入力</label>
        <input
          id={id}
          ref={input}
          value={value}
          placeholder="例：夜は見通しが悪くて危険"
          maxLength={2000}
          onChange={(event) => onChange(event.target.value)}
          onKeyDown={(event) => {
            if (
              event.key === "Enter" &&
              (event.nativeEvent.isComposing || event.keyCode === 229)
            )
              event.preventDefault();
          }}
        />
        <div className="community-support-actions">
          <button type="button" className="secondary" onClick={onSkip}>
            スキップ
          </button>
          <button type="submit" className="primary" disabled={!value.trim()}>
            確定
          </button>
        </div>
      </div>
      {error && <p role="alert">{error}</p>}
    </form>
  );
}
