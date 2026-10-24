import { useEffect, useRef, useState } from "react";
import { isDemo } from "../api/client";
import type { Place } from "../lib/types";

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

/** frontend-spec 7.1: one explicit getCurrentPosition for the origin; stale callbacks are ignored. */
export function useCurrentLocation(onLocate: (place: Place) => void) {
  const [locating, setLocating] = useState(false);
  const [message, setMessage] = useState("");
  const token = useRef(0);

  useEffect(
    () => () => {
      ++token.current;
    },
    [],
  );

  /** Invalidates any pending request so its late result cannot change the origin. */
  function cancel() {
    ++token.current;
    setLocating(false);
  }

  function request() {
    if (isDemo) {
      onLocate({
        label: "東京駅（現在地デモ）",
        lat: 35.681236,
        lon: 139.767125,
      });
      setMessage(
        "デモの現在地を東京駅に設定しました。位置情報は取得していません。",
      );
      return;
    }
    if (!window.isSecureContext) {
      setMessage(
        "HTTPSで開いたページでのみ現在地を利用できます。住所を入力してください。",
      );
      return;
    }
    if (!navigator.geolocation) {
      setMessage(
        "このブラウザーでは現在地を利用できません。住所を入力してください。",
      );
      return;
    }
    const current = ++token.current;
    setLocating(true);
    setMessage("現在地を取得しています…");
    navigator.geolocation.getCurrentPosition(
      (position) => {
        if (token.current !== current) return;
        const { latitude: lat, longitude: lon, accuracy } = position.coords;
        if (![lat, lon, accuracy].every(Number.isFinite) || accuracy <= 0) {
          setLocating(false);
          setMessage(
            "現在地を正しく取得できませんでした。住所を入力してください。",
          );
          return;
        }
        if (lat < 20 || lat > 46 || lon < 122 || lon > 154) {
          setLocating(false);
          setMessage("日本国内の地点を指定してください。");
          return;
        }
        onLocate({ label: "現在地", lat, lon });
        setMessage(
          `現在地を出発地に設定しました（精度 約${Math.round(accuracy)}m）。`,
        );
      },
      (failure) => {
        if (token.current !== current) return;
        setLocating(false);
        setMessage(geolocationMessage(failure));
      },
      // frontend-spec 7.1
      { enableHighAccuracy: true, timeout: 10_000, maximumAge: 0 },
    );
  }

  return { locating, message, setMessage, request, cancel };
}
