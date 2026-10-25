import { fireEvent, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resetApi } from "./test/apiMock";
import {
  advance,
  installGeolocation,
  lastRoute,
  openResults,
  positionAt,
  startGroup,
} from "./test/navigationApp";

vi.mock("./api/client", async () => (await import("./test/apiMock")).clientMock());
vi.mock("./components/LogoIntro", () => ({
  LogoIntro: ({ onComplete }: { onComplete: () => void }) => (
    <button onClick={onComplete}>動画をスキップして開始</button>
  ),
}));

beforeEach(resetApi);
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(navigator, "geolocation");
});

const AHEAD = "この先に、過去事故の参考区間があります。周囲に注意してください";
const DISCLAIMER = "過去事故の参考情報です。現在の危険や安全を保証するものではありません";

async function startDevice() {
  const gps = installGeolocation();
  await openResults();
  const route = await lastRoute("fastest");
  const section = route.navigation!.sections[0];
  vi.useFakeTimers();
  fireEvent.click(startGroup("最短時間").getByRole("button", { name: "実機GPSで開始" }));
  const feed = (progress: number) => {
    advance(1000);
    gps.emit(positionAt(route, progress));
  };
  return { route, section, feed };
}

describe("接近通知の画面表示（手順10・7.5・7.9）", () => {
  it("参考情報の注意を常設し、音声が使えない環境では画面通知に切り替える旨を示す", async () => {
    await startDevice();
    expect(screen.getByText(DISCLAIMER)).toBeVisible();
    expect(screen.getByText("音声を利用できません。画面通知を使用します")).toBeVisible();
    expect(screen.queryByLabelText("接近通知")).not.toBeInTheDocument();
  });

  it("区間の手前80mに入って1秒以上隔てた2測位で通知し、通過後に消して、後戻りでも再通知しない", async () => {
    const { section, feed } = await startDevice();
    // 座標の飛びと判定されない速さ（60m/s以下）で、通知距離の手前まで進める。
    for (let progress = 0; progress + 40 < section.start_m - 90; progress += 40) feed(progress + 40);
    expect(screen.queryByLabelText("接近通知")).not.toBeInTheDocument();
    feed(section.start_m - 75);
    expect(screen.queryByLabelText("接近通知")).not.toBeInTheDocument();
    feed(section.start_m - 71);
    const alert = screen.getByLabelText("接近通知");
    expect(alert).toHaveTextContent(AHEAD);
    // 受動表示：フォーカスを奪わず、ダイアログにもしない。
    expect(alert).not.toHaveAttribute("role", "dialog");
    expect(alert.contains(document.activeElement)).toBe(false);
    feed(section.start_m + 10);
    expect(screen.getByLabelText("接近通知")).toHaveTextContent("過去事故の参考区間にいます");
    for (let i = 0; i < 4; i++) feed(section.end_m + 20 + i * 15);
    expect(screen.queryByLabelText("接近通知")).not.toBeInTheDocument();
    // 通過済み区間は戻っても通知しない。
    for (let progress = section.end_m; progress > section.start_m - 70; progress -= 10) feed(progress);
    for (let i = 0; i < 3; i++) feed(section.start_m - 60);
    expect(screen.queryByLabelText("接近通知")).not.toBeInTheDocument();
  });
});
