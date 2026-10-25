import { useEffect, useMemo, useRef, useState } from "react";
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
import { isDemo, searchRoutes } from "./api/client";
import { describeError, ROUTE_INPUT_HINT } from "./api/errorMessage";
import type { Scenario } from "./lib/types";
import { toTokyoIso } from "./lib/format";
import LocationInput, {
  type GeocodeAttribution,
  type LocationValue,
} from "./components/LocationInput";
import ConditionFields from "./components/ConditionFields";
import { DataAttribution } from "./components/DataAttribution";
import { LogoIntro } from "./components/LogoIntro";
import { NavigationPanel, SimulationBanner } from "./components/NavigationPanel";
import { NavigationStart } from "./components/NavigationStart";
import { ResultsPanel } from "./components/ResultsPanel";
import { RouteMap } from "./components/RouteMap";
import { buildNavigationLayers } from "./map/navigationData";
import { useConditions } from "./state/useConditions";
import { useCurrentLocation } from "./state/useCurrentLocation";
import { useHealth } from "./state/useHealth";
import { useNavigation } from "./state/useNavigation";
import { useRouteSearch } from "./state/useRouteSearch";
import type { RouteResponse } from "./lib/types";

type Page = "places" | "conditions" | "results";
const noop = () => {};
const blank: LocationValue = { text: "", place: null };
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

export default function App() {
  const [intro, setIntro] = useState(true);
  const [page, setPage] = useState<Page>("places");
  const [origin, setOrigin] = useState<LocationValue>(blank);
  const [dest, setDest] = useState<LocationValue>(blank);
  const [geocodeAttribution, setGeocodeAttribution] =
    useState<GeocodeAttribution>([]);
  const [showAccidents, setShowAccidents] = useState(true);
  const [scenario, setScenario] = useState<Scenario>("normal");
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const heading = useRef<HTMLHeadingElement>(null);
  const demoMenu = useRef<HTMLDetailsElement>(null);
  const confirmed = !!origin.place && !!dest.place;

  const health = useHealth();
  const route = useRouteSearch();
  const { invalidate, searching, result } = route;
  const conditions = useConditions({
    origin: origin.place,
    dest: dest.place,
    scenario,
    onEdit: invalidate,
  });
  const [mapUnavailable, setMapUnavailable] = useState(false);
  const navigation = useNavigation({ result, mapAvailable: !mapUnavailable });
  const navigating = navigation.snapshot !== null;
  const [follow, setFollow] = useState(true);
  useEffect(() => {
    // 追従は開始のたびに初期ONへ戻す。
    if (!navigating) setFollow(true);
  }, [navigating]);
  const navigationLayers = useMemo(
    () => buildNavigationLayers(navigation.route, navigation.snapshot),
    [navigation.route, navigation.snapshot],
  );
  const navigationRouteId = navigation.route?.id;
  const navigationSelection = navigation.snapshot?.selection;
  // ナビ中の地図は選んだ1本だけを描く（経路の切替は終了後だけ）。
  const navigationResult = useMemo<RouteResponse | null>(() => {
    if (!result || !navigation.route || !navigationSelection) return null;
    return {
      ...result,
      candidates: [navigation.route],
      fastest_id: navigation.route.id,
      safest_id: navigationSelection === "safest" ? navigation.route.id : null,
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [result, navigationRouteId, navigationSelection]);
  const location = useCurrentLocation((place) =>
    changePlace("origin", { text: place.label, place }),
  );
  const error = route.error
    ? describeError(
        route.error,
        "ルートを検索できませんでした。",
        ROUTE_INPUT_HINT,
      )
    : "";

  useEffect(() => {
    if (!intro) heading.current?.focus();
  }, [page, intro]);

  function changePlace(which: "origin" | "dest", next: LocationValue) {
    invalidate();
    if (which === "origin") {
      location.cancel();
      setOrigin(next);
    } else setDest(next);
    location.setMessage("");
  }

  function selectRoute(id: string) {
    route.setSelectedRoute(id);
    route.setSelectedSection(null);
  }

  async function runSearch(
    currentScenario: Scenario = scenario,
    restart = false,
  ) {
    const from = origin.place;
    const to = dest.place;
    if (
      !from ||
      !to ||
      health.blocked ||
      conditions.busy ||
      (searching && !restart)
    )
      return;
    const { departure, dateTime } = conditions;
    if (departure === "custom") {
      try {
        toTokyoIso(dateTime);
      } catch (failure) {
        route.setError(failure);
        return;
      }
    }
    // Inputs are locked while searching, so a pending GPS request must not
    // overwrite the origin and abort this search when it completes later.
    const wasLocating = location.locating;
    location.cancel();
    if (wasLocating)
      location.setMessage(
        "検索を開始したため、現在地の取得を中止しました。出発地は変更されません。",
      );
    if (page !== "results") setSidebarOpen(true);
    setPage("results");
    await route.run(
      async (signal) => {
        const applied = await conditions.valuesForSearch(
          signal,
          currentScenario,
        );
        const response = await searchRoutes(
          {
            origin: { lat: from.lat, lon: from.lon },
            dest: { lat: to.lat, lon: to.lon },
            ...applied,
          },
          signal,
          currentScenario,
        );
        return response;
      },
      (response) => health.noteRouteDataset(response.dataset_id),
    );
  }

  const currentLocationButton = (
    <button
      className="text-button"
      type="button"
      disabled={location.locating}
      onClick={location.request}
    >
      <LocateFixed size={15} />
      {location.locating
        ? "取得中…"
        : isDemo
          ? "現在地を使う（デモ）"
          : "現在地を使う"}
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
        onAttribution={setGeocodeAttribution}
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
        onAttribution={setGeocodeAttribution}
      />
      {location.message && (
        <p role="status" className="field-help">
          {location.message}
        </p>
      )}
    </>
  );
  const fields = (
    <ConditionFields
      values={conditions.values}
      onChange={conditions.change}
      auto={conditions.auto}
      onReset={conditions.resetAuto}
      departure={conditions.departure}
      dateTime={conditions.dateTime}
      onDeparture={conditions.changeDeparture}
      busy={conditions.busy}
      warning={conditions.warning}
      info={conditions.info}
      demo={isDemo}
      compact={page === "results"}
    />
  );

  const healthNotice = health.warning && (
    <div className="notice warning">
      <p>{health.warning}</p>
      <button className="text-button" onClick={health.recheck}>
        準備状況を再確認
      </button>
    </div>
  );

  const attribution = (
    <DataAttribution
      dataYears={result?.data_years ?? health.info?.data_years ?? null}
      geocode={geocodeAttribution}
      demo={isDemo}
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
            disabled={searching || navigating}
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
                      !confirmed ||
                      searching ||
                      conditions.busy ||
                      health.blocked
                    }
                    onClick={() => void runSearch()}
                  >
                    ルートを比較する
                    <ArrowRight size={17} />
                  </button>
                </div>
              </>
            )}
            {healthNotice}
            {attribution}
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
                result={navigationResult ?? result}
                selectedRouteId={navigationResult ? navigation.route!.id : route.selectedRoute}
                selectedSectionId={route.selectedSection}
                onSelectRoute={navigating ? noop : selectRoute}
                onSelectSection={route.setSelectedSection}
                showAccidents={showAccidents}
                onShowAccidents={setShowAccidents}
                unavailable={scenario === "map-error"}
                onUnavailableChange={setMapUnavailable}
                navigation={
                  navigating
                    ? { layers: navigationLayers, follow, onFollowChange: setFollow }
                    : undefined
                }
              />
              {/* サイドバーを閉じても、模擬走行であることは地図の上に常時出す。 */}
              {navigation.snapshot?.simulation && !sidebarOpen && (
                <SimulationBanner snapshot={navigation.snapshot} />
              )}
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
                    disabled={!confirmed || conditions.busy || health.blocked}
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
              aria-label={navigating ? "ナビ" : "検索結果と条件"}
              hidden={!sidebarOpen}
            >
              <div className="panel-title">
                <h1 ref={heading} tabIndex={-1}>
                  {navigating ? "ナビ" : "検索結果と条件"}
                </h1>
              </div>
              {navigation.snapshot ? (
                <NavigationPanel
                  snapshot={navigation.snapshot}
                  onStop={navigation.stop}
                  onPause={navigation.pause}
                  onResume={navigation.resume}
                  onMultiplier={navigation.setMultiplier}
                  onJitter={navigation.setJitter}
                  onTryDeviation={navigation.tryDeviation}
                />
              ) : (
                <>
                  {result && route.selectedRoute && (
                    <>
                      <NavigationStart
                        result={result}
                        mapAvailable={!mapUnavailable}
                        error={navigation.startError}
                        onStart={(request) =>
                          navigation.start({ ...request, forResult: result })
                        }
                      />
                      <ResultsPanel
                        result={result}
                        selectedRouteId={route.selectedRoute}
                        onSelectRoute={selectRoute}
                      />
                    </>
                  )}
                  <h2 className="panel-subtitle">検索条件</h2>
                  {/* frontend-spec 3: inputs stay locked while searching; there is no cancel. */}
                  <fieldset className="panel-editor" disabled={searching}>
                    {healthNotice}
                    {places}
                    {fields}
                    <button
                      className="primary full"
                      disabled={
                        !confirmed ||
                        searching ||
                        conditions.busy ||
                        health.blocked
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
                </>
              )}
            </aside>
            {/* Outside the collapsible sidebar: attribution stays visible with the map. */}
            {attribution}
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
                  disabled={navigating}
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
