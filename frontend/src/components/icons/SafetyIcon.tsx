import { ShieldAlert, ShieldCheck } from "lucide-react";

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
}: {
  kind: SafetyKind;
  size?: number;
}) {
  const Icon = kind === "safe" ? ShieldCheck : ShieldAlert;
  return (
    <Icon
      size={size}
      color={SAFETY_COLORS[kind]}
      fill="none"
      strokeWidth={2}
      aria-hidden="true"
    />
  );
}
