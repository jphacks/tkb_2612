import type { NavigationSelection } from "../navigation/preflight";
import { FASTEST_LABEL, MIDDLE_LABEL, SAFE_LABEL } from "./RouteCallouts";

export const SELECTION_LABEL: Record<NavigationSelection, string> = {
  fastest: FASTEST_LABEL,
  middle: MIDDLE_LABEL,
  safest: SAFE_LABEL,
};

