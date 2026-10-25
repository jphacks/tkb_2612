import { useEffect, useRef, useState } from "react";
import { getConditions } from "../api/client";
import { describeError } from "../api/errorMessage";
import type { AutoFields } from "../components/ConditionFields";
import { toTokyoInput, toTokyoIso } from "../lib/format";
import type {
  ConditionValues,
  ConditionsResponse,
  LatLon,
  Scenario,
} from "../lib/types";

export type Departure = "now" | "custom";

function effectKey(
  origin: LatLon | null,
  dest: LatLon | null,
  departure: Departure,
  dateTime: string,
  revision: number,
  scenario: Scenario,
): string {
  return JSON.stringify([
    origin?.lat,
    origin?.lon,
    dest?.lat,
    dest?.lon,
    departure,
    dateTime,
    revision,
    scenario === "conditions-error",
  ]);
}

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
  // 再検索で取得済みの条件を、同じ入力変更で重ねて取り直さないための印。
  const adopted = useRef<string | null>(null);

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
    /** 7.6: 再検索の新originと、現在時刻での取得。 */
    override?: { origin: LatLon; dest: LatLon; datetime: string },
  ): Promise<ConditionValues> {
    const current = ++generation.current;
    setBusy(true);
    try {
      let data: ConditionsResponse | null = null;
      let message = "";
      try {
        const datetime = toTokyoIso(
          override
            ? override.datetime
            : departure === "now"
              ? toTokyoInput(new Date())
              : dateTime,
        );
        data = await getConditions(
          datetime,
          override ? override.origin : confirmed ? origin : undefined,
          override ? override.dest : confirmed ? dest : undefined,
          signal,
          currentScenario === "conditions-error" ? currentScenario : "normal",
        );
        if (data.weather_status !== "ok")
          message = weatherReasons[data.weather_status];
      } catch (failure) {
        if (signal.aborted) throw failure;
        message = `${describeError(failure, "条件を取得できませんでした。")} 自動項目は指定なしに戻しました。手動でも設定できます。`;
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

  /**
   * 7.6 現在地からの再検索: 新originで条件を15分未満でも必ず1回取り直す。
   * 出発日時は「現在」にし、customだった場合だけ自動項目をautoへ戻す（nowのmanualは保持）。
   * 呼出し側は同じ同期区間でoriginを新しい値へ更新する。その変更による自動取得は行わない。
   */
  function refreshForReSearch(
    newOrigin: LatLon,
    newDest: LatLon,
    signal: AbortSignal,
    currentScenario: Scenario,
  ): Promise<ConditionValues> {
    request.current?.abort();
    if (departure === "custom") {
      autoRef.current = { period: true, day_type: true, weather: true };
      setAuto(autoRef.current);
    }
    const now = toTokyoInput(new Date());
    setDeparture("now");
    setDateTime(now);
    adopted.current = effectKey(newOrigin, dest, "now", now, revision, scenario);
    return refresh(signal, currentScenario, {
      origin: newOrigin,
      dest: newDest,
      datetime: now,
    });
  }

  useEffect(() => {
    const key = effectKey(origin, dest, departure, dateTime, revision, scenario);
    if (adopted.current === key) {
      adopted.current = null;
      return;
    }
    adopted.current = null;
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
    refreshForReSearch,
  };
}
