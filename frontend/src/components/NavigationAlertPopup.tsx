import { useEffect, useState } from "react";
import { TriangleAlert } from "lucide-react";
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
          <TriangleAlert size={16} aria-hidden="true" />
          <p>{alert.alertMessage === "過去事故の参考区間にいます"
            ? "事故多発区間を通過中です"
            : "この先に事故多発区間があります"}</p>
        </section>
      )}
    </div>
  );
}
