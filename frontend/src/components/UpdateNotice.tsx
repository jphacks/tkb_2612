import { useState, useSyncExternalStore } from "react";
import { applyUpdate, getUpdateSnapshot, subscribeUpdates, type UpdateOutcome } from "../pwa/register";

const OUTCOME_MESSAGES: Partial<Record<UpdateOutcome, string>> = {
  "other-windows": "ほかのタブやウィンドウでもこのアプリを開いているため切り替えられません。ほかを閉じてから再読込してください。",
  unavailable: "更新を適用できませんでした。もう一度お試しください。",
};

/**
 * 新しいバージョンの待機を知らせる。検索・GPS取得・ナビ中（一時停止を含む）は切り替えず、
 * 終了後に利用者が同意した再読込だけで、待機中の版へ切り替えて更新する。
 */
export function UpdateNotice() {
  const update = useSyncExternalStore(subscribeUpdates, getUpdateSnapshot);
  const [switching, setSwitching] = useState(false);
  const [outcome, setOutcome] = useState<UpdateOutcome | null>(null);
  if (!update.waiting) return null;
  const message = outcome ? OUTCOME_MESSAGES[outcome] : undefined;
  const onReload = async () => {
    setSwitching(true);
    setOutcome(null);
    const result = await applyUpdate();
    if (result !== "reloading") {
      setSwitching(false);
      setOutcome(result);
    }
  };
  return (
    <div className="update-notice" role="status">
      {update.canReload ? (
        <>
          <p>新しいバージョンがあります。再読込できます。</p>
          <p>再読込すると、入力した地点・検索結果・経路が消えます。</p>
          {message ? <p>{message}</p> : null}
          <button type="button" className="text-button" disabled={switching} onClick={() => void onReload()}>
            再読込
          </button>
        </>
      ) : (
        <p>新しいバージョンがあります。検索またはナビの終了後に再読込できます。</p>
      )}
    </div>
  );
}
