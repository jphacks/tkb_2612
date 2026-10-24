import { useEffect, useRef, useState } from "react";
import {
  ArrowLeft,
  ArrowRight,
  Check,
  ChevronDown,
  LocateFixed,
  MapPin,
  Navigation,
  RefreshCw,
  Settings2,
  SlidersHorizontal,
  X,
} from "lucide-react";
import {
  ApiError,
  getConditions,
  getHealth,
  isDemo,
  searchRoutes,
} from "./api/client";
import type {
  ConditionValues,
  ConditionsResponse,
  HealthResponse,
  RouteResponse,
  Scenario,
} from "./lib/types";
import { toTokyoInput, toTokyoIso } from "./lib/format";
import LocationInput, { type LocationValue } from "./components/LocationInput";
import ConditionFields, { type AutoFields } from "./components/ConditionFields";
import { LogoIntro } from "./components/LogoIntro";
import { RouteMap } from "./components/RouteMap";
import ResultsPanel from "./components/ResultsPanel";

type Page = "places" | "conditions" | "results";
const blank: LocationValue = { text: "", place: null };
const initialValues: ConditionValues = {
  mode: "walk",
  period: "all",
  day_type: "all",
  weather: "all",
  max_detour_min: 10,
};
const scenarios: { id: Scenario; label: string }[] = [
  { id: "normal", label: "通常" },
  { id: "empty", label: "住所候補0件" },
  { id: "loading", label: "検索中" },
  { id: "error", label: "エラー" },
  { id: "same", label: "同一経路" },
  { id: "zero", label: "事故0件" },
  { id: "conditions-error", label: "条件取得不可" },
  { id: "map-error", label: "地図利用不可" },
];
const modeNames = { walk: "徒歩", bike: "自転車", car: "自動車" };

export default function App() {
  const [intro, setIntro] = useState(true);
  const [page, setPage] = useState<Page>("places");
  const [origin, setOrigin] = useState<LocationValue>(blank);
  const [dest, setDest] = useState<LocationValue>(blank);
  const [values, setValues] = useState<ConditionValues>(initialValues);
  const valuesRef = useRef(values);
  const [auto, setAuto] = useState<AutoFields>({
    period: true,
    day_type: true,
    weather: true,
  });
  const autoRef = useRef(auto);
  const [departure, setDeparture] = useState<"now" | "custom">("now");
  const [dateTime, setDateTime] = useState(() => toTokyoInput(new Date()));
  const [conditions, setConditions] = useState<ConditionsResponse | null>(null);
  const [conditionBusy, setConditionBusy] = useState(false);
  const [conditionWarning, setConditionWarning] = useState("");
  const [conditionRevision, setConditionRevision] = useState(0);
  const lastAutoAt = useRef(0);
  const [health, setHealth] = useState<HealthResponse | null>(null);
  const [healthWarning, setHealthWarning] = useState("");
  const [dataBlocked, setDataBlocked] = useState(false);
  const [healthRevision, setHealthRevision] = useState(0);
  const [result, setResult] = useState<RouteResponse | null>(null);
  const [selectedRoute, setSelectedRoute] = useState<string | null>(null);
  const [selectedSection, setSelectedSection] = useState<string | null>(null);
  const [showAccidents, setShowAccidents] = useState(true);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState("");
  const [scenario, setScenario] = useState<Scenario>("normal");
  const [editorOpen, setEditorOpen] = useState(false);
  const [locating, setLocating] = useState(false);
  const [locationMessage, setLocationMessage] = useState("");
  const routeRequest = useRef<AbortController | null>(null);
  const conditionRequest = useRef<AbortController | null>(null);
  const conditionGeneration = useRef(0);
  const locationRequest = useRef(0);
  const requestVersion = useRef(0);
  const heading = useRef<HTMLHeadingElement>(null);
  const demoMenu = useRef<HTMLDetailsElement>(null);
  const confirmed = !!origin.place && !!dest.place;

  useEffect(() => {
    if (!intro) heading.current?.focus();
  }, [page, intro]);

  useEffect(() => {
    const abort = new AbortController();
    setHealthWarning("");
    getHealth(abort.signal)
      .then((data) => {
        setHealth(data);
        setDataBlocked(false);
      })
      .catch((failure) => {
        if (abort.signal.aborted) return;
        setDataBlocked(
          failure instanceof ApiError && failure.code === "data_unavailable",
        );
        setHealthWarning(
          failure instanceof Error
            ? failure.message
            : "データの準備状況を確認できませんでした。",
        );
      });
    return () => abort.abort();
  }, [healthRevision]);

  function invalidate() {
    ++requestVersion.current;
    routeRequest.current?.abort();
    setSearching(false);
    setResult(null);
    setSelectedRoute(null);
    setSelectedSection(null);
    setError("");
  }

  function updateValues(next: ConditionValues) {
    valuesRef.current = next;
    setValues(next);
  }

  function changeConditions(patch: Partial<ConditionValues>) {
    invalidate();
    const nextAuto = { ...autoRef.current };
    for (const key of ["period", "weather", "day_type"] as const)
      if (key in patch) nextAuto[key] = false;
    autoRef.current = nextAuto;
    setAuto(nextAuto);
    updateValues({ ...valuesRef.current, ...patch });
  }

  function changePlace(which: "origin" | "dest", next: LocationValue) {
    invalidate();
    if (which === "origin") {
      ++locationRequest.current;
      setLocating(false);
      setOrigin(next);
    } else setDest(next);
    setLocationMessage("");
  }

  function resetAuto(key: keyof AutoFields) {
    invalidate();
    autoRef.current = { ...autoRef.current, [key]: true };
    setAuto(autoRef.current);
    setConditionRevision((count) => count + 1);
  }

  function changeDeparture(mode: "now" | "custom", value: string) {
    invalidate();
    setDeparture(mode);
    setDateTime(value);
    autoRef.current = { period: true, day_type: true, weather: true };
    setAuto(autoRef.current);
    setConditionRevision((count) => count + 1);
  }

  async function refreshConditions(
    signal: AbortSignal,
    currentScenario: Scenario,
  ): Promise<ConditionValues> {
    const generation = ++conditionGeneration.current;
    setConditionBusy(true);
    try {
      let data: ConditionsResponse | null = null;
      let warning = "";
      try {
        const datetime = toTokyoIso(
          departure === "now" ? toTokyoInput(new Date()) : dateTime,
        );
        data = await getConditions(
          datetime,
          confirmed ? origin.place! : undefined,
          confirmed ? dest.place! : undefined,
          signal,
          currentScenario === "conditions-error" ? currentScenario : "normal",
        );
        if (data.weather_status !== "ok") {
          const reasons = {
            no_location: "両地点を確定すると天気を取得できます。",
            past: "過去の日時の天気は自動設定できません。",
            out_of_range: "指定日時は天気予報の対象期間外です。",
            unavailable: "天気の取得元を利用できません。",
          };
          warning = reasons[data.weather_status];
        }
      } catch (failure) {
        if (signal.aborted) throw failure;
        warning = `${failure instanceof Error ? failure.message : "条件を取得できませんでした。"} 自動項目は指定なしに戻しました。手動でも設定できます。`;
      }
      if (signal.aborted || generation !== conditionGeneration.current)
        throw new DOMException("キャンセル", "AbortError");
      setConditions(data);
      setConditionWarning(warning);
      const next = { ...valuesRef.current };
      if (autoRef.current.period) next.period = data?.period ?? "all";
      if (autoRef.current.day_type) next.day_type = data?.day_type ?? "all";
      if (autoRef.current.weather) next.weather = data?.weather ?? "all";
      updateValues(next);
      lastAutoAt.current = Date.now();
      return next;
    } finally {
      if (generation === conditionGeneration.current) setConditionBusy(false);
    }
  }

  useEffect(() => {
    const abort = new AbortController();
    conditionRequest.current?.abort();
    conditionRequest.current = abort;
    void refreshConditions(abort.signal, scenario).catch(() => {});
    return () => abort.abort();
    // Only changed locations/departure/reset trigger automatic requests, never manual field edits.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    origin.place?.lat,
    origin.place?.lon,
    dest.place?.lat,
    dest.place?.lon,
    departure,
    dateTime,
    conditionRevision,
    scenario === "conditions-error",
  ]);

  useEffect(
    () => () => {
      routeRequest.current?.abort();
      conditionRequest.current?.abort();
      ++locationRequest.current;
    },
    [],
  );

  async function runSearch(
    currentScenario: Scenario = scenario,
    restart = false,
  ) {
    if (
      !origin.place ||
      !dest.place ||
      dataBlocked ||
      conditionBusy ||
      (searching && !restart)
    )
      return;
    if (departure === "custom") {
      try {
        toTokyoIso(dateTime);
      } catch (failure) {
        setError((failure as Error).message);
        return;
      }
    }
    routeRequest.current?.abort();
    const abort = new AbortController();
    routeRequest.current = abort;
    const version = ++requestVersion.current;
    setSearching(true);
    setResult(null);
    setSelectedSection(null);
    setError("");
    setEditorOpen(false);
    setPage("results");
    try {
      let applied = valuesRef.current;
      if (
        departure === "now" &&
        Date.now() - lastAutoAt.current >= 15 * 60_000
      ) {
        conditionRequest.current?.abort();
        applied = await refreshConditions(abort.signal, currentScenario);
      }
      const response = await searchRoutes(
        {
          origin: { lat: origin.place.lat, lon: origin.place.lon },
          dest: { lat: dest.place.lat, lon: dest.place.lon },
          ...applied,
        },
        abort.signal,
        currentScenario,
      );
      if (version !== requestVersion.current || abort.signal.aborted) return;
      setResult(response);
      setSelectedRoute(response.safest_id ?? response.fastest_id);
    } catch (failure) {
      if (abort.signal.aborted || version !== requestVersion.current) return;
      setError(
        failure instanceof Error
          ? failure.message
          : "ルートを検索できませんでした。",
      );
    } finally {
      if (version === requestVersion.current) setSearching(false);
    }
  }

  function useCurrentLocation() {
    if (isDemo) {
      changePlace("origin", {
        text: "東京駅（現在地デモ）",
        place: {
          label: "東京駅（現在地デモ）",
          lat: 35.681236,
          lon: 139.767125,
        },
      });
      setLocationMessage(
        "デモの現在地を東京駅に設定しました。位置情報は取得していません。",
      );
      return;
    }
    if (!navigator.geolocation) {
      setLocationMessage(
        "このブラウザーでは現在地を利用できません。住所を入力してください。",
      );
      return;
    }
    const token = ++locationRequest.current;
    setLocating(true);
    setLocationMessage("現在地を取得しています…");
    navigator.geolocation.getCurrentPosition(
      (position) => {
        if (locationRequest.current !== token) return;
        const { latitude: lat, longitude: lon } = position.coords;
        if (lat < 20 || lat > 46 || lon < 122 || lon > 154) {
          setLocating(false);
          setLocationMessage("日本国内の地点を指定してください。");
          return;
        }
        changePlace("origin", {
          text: "現在地",
          place: { label: "現在地", lat, lon },
        });
      },
      () => {
        if (locationRequest.current !== token) return;
        setLocating(false);
        setLocationMessage(
          "現在地を取得できませんでした。住所を入力するか、位置情報の設定を確認してください。",
        );
      },
      { enableHighAccuracy: false, timeout: 10_000, maximumAge: 60_000 },
    );
  }

  const currentLocationButton = (
    <button
      className="text-button"
      type="button"
      disabled={locating}
      onClick={useCurrentLocation}
    >
      <LocateFixed size={15} />
      {locating ? "取得中…" : isDemo ? "現在地を使う（デモ）" : "現在地を使う"}
    </button>
  );
  const places = (
    <>
      <LocationInput
        label="出発地"
        value={origin}
        onChange={(next) => changePlace("origin", next)}
        scenario={scenario}
        action={currentLocationButton}
      />
      <div className="location-connector" aria-hidden="true">
        <span />
        <span />
        <span />
      </div>
      <LocationInput
        label="目的地"
        value={dest}
        onChange={(next) => changePlace("dest", next)}
        scenario={scenario}
      />
      {locationMessage && (
        <p role="status" className="field-help">
          {locationMessage}
        </p>
      )}
    </>
  );
  const fields = (
    <ConditionFields
      values={values}
      onChange={changeConditions}
      auto={auto}
      onReset={resetAuto}
      departure={departure}
      dateTime={dateTime}
      onDeparture={changeDeparture}
      conditionData={conditions}
      busy={conditionBusy}
      warning={conditionWarning}
      compact={page === "results"}
    />
  );
  const summary = `${modeNames[values.mode]} · ${values.weather === "all" ? "全天候" : values.weather} · ${values.period === "all" ? "全時間帯" : values.period} · ${values.day_type === "all" ? "全曜日" : values.day_type} · 追加${values.max_detour_min}分まで`;

  return (
    <>
      <div
        className={`app ${page === "results" ? "app-results" : ""}`}
        inert={intro}
        aria-hidden={intro || undefined}
      >
        <header className="app-header">
          <button
            className="wordmark"
            type="button"
            onClick={() => {
              if (page !== "places") {
                invalidate();
                setPage("places");
              }
            }}
            aria-label="AcciLessNavi 地点入力へ"
          >
            AcciLess<span>Navi</span>
          </button>
          <nav aria-label="検索ステップ" className="steps">
            {(["places", "conditions", "results"] as const).map(
              (item, index) => (
                <button
                  key={item}
                  type="button"
                  aria-current={page === item ? "step" : undefined}
                  disabled={
                    item === "conditions"
                      ? !confirmed
                      : item === "results"
                        ? !result
                        : false
                  }
                  onClick={() => {
                    if (page !== item && searching) invalidate();
                    setPage(item);
                  }}
                >
                  <span>{index + 1}</span>
                  <b>{["地点", "条件", "比較"][index]}</b>
                </button>
              ),
            )}
          </nav>
          <span className={`mode-badge ${isDemo ? "" : "live"}`}>
            {isDemo ? "デモデータ" : "API接続"}
          </span>
        </header>
        {page !== "results" ? (
          <main className={`form-page ${page}`}>
            <div className="page-kicker">
              {page === "places" ? "01 / 地点を入力" : "02 / 条件を設定"}
            </div>
            <h1 ref={heading} tabIndex={-1}>
              {page === "places" ? "どこへ行きますか？" : "移動の条件を選ぶ"}
            </h1>
            <p className="page-description">
              {page === "places"
                ? "最短時間と、事故履歴を考慮した経路を比較します。"
                : "移動手段や天気を指定して、経路を比較できます。"}
            </p>
            {page === "places" ? (
              <div className="place-form">
                {places}
                <button
                  className="primary full next-button"
                  disabled={!confirmed}
                  onClick={() => setPage("conditions")}
                >
                  条件を選ぶ
                  <ArrowRight size={18} />
                </button>
              </div>
            ) : (
              <>
                <div className="trip-summary">
                  <MapPin size={18} />
                  <div>
                    <span>{origin.text}</span>
                    <ArrowRight size={14} />
                    <span>{dest.text}</span>
                  </div>
                  <button
                    type="button"
                    className="text-button"
                    onClick={() => setPage("places")}
                  >
                    変更
                  </button>
                </div>
                {isDemo && (
                  <p className="demo-note">
                    デモ条件：天気は固定値。祝日データは網羅していません。
                  </p>
                )}
                {fields}
                {error && (
                  <p className="notice warning" role="alert">
                    {error}
                  </p>
                )}
                <div className="form-actions">
                  <button
                    className="secondary"
                    onClick={() => setPage("places")}
                  >
                    <ArrowLeft size={16} />
                    戻る
                  </button>
                  <button
                    className="primary"
                    disabled={
                      !confirmed || searching || conditionBusy || dataBlocked
                    }
                    onClick={() => void runSearch()}
                  >
                    ルートを比較する
                    <ArrowRight size={17} />
                  </button>
                </div>
              </>
            )}
            {healthWarning && (
              <div className="notice warning">
                <p>{healthWarning}</p>
                <button
                  className="text-button"
                  onClick={() => setHealthRevision((count) => count + 1)}
                >
                  準備状況を再確認
                </button>
              </div>
            )}
            <p className="form-footnote">
              {isDemo
                ? "架空の経路・事故データを使用しています。"
                : health
                  ? `使用データ：${health.data_years.join("・")}年`
                  : "データの準備状況を確認しています。"}
              <br />
              過去の事故履歴に基づく比較であり、安全を保証するものではありません。
            </p>
          </main>
        ) : (
          <main className="result-page">
            <div className="map-stage">
              <RouteMap
                result={result}
                selectedRouteId={selectedRoute}
                selectedSectionId={selectedSection}
                onSelectRoute={(id) => {
                  setSelectedRoute(id);
                  setSelectedSection(null);
                }}
                onSelectSection={setSelectedSection}
                showAccidents={showAccidents}
                unavailable={scenario === "map-error"}
              />
            </div>
            <aside className="route-panel" aria-label="経路の比較と検索条件">
              <div className="panel-trip">
                <div className="panel-title">
                  <h1 ref={heading} tabIndex={-1}>
                    ルート比較
                  </h1>
                  <button
                    className="icon-button"
                    aria-label="条件を編集"
                    aria-expanded={editorOpen}
                    onClick={() => setEditorOpen((open) => !open)}
                  >
                    {editorOpen ? <X size={19} /> : <Settings2 size={19} />}
                  </button>
                </div>
                <div className="compact-locations">
                  <p>
                    <span className="point-symbol start" />
                    {origin.text || "出発地未設定"}
                  </p>
                  <p>
                    <span className="point-symbol end" />
                    {dest.text || "目的地未設定"}
                  </p>
                </div>
                <p className="compact-conditions">
                  {departure === "now"
                    ? "今すぐ出発"
                    : dateTime.replace("T", " ")}
                  （日本時間）
                  <br />
                  {summary}
                </p>
                <div className="panel-actions">
                  <button
                    className="secondary"
                    onClick={() => setEditorOpen((open) => !open)}
                  >
                    <SlidersHorizontal size={15} />
                    条件を変更
                    <ChevronDown size={14} />
                  </button>
                  <button
                    className="primary"
                    disabled={
                      !confirmed || searching || conditionBusy || dataBlocked
                    }
                    onClick={() => void runSearch()}
                  >
                    <RefreshCw size={15} />
                    再検索
                  </button>
                </div>
              </div>
              {editorOpen && (
                <div className="panel-editor">
                  {places}
                  {fields}
                  <button
                    className="primary full"
                    disabled={
                      !confirmed || searching || conditionBusy || dataBlocked
                    }
                    onClick={() => void runSearch()}
                  >
                    変更した条件で再検索
                    <ArrowRight size={16} />
                  </button>
                </div>
              )}
              {searching ? (
                <div className="result-empty" role="status">
                  <Navigation className="spin" size={28} />
                  <h2>ルートを検索しています</h2>
                  <p>条件に合う候補を比較しています。</p>
                  <button
                    className="secondary"
                    onClick={() => {
                      invalidate();
                      setConditionBusy(false);
                    }}
                  >
                    検索をキャンセル
                  </button>
                </div>
              ) : error ? (
                <div className="result-empty" role="alert">
                  <h2>検索できませんでした</h2>
                  <p>{error}</p>
                  <button
                    className="primary"
                    onClick={() => {
                      if (isDemo) setScenario("normal");
                      void runSearch(isDemo ? "normal" : scenario);
                    }}
                  >
                    再試行
                  </button>
                </div>
              ) : result && selectedRoute ? (
                <ResultsPanel
                  result={result}
                  selectedRouteId={selectedRoute}
                  onSelectRoute={(id) => {
                    setSelectedRoute(id);
                    setSelectedSection(null);
                  }}
                  selectedSectionId={selectedSection}
                  onSelectSection={setSelectedSection}
                  showAccidents={showAccidents}
                  onShowAccidents={setShowAccidents}
                  isDemo={isDemo}
                />
              ) : (
                <div className="result-empty" role="status">
                  <MapPin size={28} />
                  <h2>条件を変更しました</h2>
                  <p>変更した条件で再検索してください。</p>
                </div>
              )}
              <button
                className="text-button panel-back"
                onClick={() => {
                  if (searching) invalidate();
                  setPage("conditions");
                }}
              >
                <ArrowLeft size={15} />
                条件選択に戻る
              </button>
            </aside>
          </main>
        )}
        {isDemo && (
          <details className="demo-menu" ref={demoMenu}>
            <summary>
              <SlidersHorizontal size={15} />
              デモ状態切替
            </summary>
            <div className="demo-options">
              <p>表示・操作を確認できます</p>
              {scenarios.map((item) => (
                <button
                  key={item.id}
                  type="button"
                  aria-pressed={scenario === item.id}
                  onClick={() => {
                    setScenario(item.id);
                    if (demoMenu.current) demoMenu.current.open = false;
                    if (item.id === "empty") {
                      invalidate();
                      setPage("places");
                    } else if (
                      item.id === "conditions-error" ||
                      scenario === "conditions-error"
                    )
                      invalidate();
                    else if (page === "results" && confirmed) {
                      invalidate();
                      void runSearch(item.id, true);
                    }
                  }}
                >
                  <span>{item.label}</span>
                  {scenario === item.id && <Check size={15} />}
                </button>
              ))}
            </div>
          </details>
        )}
      </div>
      {intro && <LogoIntro onComplete={() => setIntro(false)} />}
    </>
  );
}
