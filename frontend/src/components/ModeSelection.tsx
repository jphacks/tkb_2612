import { useEffect, useRef, useState } from "react";
import { Route, UsersRound } from "lucide-react";
import "./community.css";

export type AppMode = "navigation" | "community";

export function ModeSelection({
  onSelect,
  visible,
}: {
  onSelect: (mode: AppMode) => void;
  visible: boolean;
}) {
  const [active, setActive] = useState<AppMode>("navigation");
  const [armed, setArmed] = useState<AppMode | null>(null);
  const [chosen, setChosen] = useState<AppMode | null>(null);
  const container = useRef<HTMLElement>(null);
  useEffect(() => {
    if (visible) container.current?.focus();
  }, [visible]);
  useEffect(() => {
    if (!chosen) return;
    const timer = window.setTimeout(() => onSelect(chosen), 150);
    return () => window.clearTimeout(timer);
  }, [chosen, onSelect]);

  return (
    <main
      className="mode-selection"
      ref={container}
      tabIndex={-1}
      aria-label="モード選択"
    >
      <div className="mode-options">
        {(
          [
            ["navigation", "安全経路選択ナビモード", Route],
            ["community", "みんなの安全情報モード", UsersRound],
          ] as const
        ).map(([mode, label, Icon]) => (
          <button
            key={mode}
            type="button"
            className={`mode-option mode-option-${mode}${active === mode ? " is-active" : ""}`}
            aria-pressed={active === mode}
            onClick={() => {
              if (active === mode && armed === mode) setChosen(mode);
              else {
                setActive(mode);
                setArmed(mode);
              }
            }}
          >
            <Icon size={32} strokeWidth={1.8} aria-hidden="true" />
            <span>{label}</span>
          </button>
        ))}
      </div>
    </main>
  );
}
