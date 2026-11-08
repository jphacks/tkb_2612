import { ShieldCheck, TriangleAlert } from "lucide-react";

export type SafetyKind = "safe" | "danger";
export const SAFETY_COLORS: Record<SafetyKind, string> = {
  safe: "#16A34A",
  danger: "#DC2626",
};
export const SAFETY_LABELS: Record<SafetyKind, string> = {
  safe: "安全",
  danger: "危険",
};

export function SafetyIcon({
  kind,
  size = 32,
  count = 1,
}: {
  kind: SafetyKind;
  size?: number;
  count?: number;
}) {
  const Icon = kind === "safe" ? ShieldCheck : TriangleAlert;
  return (
    <span className="safety-icon" style={{ width: size, height: size }}>
      <Icon
        size={size}
        color={SAFETY_COLORS[kind]}
        fill="none"
        strokeWidth={2}
        aria-hidden="true"
      />
      {count > 1 && (
        <span
          className="safety-icon-count"
          style={{ borderColor: SAFETY_COLORS[kind] }}
          aria-label={`${count}票`}
        >
          {count}
        </span>
      )}
    </span>
  );
}
