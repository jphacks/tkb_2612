import { useEffect, useRef, useState } from "react";
import { X } from "lucide-react";
import type { LatLon } from "../lib/types";
import { INITIAL_CENTER } from "../community/coordinates";
import {
  readReports,
  saveReports,
  type SafetyReport,
} from "../community/reports";
import LocationInput, { type LocationValue } from "./LocationInput";
import { CommunityMap } from "./CommunityMap";
import { SafetyIcon, SAFETY_LABELS, type SafetyKind } from "./icons/SafetyIcon";
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
  const positionGeneration = useRef(0);
  const detailInput = useRef<HTMLInputElement>(null);

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
    if (kind) detailInput.current?.focus();
  }, [kind]);

  function close() {
    setPoint(null);
    setViewing(null);
    setKind(null);
    setText("");
    setError("");
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
        popupHeight={viewing || kind ? 164 : 64}
      >
        {viewing ? (
          <section className="community-review" aria-label="口コミ">
            <SafetyIcon kind={viewing.kind} size={28} />
            <button
              type="button"
              className="icon-button community-close"
              aria-label="口コミを閉じる"
              onClick={close}
            >
              <X size={18} />
            </button>
            {viewing.text && <p>{viewing.text}</p>}
          </section>
        ) : (
          point && (
            <form
              className={`community-composer${kind ? " is-editing" : ""}`}
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
                  <input
                    ref={detailInput}
                    aria-label="口コミ"
                    value={text}
                    placeholder="例：夜道は危ない"
                    maxLength={2000}
                    onKeyDown={(event) => {
                      if (
                        event.key === "Enter" &&
                        (event.nativeEvent.isComposing || event.keyCode === 229)
                      )
                        event.preventDefault();
                    }}
                    onChange={(event) => setText(event.target.value)}
                  />
                  <button className="primary" type="submit">
                    投稿
                  </button>
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
    </main>
  );
}
