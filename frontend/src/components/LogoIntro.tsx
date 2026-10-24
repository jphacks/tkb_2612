import { useEffect, useRef, useState } from "react";
import { ArrowRight, Play } from "lucide-react";
import "./intro.css";

type LogoIntroProps = { onComplete: () => void };

/** A fresh component is mounted on every document load; no persistence skips the video. */
export function LogoIntro({ onComplete }: LogoIntroProps) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const completeRef = useRef(onComplete);
  const [playback, setPlayback] = useState<"playing" | "blocked" | "failed">(
    "playing",
  );
  const [leaving, setLeaving] = useState(false);
  completeRef.current = onComplete;

  useEffect(() => {
    let active = true;
    const video = videoRef.current;
    if (!video) return;
    const start = video.play();
    start?.catch(() => {
      if (active) setPlayback("blocked");
    });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    if (!leaving) return;
    const reducedMotion = window.matchMedia?.(
      "(prefers-reduced-motion: reduce)",
    ).matches;
    const timeout = window.setTimeout(
      () => completeRef.current(),
      reducedMotion ? 0 : 450,
    );
    return () => window.clearTimeout(timeout);
  }, [leaving]);

  const finish = () => {
    videoRef.current?.pause();
    setLeaving(true);
  };

  const play = () => {
    const video = videoRef.current;
    if (!video) return;
    setPlayback("playing");
    void video.play().catch(() => setPlayback("failed"));
  };

  return (
    <section
      className={`logo-intro${leaving ? " logo-intro-leaving" : ""}`}
      aria-label="AcciLessNavi ロゴ動画"
      aria-busy={leaving}
    >
      <div className="logo-intro-content">
        <video
          ref={videoRef}
          className="logo-intro-video"
          src="/assets/acciless-navi.webm"
          poster="/assets/acciless-navi-poster.jpg"
          autoPlay
          muted
          playsInline
          preload="auto"
          aria-label="AcciLessNavi のロゴアニメーション"
          onEnded={finish}
          onError={() => setPlayback("failed")}
          onPlaying={() => setPlayback("playing")}
        />
        <div className="logo-intro-status" aria-live="polite">
          {playback === "blocked" && (
            <p>ロゴ動画を再生するか、そのまま始められます。</p>
          )}
          {playback === "failed" && (
            <p>動画を再生できませんでした。そのまま始められます。</p>
          )}
          {playback !== "playing" && (
            <button
              type="button"
              className="intro-play-button"
              onClick={play}
              disabled={leaving}
            >
              <Play size={18} aria-hidden="true" /> 動画を再生
            </button>
          )}
        </div>
      </div>
      <button
        type="button"
        className="intro-skip-button"
        onClick={finish}
        disabled={leaving}
      >
        スキップして始める <ArrowRight size={18} aria-hidden="true" />
      </button>
    </section>
  );
}
