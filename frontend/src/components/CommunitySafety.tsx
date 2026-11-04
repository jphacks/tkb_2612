import { useEffect, useId, useRef, useState } from "react";
import { Info, X } from "lucide-react";
import type { LatLon } from "../lib/types";
import { INITIAL_CENTER } from "../community/coordinates";
import {
  readReports,
  saveReports,
  type SafetyReport,
} from "../community/reports";
import LocationInput, { type LocationValue } from "./LocationInput";
import { CommunityMap } from "./CommunityMap";
import { CommunityReview } from "./CommunityReview";
import {
  SafetyIcon,
  SAFETY_COLORS,
  SAFETY_LABELS,
  type SafetyKind,
} from "./icons/SafetyIcon";
import "./community.css";

export function CommunitySafety() {
  const [center, setCenter] = useState<LatLon>(INITIAL_CENTER);
  const [currentPosition, setCurrentPosition] = useState<LatLon | null>(null);
  const [location, setLocation] = useState<LocationValue>({
    text: "",
    place: null,
  });
  const [reports, setReports] = useState(readReports);
  const [point, setPoint] = useState<LatLon | null>(null);
  const [viewing, setViewing] = useState<SafetyReport | null>(null);
  const [kind, setKind] = useState<SafetyKind | null>(null);
  const [text, setText] = useState("");
  const [error, setError] = useState("");
  const [addingSupport, setAddingSupport] = useState(false);
  const [supportText, setSupportText] = useState("");
  const positionGeneration = useRef(0);
  const detailInput = useRef<HTMLInputElement>(null);
  const detailId = useId();

  function locate() {
    const generation = ++positionGeneration.current;
    navigator.geolocation?.getCurrentPosition(
      (position) => {
        if (generation !== positionGeneration.current) return;
        const next = {
          lat: position.coords.latitude,
          lon: position.coords.longitude,
        };
        if (
          !Number.isFinite(next.lat) ||
          !Number.isFinite(next.lon) ||
          Math.abs(next.lat) > 90 ||
          Math.abs(next.lon) > 180
        )
          return;
        setCurrentPosition(next);
        setCenter(next);
        setLocation({ text: "現在地", place: { ...next, label: "現在地" } });
        close();
      },
      () => {},
      { enableHighAccuracy: true, timeout: 10000, maximumAge: 60000 },
    );
  }
  useEffect(() => {
    locate();
    return () => {
      ++positionGeneration.current;
    };
  }, []);
  useEffect(() => {
    if (kind || addingSupport) detailInput.current?.focus();
  }, [kind, addingSupport]);

  function close() {
    setPoint(null);
    setViewing(null);
    setKind(null);
    setText("");
    setError("");
    setAddingSupport(false);
    setSupportText("");
  }
  function pick(next: LatLon) {
    ++positionGeneration.current;
    close();
    setPoint(next);
  }
  function post() {
    if (!point || !kind) return;
    const report: SafetyReport = {
      ...point,
      id: crypto.randomUUID(),
      kind,
      text: text.trim(),
      createdAt: new Date().toISOString(),
      supportCount: 1,
      comments: [],
    };
    const next = [...reports, report];
    try {
      saveReports(next);
    } catch {
      setError("口コミを保存できませんでした。");
      return;
    }
    setReports(next);
    setViewing(report);
    setPoint(null);
    setKind(null);
    setText("");
    setError("");
  }

  function updateReport(updated: SafetyReport) {
    const next = reports.map((report) =>
      report.id === updated.id ? updated : report,
    );
    try {
      saveReports(next);
    } catch {
      setError("口コミを保存できませんでした。");
      return false;
    }
    setReports(next);
    setViewing(updated);
    setError("");
    return true;
  }

  function support() {
    if (!viewing || addingSupport) return;
    if (updateReport({ ...viewing, supportCount: viewing.supportCount + 1 })) {
      setSupportText("");
      setAddingSupport(true);
    }
  }

  function confirmSupport() {
    if (!viewing || !supportText.trim()) return;
    const updated: SafetyReport = {
      ...viewing,
      comments: [
        ...viewing.comments,
        {
          id: crypto.randomUUID(),
          text: supportText.trim(),
          createdAt: new Date().toISOString(),
        },
      ],
    };
    if (updateReport(updated)) {
      setAddingSupport(false);
      setSupportText("");
    }
  }

  return (
    <main className="community-page" aria-label="みんなの安全情報モード">
      <CommunityMap
        center={center}
        currentPosition={currentPosition}
        reports={reports}
        selected={viewing ?? point}
        onPick={pick}
        onReport={(report) => {
          ++positionGeneration.current;
          close();
          setViewing(report);
        }}
        onLocate={locate}
        popupWidth={viewing || kind ? 320 : 120}
        popupHeight={viewing || kind ? 200 : 64}
        bottomInset={100}
      >
        {viewing ? (
          addingSupport ? (
            <form
              className="community-composer community-support-form"
              aria-label="追加の詳細情報"
              style={{ borderColor: SAFETY_COLORS[viewing.kind] }}
              onSubmit={(event) => {
                event.preventDefault();
                confirmSupport();
              }}
            >
              <SafetyIcon kind={viewing.kind} count={viewing.supportCount} />
              <div className="community-detail">
                <label htmlFor={detailId}>詳細情報を入力</label>
                <input
                  id={detailId}
                  ref={detailInput}
                  value={supportText}
                  placeholder="例：夜道は危ない"
                  maxLength={2000}
                  onChange={(event) => setSupportText(event.target.value)}
                  onKeyDown={(event) => {
                    if (
                      event.key === "Enter" &&
                      (event.nativeEvent.isComposing || event.keyCode === 229)
                    )
                      event.preventDefault();
                  }}
                />
                <div className="community-support-actions">
                  <button
                    type="button"
                    className="secondary"
                    onClick={() => {
                      setAddingSupport(false);
                      setSupportText("");
                      setError("");
                    }}
                  >
                    スキップ
                  </button>
                  <button
                    type="submit"
                    className="primary"
                    disabled={!supportText.trim()}
                  >
                    確定
                  </button>
                </div>
              </div>
              {error && <p role="alert">{error}</p>}
            </form>
          ) : (
            <CommunityReview
              report={viewing}
              onClose={close}
              onSupport={support}
              error={error}
            />
          )
        ) : (
          point && (
            <form
              className={`community-composer${kind ? " is-editing" : ""}`}
              style={kind ? { borderColor: SAFETY_COLORS[kind] } : undefined}
              aria-label="安全情報を投稿"
              onKeyDown={(event) => {
                if (event.key === "Escape") {
                  event.preventDefault();
                  close();
                }
              }}
              onSubmit={(event) => {
                event.preventDefault();
                post();
              }}
            >
              <div
                className="community-votes"
                role="group"
                aria-label="安全・危険"
              >
                {(["safe", "danger"] as const).map((value) => (
                  <button
                    key={value}
                    type="button"
                    aria-label={SAFETY_LABELS[value]}
                    aria-pressed={kind === value}
                    className={kind === value ? "is-selected" : ""}
                    style={{ color: SAFETY_COLORS[value] }}
                    onClick={() => setKind(value)}
                  >
                    <SafetyIcon kind={value} />
                  </button>
                ))}
                {kind && (
                  <button
                    type="button"
                    className="icon-button community-close"
                    aria-label="投稿を閉じる"
                    onClick={close}
                  >
                    <X size={18} />
                  </button>
                )}
              </div>
              {kind && (
                <div className="community-detail">
                  <label htmlFor={detailId}>詳細情報を記入</label>
                  <div className="community-detail-input">
                    <input
                      id={detailId}
                      ref={detailInput}
                      value={text}
                      placeholder="例：夜道は危ない"
                      maxLength={2000}
                      onKeyDown={(event) => {
                        if (
                          event.key === "Enter" &&
                          (event.nativeEvent.isComposing ||
                            event.keyCode === 229)
                        )
                          event.preventDefault();
                      }}
                      onChange={(event) => setText(event.target.value)}
                    />
                    <button className="primary" type="submit">
                      投稿
                    </button>
                  </div>
                </div>
              )}
              {error && <p role="alert">{error}</p>}
            </form>
          )
        )}
      </CommunityMap>
      <div className="community-search">
        <LocationInput
          label="表示する場所"
          value={location}
          variant="map"
          scenario="normal"
          onChange={(next) => {
            ++positionGeneration.current;
            setLocation(next);
            if (next.place) {
              setCenter({ lat: next.place.lat, lon: next.place.lon });
              close();
            }
          }}
        />
      </div>
      <aside className="community-guide" aria-label="安全情報の共有方法">
        <Info size={22} aria-hidden="true" />
        <p>マップ上の地点を押して、地域の安全・危険情報を共有してください。</p>
      </aside>
    </main>
  );
}
