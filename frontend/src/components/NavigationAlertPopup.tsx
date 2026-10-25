import { useEffect, useState } from "react";
import { Info } from "lucide-react";
import type { NavigationSnapshot } from "../navigation/session";

export type NavigationAlert = Pick<NavigationSnapshot, "phase" | "alertMessage" | "alertIds">;

/** 判定済みのナビ通知だけを受動表示する。進捗停止中は古い位置の通知を隠す。 */
export function NavigationAlertPopup({ alert, announcement = "" }: { alert?: NavigationAlert; announcement?: string }) {
  const [startNotice, setStartNotice] = useState("");
  useEffect(() => {
    setStartNotice(announcement);
    // 接近通知の更新時に開始文を繰り返し読み上げない。
    const timer = setTimeout(() => setStartNotice(""), 1000);
    return () => clearTimeout(timer);
  }, [announcement]);
  return (
    <div className="nav-approach-region"
      aria-live="polite" aria-atomic="true">
      {startNotice && <span className="sr-only">{startNotice}</span>}
      {alert?.phase === "tracking" && alert.alertMessage && (
        <section className="nav-alert" aria-label="接近通知">
          <Info size={16} aria-hidden="true" />
          <div>
            <p>{alert.alertMessage}</p>
            {alert.alertIds.length > 1 && (
              <p className="nav-alert-count">参考区間 {alert.alertIds.length}件</p>
            )}
          </div>
        </section>
      )}
    </div>
  );
}
