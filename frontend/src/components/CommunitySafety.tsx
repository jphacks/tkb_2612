import { useEffect, useRef, useState } from "react";
import type { LatLon } from "../lib/types";
import { ApiError } from "../api/errors";
import { INITIAL_CENTER } from "../community/coordinates";
import {
  createSafetyReport,
  listSafetyReports,
  voteSafetyReport,
} from "../community/api";
import {
  isTooWide,
  pointKey,
  type SafetyBounds,
  type SafetyReport,
} from "../community/reports";
import { DEMO_REPORTS_KEY } from "../community/demoStore";
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

/** 地図の移動が落ち着いてから読み直すまでの待ち時間。 */
export const RELOAD_DELAY_MS = 300;
const SAVE_FAILED = "口コミを保存できませんでした。時間をおいて再試行してください。";
export const TOO_WIDE = "表示範囲が広すぎます。地図を拡大すると口コミを表示します。";

function failure(error: unknown, fallback: string) {
  return error instanceof ApiError ? error.message : fallback;
}

/** 一覧の中の同じ地点を差し替える（無ければ先頭に足す）。 */
function upsert(reports: SafetyReport[], report: SafetyReport) {
  return reports.some((item) => item.id === report.id)
    ? reports.map((item) => (item.id === report.id ? report : item))
    : [report, ...reports];
}

export function CommunitySafety() {
  const [center, setCenter] = useState<LatLon>(INITIAL_CENTER);
  const [currentPosition, setCurrentPosition] = useState<LatLon | null>(null);
  const [location, setLocation] = useState<LocationValue>({
    text: "",
    place: null,
  });
  const [reports, setReports] = useState<SafetyReport[]>([]);
  const [bounds, setBounds] = useState<SafetyBounds | null>(null);
  const [reloads, setReloads] = useState(0);
  const [loadError, setLoadError] = useState("");
  const [point, setPoint] = useState<LatLon | null>(null);
  const [viewing, setViewing] = useState<SafetyReport | null>(null);
  const [kind, setKind] = useState<SafetyKind | null>(null);
  const [text, setText] = useState("");
  const [error, setError] = useState("");
  const [composingPost, setComposingPost] = useState(false);
  const [voteKind, setVoteKind] = useState<SafetyKind | null>(null);
  const [voteText, setVoteText] = useState("");
  const [busy, setBusy] = useState(false);
  const positionGeneration = useRef(0);
  const viewingId = useRef<string | null>(null);
  viewingId.current = viewing?.id ?? null;

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

  // 表示範囲の口コミを読む。地図の移動中は待ち、新しい範囲で前の読込を取り消す。
  // 広すぎる範囲（地図を引いたとき）は読まずに拡大を促す（表示中の地点はそのまま）。
  useEffect(() => {
    if (!bounds) return;
    if (isTooWide(bounds)) {
      setLoadError(TOO_WIDE);
      return;
    }
    const controller = new AbortController();
    const timer = window.setTimeout(() => {
      listSafetyReports(bounds, controller.signal)
        .then((latest) => {
          if (controller.signal.aborted) return;
          setReports(latest);
          setLoadError("");
          const open = latest.find((report) => report.id === viewingId.current);
          if (open) setViewing(open);
        })
        .catch((failed: unknown) => {
          if (controller.signal.aborted) return;
          setLoadError(failure(failed, "口コミを読み込めませんでした。"));
        });
    }, RELOAD_DELAY_MS);
    return () => {
      window.clearTimeout(timer);
      controller.abort();
    };
  }, [bounds, reloads]);

  // 他の人の投稿を取り込むため、画面に戻ったとき（デモでは別タブの保存時も）読み直す。
  useEffect(() => {
    const reload = () => setReloads((count) => count + 1);
    const storage = (event: StorageEvent) => {
      if (event.key === DEMO_REPORTS_KEY || event.key === null) reload();
    };
    window.addEventListener("focus", reload);
    window.addEventListener("storage", storage);
    return () => {
      window.removeEventListener("focus", reload);
      window.removeEventListener("storage", storage);
    };
  }, []);

  function close() {
    setPoint(null);
    setViewing(null);
    setKind(null);
    setText("");
    setError("");
    setVoteKind(null);
    setVoteText("");
    setComposingPost(false);
  }
  function pick(next: LatLon) {
    ++positionGeneration.current;
    if (busy) return;
    close();
    const existing = reports.find(
      (report) => pointKey(report) === pointKey(next),
    );
    if (existing) setViewing(existing);
    else setPoint(next);
  }

  async function post(details: string) {
    if (!point || !kind || busy) return;
    setBusy(true);
    try {
      const report = await createSafetyReport({
        lat: point.lat,
        lon: point.lon,
        kind,
        text: details.trim() || null,
      });
      setReports((list) => upsert(list, report));
      // 書込み前に読んだ一覧が後から届いて上書きしないよう、読込を取り消して読み直す
      setReloads((count) => count + 1);
      setViewing(report);
      setPoint(null);
      setKind(null);
      setText("");
      setComposingPost(false);
      setError("");
    } catch (failed) {
      setError(failure(failed, SAVE_FAILED));
    } finally {
      setBusy(false);
    }
  }

  async function vote(details: string) {
    if (!viewing || !voteKind || busy) return;
    setBusy(true);
    try {
      const report = await voteSafetyReport(viewing.id, {
        kind: voteKind,
        text: details.trim() || null,
      });
      setReports((list) => upsert(list, report));
      setReloads((count) => count + 1);
      setViewing(report);
      setVoteKind(null);
      setVoteText("");
      setError("");
    } catch (failed) {
      setError(failure(failed, SAVE_FAILED));
      if (failed instanceof ApiError && failed.code === "already_voted") {
        // 別のタブ等で投票済み。最新の票を読み直して投票済みの表示にする
        setVoteKind(null);
        setVoteText("");
        setReloads((count) => count + 1);
      }
    } finally {
      setBusy(false);
    }
  }

  const forming = composingPost || voteKind !== null;
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
          if (busy) return;
          close();
          setViewing(report);
        }}
        onLocate={locate}
        onBoundsChange={setBounds}
        popupWidth={viewing || forming ? 320 : 146}
        popupHeight={viewing ? 240 : forming ? 200 : kind ? 136 : 82}
        bottomInset={80}
      >
        {viewing ? (
          voteKind ? (
            <CommunityDetailsForm
              kind={voteKind}
              count={
                (voteKind === "safe"
                  ? viewing.safe_count
                  : viewing.danger_count) + 1
              }
              value={voteText}
              onChange={setVoteText}
              onSkip={() => void vote("")}
              onConfirm={() => void vote(voteText)}
              error={error}
              busy={busy}
            />
          ) : (
            <CommunityReview
              key={viewing.id}
              report={viewing}
              onClose={close}
              onVote={(next) => {
                setError("");
                setVoteText("");
                setVoteKind(next);
              }}
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
              onSkip={() => void post("")}
              onConfirm={() => void post(text)}
              error={error}
              busy={busy}
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
      {loadError && (
        <p className="community-load-error" role="status">
          {loadError}
        </p>
      )}
    </main>
  );
}
