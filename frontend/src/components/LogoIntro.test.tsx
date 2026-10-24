import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LogoIntro } from "./LogoIntro";

describe("LogoIntro", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined);
    vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
    vi.stubGlobal("matchMedia", vi.fn().mockReturnValue({ matches: false }));
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("plays on each mount and completes after the left slide, rather than on the video event alone", async () => {
    const completed = vi.fn();
    const first = render(<LogoIntro onComplete={completed} />);
    await act(async () => {});
    expect(HTMLMediaElement.prototype.play).toHaveBeenCalledTimes(1);
    fireEvent.ended(screen.getByLabelText("AcciLessNavi のロゴアニメーション"));
    expect(screen.getByLabelText("AcciLessNavi ロゴ動画")).toHaveClass(
      "logo-intro-leaving",
    );
    expect(completed).not.toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(450));
    expect(completed).toHaveBeenCalledTimes(1);
    first.unmount();
    render(<LogoIntro onComplete={completed} />);
    await act(async () => {});
    expect(HTMLMediaElement.prototype.play).toHaveBeenCalledTimes(2);
    expect(screen.getByLabelText("AcciLessNavi ロゴ動画")).not.toHaveClass(
      "logo-intro-leaving",
    );
  });

  it("allows progression when automatic playback is rejected", async () => {
    vi.mocked(HTMLMediaElement.prototype.play).mockRejectedValue(
      new Error("autoplay denied"),
    );
    const completed = vi.fn();
    render(<LogoIntro onComplete={completed} />);
    await act(async () => {});
    expect(screen.getByRole("button", { name: "動画を再生" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "スキップして始める" }));
    act(() => vi.advanceTimersByTime(450));
    expect(completed).toHaveBeenCalledTimes(1);
  });
});
