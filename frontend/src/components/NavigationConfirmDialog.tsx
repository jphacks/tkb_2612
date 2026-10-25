import { useEffect, useRef } from "react";
import type { NavigationKind } from "../navigation/session";

export const SAFETY_NOTICE =
  "過去事故の参考情報で、安全を保証しません。自動車・自転車の運転中は画面を注視せず、端末を手に持って操作しないでください。設定・再検索・再開は安全な場所で停止してから行ってください。道路標識と周囲の状況を優先してください";
export const WALKING_NOTICE = "徒歩の場合も、歩きながら画面を注視しないでください。";

export interface NavigationConfirmDialogProps {
  /** 例: 最短ルート */
  selectionLabel: string;
  kind: NavigationKind;
  onConfirm: () => void;
  onCancel: () => void;
}

/** 開始前の注意。確認の操作があるまでナビを開始しない。 */
export function NavigationConfirmDialog({
  selectionLabel,
  kind,
  onConfirm,
  onCancel,
}: NavigationConfirmDialogProps) {
  const dialog = useRef<HTMLDivElement>(null);
  const confirm = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    confirm.current?.focus();
    return () => previous?.focus?.();
  }, []);

  function onKeyDown(event: React.KeyboardEvent) {
    if (event.key === "Escape") {
      event.stopPropagation();
      onCancel();
      return;
    }
    if (event.key !== "Tab") return;
    const items = [...(dialog.current?.querySelectorAll<HTMLElement>("button") ?? [])];
    if (!items.length) return;
    const first = items[0];
    const last = items[items.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  return (
    <div className="nav-dialog-backdrop">
      <div
        ref={dialog}
        className="nav-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="nav-dialog-title"
        aria-describedby="nav-dialog-notice"
        onKeyDown={onKeyDown}
      >
        <h2 id="nav-dialog-title">ナビを開始する前に</h2>
        <div id="nav-dialog-notice" className="nav-dialog-notice">
          <p>{SAFETY_NOTICE}</p>
          <p>{WALKING_NOTICE}</p>
        </div>
        <p className="nav-dialog-target">
          {selectionLabel}を{kind === "simulation" ? "模擬走行" : "実機GPS"}で開始します
        </p>
        <div className="nav-dialog-actions">
          <button type="button" className="secondary" onClick={onCancel}>
            キャンセル
          </button>
          <button ref={confirm} type="button" className="primary" onClick={onConfirm}>
            確認して開始
          </button>
        </div>
      </div>
    </div>
  );
}
