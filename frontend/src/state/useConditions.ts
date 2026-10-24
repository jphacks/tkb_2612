import { useEffect, useRef, useState } from "react";
import { getConditions } from "../api/client";
import type { AutoFields } from "../components/ConditionFields";
import { toTokyoInput, toTokyoIso } from "../lib/format";
import type {
  ConditionValues,
  ConditionsResponse,
  LatLon,
  Scenario,
} from "../lib/types";

export type Departure = "now" | "custom";

const initialValues: ConditionValues = {
  mode: "walk",
  period: "all",
  day_type: "all",
  weather: "all",
  max_detour_min: 10,
};

const weatherReasons = {
  no_location: "両地点を確定すると天気を取得できます。",
  past: "過去の日時の天気は自動設定できません。",
  out_of_range: "指定日時は天気予報の対象期間外です。",
  unavailable: "天気の取得元を利用できません。",
};

type Options = {
  origin: LatLon | null;
  dest: LatLon | null;
  scenario: Scenario;
  /** Called before any user edit; the previous comparison no longer matches the inputs. */
  onEdit: () => void;
};

/**
 * frontend-spec 4: departure time and the auto/manual merge of /api/conditions.
 * Manual values are never overwritten, and older responses are discarded by generation.
 */
export function useConditions({ origin, dest, scenario, onEdit }: Options) {
  const [values, setValues] = useState<ConditionValues>(initialValues);
  const valuesRef = useRef(values);
  const [auto, setAuto] = useState<AutoFields>({
    period: true,
    day_type: true,
    weather: true,
  });
  const autoRef = useRef(auto);
  const [departure, setDeparture] = useState<Departure>("now");
  const [dateTime, setDateTime] = useState(() => toTokyoInput(new Date()));
  const [busy, setBusy] = useState(false);
  const [warning, setWarning] = useState("");
  const [info, setInfo] = useState<ConditionsResponse | null>(null);
  const [revision, setRevision] = useState(0);
  const lastAutoAt = useRef(0);
  const request = useRef<AbortController | null>(null);
  const generation = useRef(0);
  const confirmed = !!origin && !!dest;

  function updateValues(next: ConditionValues) {
    valuesRef.current = next;
    setValues(next);
  }

  function change(patch: Partial<ConditionValues>) {
    onEdit();
    const nextAuto = { ...autoRef.current };
    for (const key of ["period", "weather", "day_type"] as const)
      if (key in patch) nextAuto[key] = false;
    autoRef.current = nextAuto;
    setAuto(nextAuto);
    updateValues({ ...valuesRef.current, ...patch });
  }

  function resetAuto(key: keyof AutoFields) {
    onEdit();
    autoRef.current = { ...autoRef.current, [key]: true };
    setAuto(autoRef.current);
    setRevision((count) => count + 1);
  }

  function changeDeparture(mode: Departure, value: string) {
    onEdit();
    setDeparture(mode);
    setDateTime(value);
    autoRef.current = { period: true, day_type: true, weather: true };
    setAuto(autoRef.current);
    setRevision((count) => count + 1);
  }

  async function refresh(
    signal: AbortSignal,
    currentScenario: Scenario,
  ): Promise<ConditionValues> {
    const current = ++generation.current;
    setBusy(true);
    try {
      let data: ConditionsResponse | null = null;
      let message = "";
      try {
        const datetime = toTokyoIso(
          departure === "now" ? toTokyoInput(new Date()) : dateTime,
        );
        data = await getConditions(
          datetime,
          confirmed ? origin : undefined,
          confirmed ? dest : undefined,
          signal,
          currentScenario === "conditions-error" ? currentScenario : "normal",
        );
        if (data.weather_status !== "ok")
          message = weatherReasons[data.weather_status];
      } catch (failure) {
        if (signal.aborted) throw failure;
        message = `${failure instanceof Error ? failure.message : "条件を取得できませんでした。"} 自動項目は指定なしに戻しました。手動でも設定できます。`;
      }
      if (signal.aborted || current !== generation.current)
        throw new DOMException("キャンセル", "AbortError");
      setWarning(message);
      setInfo(data);
      const next = { ...valuesRef.current };
      if (autoRef.current.period) next.period = data?.period ?? "all";
      if (autoRef.current.day_type) next.day_type = data?.day_type ?? "all";
      if (autoRef.current.weather) next.weather = data?.weather ?? "all";
      updateValues(next);
      lastAutoAt.current = Date.now();
      return next;
    } finally {
      if (current === generation.current) setBusy(false);
    }
  }

  /** frontend-spec 3: in "now" mode, conditions older than 15 minutes are fetched again first. */
  async function valuesForSearch(
    signal: AbortSignal,
    currentScenario: Scenario,
  ): Promise<ConditionValues> {
    if (departure === "now" && Date.now() - lastAutoAt.current >= 15 * 60_000) {
      request.current?.abort();
      return refresh(signal, currentScenario);
    }
    return valuesRef.current;
  }

  useEffect(() => {
    const abort = new AbortController();
    request.current?.abort();
    request.current = abort;
    void refresh(abort.signal, scenario).catch(() => {});
    return () => abort.abort();
    // Only changed locations/departure/reset trigger automatic requests, never manual field edits.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    origin?.lat,
    origin?.lon,
    dest?.lat,
    dest?.lon,
    departure,
    dateTime,
    revision,
    scenario === "conditions-error",
  ]);

  useEffect(() => () => request.current?.abort(), []);

  return {
    values,
    auto,
    departure,
    dateTime,
    busy,
    warning,
    info,
    change,
    resetAuto,
    changeDeparture,
    valuesForSearch,
  };
}
