import type { ReactNode } from "react";
import { ArrowUp, CornerUpLeft, CornerUpRight, Flag, LocateFixed, Navigation, Pause } from "lucide-react";
import type { NavigationGuidance } from "../navigation/guidance";
import { NavigationAlertPopup, type NavigationAlert } from "./NavigationAlertPopup";
import "./navigation-guidance.css";

interface Props {
  alert?: NavigationAlert;
  guidance?: NavigationGuidance | null;
  announcement?: string;
  banner?: ReactNode;
}

const maneuvers = {
  left: { label: "左折", Icon: CornerUpLeft },
  right: { label: "右折", Icon: CornerUpRight },
  straight: { label: "直進", Icon: ArrowUp },
  arrive: { label: "目的地", Icon: Flag },
};

function distanceLabel(distanceM: number): string {
  if (distanceM < 10) return "まもなく";
  if (distanceM >= 1000) return `${(Math.ceil(distanceM / 100) / 10).toFixed(1)} km先`;
  return `${Math.ceil(distanceM / 10) * 10} m先`;
}

/** 地図中央を空ける受動表示。位置確認中・一時停止中は古い右左折の案内を出さない。 */
export function NavigationGuidanceBar({ alert, guidance, announcement = "", banner }: Props) {
  const active = !!alert;
  const direction = active && alert.phase === "tracking" && guidance ? maneuvers[guidance.maneuver as keyof typeof maneuvers] : null;
  let label = "経路に沿って進む";
  let Icon = Navigation;
  if (direction) {
    label = direction.label;
    Icon = direction.Icon;
  } else if (alert?.phase === "arrived") {
    label = "目的地に到着しました";
    Icon = Flag;
  } else if (alert?.phase === "paused") {
    label = "一時停止中";
    Icon = Pause;
  } else if (alert?.phase === "deviated") {
    label = "経路から外れています";
  } else if (alert && alert.phase !== "tracking") {
    label = "位置を確認中";
    Icon = LocateFixed;
  }
  return (
    <div className="nav-guidance-overlay">
      {banner}
      <div className={active ? "nav-guidance-bar" : undefined} role={active ? "region" : undefined} aria-label={active ? "ナビ案内" : undefined}>
        {active && <Icon className="nav-guidance-icon" size={32} aria-hidden="true" />}
        <div className="nav-guidance-content">
          {active && (
            <div className="nav-guidance-instruction">
              {direction && guidance && <strong className="nav-guidance-distance">{distanceLabel(guidance.distanceM)}</strong>}
              <span className="nav-guidance-action" aria-live="polite">{label}</span>
            </div>
          )}
          <NavigationAlertPopup alert={active ? alert : undefined} announcement={alert ? announcement : ""} />
        </div>
      </div>
    </div>
  );
}
