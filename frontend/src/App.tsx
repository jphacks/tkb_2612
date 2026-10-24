import { useEffect, useRef, useState } from "react";
import {
  ArrowLeft,
  ArrowRight,
  Check,
  LocateFixed,
  Menu,
  MapPin,
  Navigation,
  RefreshCw,
  SlidersHorizontal,
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
  RouteResponse,
  Scenario,
} from "./lib/types";
import { toTokyoInput, toTokyoIso } from "./lib/format";
import LocationInput, { type LocationValue } from "./components/LocationInput";
import ConditionFields, { type AutoFields } from "./components/ConditionFields";
import { LogoIntro } from "./components/LogoIntro";
import { RouteMap } from "./components/RouteMap";

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

function geolocationMessage(failure: GeolocationPositionError): string {
  switch (failure.code) {
    case failure.PERMISSION_DENIED:
      return "位置情報の利用が許可されていません。住所を入力してください。";
    case failure.TIMEOUT:
      return "現在地の取得がタイムアウトしました。再試行するか、住所を入力してください。";
    default:
      return "現在地を取得できませんでした。再試行するか、住所を入力してください。";
  }
}

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
  const [conditionBusy, setConditionBusy] = useState(false);
  const [conditionWarning, setConditionWarning] = useState("");
  const [conditionInfo, setConditionInfo] = useState<ConditionsResponse | null>(
    null,
  );
  const [conditionRevision, setConditionRevision] = useState(0);
  const lastAutoAt = useRef(0);
  const [healthWarning, setHealthWarning] = useState("");
  const [dataBlocked, setDataBlocked] = useState(false);
  const [healthRevision, setHealthRevision] = useState(0);
  const healthDataset = useRef<string | null>(null);
  const [result, setResult] = useState<RouteResponse | null>(null);
  const [selectedRoute, setSelectedRoute] = useState<string | null>(null);
  const [selectedSection, setSelectedSection] = useState<string | null>(null);
  const [showAccidents, setShowAccidents] = useState(true);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState("");
  const [scenario, setScenario] = useState<Scenario>("normal");
  const [sidebarOpen, setSidebarOpen] = useState(true);
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
      .then((health) => {
        healthDataset.current = health.dataset_id;
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
      setConditionWarning(warning);
      setConditionInfo(data);
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
    if (page !== "results") setSidebarOpen(true);
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
      // frontend-spec 3: a different data generation re-checks /api/health once.
      if (
        healthDataset.current !== null &&
        response.dataset_id !== healthDataset.current
      ) {
        healthDataset.current = response.dataset_id;
        setHealthRevision((count) => count + 1);
      }
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
    if (!window.isSecureContext) {
      setLocationMessage(
        "HTTPSで開いたページでのみ現在地を利用できます。住所を入力してください。",
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
        const { latitude: lat, longitude: lon, accuracy } = position.coords;
        if (
          ![lat, lon, accuracy].every(Number.isFinite) ||
          accuracy <= 0
        ) {
          setLocating(false);
          setLocationMessage(
            "現在地を正しく取得できませんでした。住所を入力してください。",
          );
          return;
        }
        if (lat < 20 || lat > 46 || lon < 122 || lon > 154) {
          setLocating(false);
          setLocationMessage("日本国内の地点を指定してください。");
          return;
        }
        changePlace("origin", {
          text: "現在地",
          place: { label: "現在地", lat, lon },
        });
        setLocationMessage(
          `現在地を出発地に設定しました（精度 約${Math.round(accuracy)}m）。`,
        );
      },
      (failure) => {
        if (locationRequest.current !== token) return;
        setLocating(false);
        setLocationMessage(geolocationMessage(failure));
      },
      // frontend-spec 7.1
      { enableHighAccuracy: true, timeout: 10_000, maximumAge: 0 },
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
      busy={conditionBusy}
      warning={conditionWarning}
      info={conditionInfo}
      demo={isDemo}
      compact={page === "results"}
    />
  );

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
            disabled={searching}
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
          <span className={`mode-badge ${isDemo ? "" : "live"}`}>
            {isDemo ? "デモデータ" : "API接続"}
          </span>
        </header>
        {page !== "results" ? (
          <main className={`form-page ${page}`}>
            <h1 ref={heading} tabIndex={-1}>
              {page === "places" ? "地点を入力" : "条件を設定"}
            </h1>
            {page === "places" ? (
              <div className="place-form">
                {places}
                <button
                  className="primary next-button"
                  disabled={!confirmed}
                  onClick={() => setPage("conditions")}
                >
                  条件を選ぶ
                  <ArrowRight size={18} />
                </button>
              </div>
            ) : (
              <>
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
          </main>
        ) : (
          <main className={`result-page${sidebarOpen ? " sidebar-open" : ""}`}>
            <button
              className="sidebar-toggle"
              type="button"
              aria-label={
                sidebarOpen ? "条件サイドバーを閉じる" : "条件サイドバーを開く"
              }
              aria-expanded={sidebarOpen}
              aria-controls="search-conditions"
              onClick={() => setSidebarOpen((open) => !open)}
            >
              <Menu size={22} aria-hidden="true" />
            </button>
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
                onShowAccidents={setShowAccidents}
                unavailable={scenario === "map-error"}
              />
              {searching ? (
                <div className="map-search-status result-empty" role="status">
                  <Navigation className="spin" size={28} />
                  <h2>ルートを検索しています</h2>
                  <p>応答があるまでお待ちください。</p>
                </div>
              ) : error ? (
                <div className="map-search-status result-empty" role="alert">
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
              ) : !result ? (
                <div className="map-search-status result-empty" role="status">
                  <MapPin size={28} />
                  <h2>条件を変更しました</h2>
                  <p>変更した条件で再検索してください。</p>
                  <button
                    className="primary"
                    disabled={!confirmed || conditionBusy || dataBlocked}
                    onClick={() => void runSearch()}
                  >
                    変更した条件で再検索
                  </button>
                </div>
              ) : null}
            </div>
            <aside
              id="search-conditions"
              className="route-panel"
              aria-label="検索条件"
              hidden={!sidebarOpen}
            >
              <div className="panel-title">
                <h1 ref={heading} tabIndex={-1}>
                  検索条件
                </h1>
              </div>
              {/* frontend-spec 3: inputs stay locked while searching; there is no cancel. */}
              <fieldset className="panel-editor" disabled={searching}>
                {places}
                {fields}
                <button
                  className="primary full"
                  disabled={
                    !confirmed || searching || conditionBusy || dataBlocked
                  }
                  onClick={() => void runSearch()}
                >
                  <RefreshCw size={16} />
                  再検索
                </button>
                <button
                  className="text-button panel-back"
                  onClick={() => setPage("conditions")}
                >
                  <ArrowLeft size={15} />
                  条件選択に戻る
                </button>
              </fieldset>
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
