import { useEffect, useRef, useState } from "react";
import type { LatLon } from "../lib/types";
import { INITIAL_CENTER } from "../community/coordinates";
import {
  readReports,
  saveReports,
  readSessionVotes,
  saveReportsWithVote,
  pointKey,
  type SafetyReport,
} from "../community/reports";
import LocationInput, { type LocationValue } from "./LocationInput";
import { CommunityMap } from "./CommunityMap";
import { CommunityReview } from "./CommunityReview";
import { CommunityDetailsForm } from "./CommunityDetailsForm";
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
  const [votedPoints, setVotedPoints] = useState(readSessionVotes);
  const [composingPost, setComposingPost] = useState(false);
  const [addingSupport, setAddingSupport] = useState(false);
  const [supportText, setSupportText] = useState("");
  const positionGeneration = useRef(0);

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

  function close() {
    setPoint(null);
    setViewing(null);
    setKind(null);
    setText("");
    setError("");
    setAddingSupport(false);
    setSupportText("");
    setComposingPost(false);
  }
  function pick(next: LatLon) {
    ++positionGeneration.current;
    close();
    const existing = reports.find(
      (report) => pointKey(report) === pointKey(next),
    );
    if (existing) setViewing(existing);
    else setPoint(next);
  }
  function post(details: string) {
    if (!point || !kind || readSessionVotes().has(pointKey(point))) return;
    const report: SafetyReport = {
      ...point,
      id: crypto.randomUUID(),
      kind,
      text: details.trim(),
      createdAt: new Date().toISOString(),
      supportCount: 1,
      comments: [],
    };
    const next = [...reports, report];
    try {
      setVotedPoints(saveReportsWithVote(next, report));
    } catch {
      setError("口コミを保存できませんでした。");
      return;
    }
    setReports(next);
    setViewing(report);
    setPoint(null);
    setKind(null);
    setText("");
    setComposingPost(false);
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
    if (!viewing || addingSupport || readSessionVotes().has(pointKey(viewing)))
      return;
    const updated = { ...viewing, supportCount: viewing.supportCount + 1 };
    const next = reports.map((report) =>
      report.id === updated.id ? updated : report,
    );
    try {
      setVotedPoints(saveReportsWithVote(next, updated));
    } catch {
      setError("口コミを保存できませんでした。");
      return;
    }
    setReports(next);
    setViewing(updated);
    setError("");
    setSupportText("");
    setAddingSupport(true);
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
    <main className="community-page" aria-label="安全危険情報共有モード">
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
        popupWidth={viewing || composingPost ? 320 : 146}
        popupHeight={viewing || composingPost ? 200 : kind ? 136 : 82}
        bottomInset={80}
      >
        {viewing ? (
          addingSupport ? (
            <CommunityDetailsForm
              kind={viewing.kind}
              count={viewing.supportCount}
              value={supportText}
              onChange={setSupportText}
              onSkip={() => {
                setAddingSupport(false);
                setSupportText("");
                setError("");
              }}
              onConfirm={confirmSupport}
              error={error}
            />
          ) : (
            <CommunityReview
              key={viewing.id}
              report={viewing}
              onClose={close}
              onSupport={support}
              supported={votedPoints.has(pointKey(viewing))}
              error={error}
            />
          )
        ) : (
          point &&
          (composingPost && kind ? (
            <CommunityDetailsForm
              kind={kind}
              value={text}
              onChange={setText}
              onSkip={() => post("")}
              onConfirm={() => post(text)}
              error={error}
            />
          ) : (
            <div
              className="community-composer community-choice-panel"
              role="group"
              aria-label="安全情報を投稿"
              onKeyDown={(event) => {
                if (event.key === "Escape") {
                  event.preventDefault();
                  close();
                }
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
                    className={`community-choice${kind === value ? " is-selected" : ""}`}
                    style={{ color: SAFETY_COLORS[value] }}
                    onClick={() => setKind(value)}
                  >
                    <SafetyIcon kind={value} />
                  </button>
                ))}
              </div>
              {kind && (
                <button
                  type="button"
                  className="primary community-publish"
                  onClick={() => setComposingPost(true)}
                >
                  投票
                </button>
              )}
            </div>
          ))
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
