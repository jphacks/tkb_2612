import { RealClock } from "./clock";
import type { PositionConsumer, PositionSource } from "./source";

export type DeviceError = "denied" | "unavailable" | "timeout";
export interface DeviceSourceEvents {
  onError(error: DeviceError): void;
}

/** frontend-spec 7.2: 開始時と明示再開時だけwatchPositionを登録する。エラーで再登録しない。 */
export const DEVICE_WATCH_OPTIONS: PositionOptions = {
  enableHighAccuracy: true,
  timeout: 10_000,
  maximumAge: 1000,
};

/** 実機の位置源。登録解除後に届いた古いコールバックは世代で捨てる。 */
export function createDeviceSource(
  geolocation: Pick<Geolocation, "watchPosition" | "clearWatch">,
  events: DeviceSourceEvents,
  clock: RealClock = new RealClock(),
): PositionSource {
  let consumer: PositionConsumer | null = null;
  let watchId: number | null = null;
  let generation = 0;
  function unregister() {
    generation++;
    if (watchId !== null) {
      try {
        geolocation.clearWatch(watchId);
      } catch {
        /* 解除失敗でも世代で旧コールバックを捨てる。 */
      }
      watchId = null;
    }
  }
  function register() {
    unregister();
    const mine = generation;
    try {
      watchId = geolocation.watchPosition(
        (position) => {
          if (mine !== generation || !consumer) return;
          const { longitude: lon, latitude: lat, accuracy } = position.coords;
          consumer({ lon, lat, accuracy, timestamp: position.timestamp });
        },
        (failure) => {
          if (mine !== generation) return;
          events.onError(
            failure.code === failure.PERMISSION_DENIED
              ? "denied"
              : failure.code === failure.TIMEOUT
                ? "timeout"
                : "unavailable",
          );
        },
        DEVICE_WATCH_OPTIONS,
      );
    } catch {
      events.onError("unavailable");
    }
  }
  return {
    clock,
    start(_realNow, next) {
      consumer = next;
      register();
    },
    pause: unregister,
    resume() {
      if (consumer) register();
    },
    stop() {
      unregister();
      consumer = null;
    },
  };
}
