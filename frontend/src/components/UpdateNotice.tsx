import { useSyncExternalStore } from "react";
import { getUpdateSnapshot, subscribeUpdates } from "../pwa/register";

/**
 * 新しいバージョンの待機を知らせる。検索・GPS取得・ナビ中（一時停止を含む）は切り替えず、
 * 終了後に利用者が同意した再読込だけで更新する。
 */
export function UpdateNotice() {
  const update = useSyncExternalStore(subscribeUpdates, getUpdateSnapshot);
  if (!update.waiting) return null;
  return (
    <div className="update-notice" role="status">
      {update.canReload ? (
        <>
          <p>新しいバージョンがあります。再読込できます。</p>
          <p>再読込すると、入力した地点・検索結果・経路が消えます。</p>
          <button type="button" className="text-button" onClick={() => window.location.reload()}>
            再読込
          </button>
        </>
      ) : (
        <p>新しいバージョンがあります。検索またはナビの終了後に再読込できます。</p>
      )}
    </div>
  );
}
