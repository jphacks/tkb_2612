import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api/errors";
import { createDemoConditions, DEMO_PLACES } from "../api/demo";
import { toTokyoInput } from "../lib/format";
import { useConditions } from "./useConditions";

const client = vi.hoisted(() => ({ getConditions: vi.fn() }));
vi.mock("../api/client", () => client);

const origin = { lat: DEMO_PLACES[0].lat, lon: DEMO_PLACES[0].lon };
const dest = { lat: DEMO_PLACES[1].lat, lon: DEMO_PLACES[1].lon };
const moved = { lat: 35.69, lon: 139.75 };

beforeEach(() => {
  client.getConditions.mockReset().mockImplementation((datetime: string, from?: unknown) =>
    Promise.resolve(createDemoConditions(datetime, from ? origin : undefined)),
  );
});

function setup() {
  const onEdit = vi.fn();
  const view = renderHook(
    (props: { origin: typeof origin; dest: typeof dest }) =>
      useConditions({ ...props, scenario: "normal", onEdit }),
    { initialProps: { origin, dest } },
  );
  return { ...view, onEdit };
}

async function reSearch(view: ReturnType<typeof setup>) {
  let values!: Awaited<ReturnType<typeof view.result.current.refreshForReSearch>>;
  await act(async () => {
    const pending = view.result.current.refreshForReSearch(
      moved,
      dest,
      new AbortController().signal,
      "normal",
    );
    // 呼出し側は同じ同期区間でoriginを更新する。
    view.rerender({ origin: moved, dest });
    values = await pending;
  });
  return values;
}

describe("useConditions.refreshForReSearch（7.6の条件取得）", () => {
  it("直前の取得から15分未満でも新originで必ず1回取り直し、origin変更による自動取得を重ねない。nowのmanualは保持する", async () => {
    const view = setup();
    await waitFor(() => expect(client.getConditions).toHaveBeenCalledTimes(1));
    act(() => view.result.current.change({ weather: "雨" }));
    const values = await reSearch(view);
    expect(client.getConditions).toHaveBeenCalledTimes(2);
    const [datetime, from, to] = client.getConditions.mock.calls[1];
    expect(datetime.slice(0, 16)).toBe(`${toTokyoInput(new Date())}`.slice(0, 16));
    expect(from).toEqual(moved);
    expect(to).toEqual(dest);
    expect(values.weather).toBe("雨");
    expect(view.result.current.auto.weather).toBe(false);
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(client.getConditions).toHaveBeenCalledTimes(2);
  });

  it("customの出発日時は現在へ戻し、自動項目はautoへ戻して取得し直す", async () => {
    const view = setup();
    await waitFor(() => expect(client.getConditions).toHaveBeenCalledTimes(1));
    act(() => view.result.current.changeDeparture("custom", "2026-12-24T10:00"));
    await waitFor(() => expect(client.getConditions).toHaveBeenCalledTimes(2));
    act(() => view.result.current.change({ weather: "雨" }));
    const values = await reSearch(view);
    expect(client.getConditions).toHaveBeenCalledTimes(3);
    expect(client.getConditions.mock.calls[2][0]).not.toContain("2026-12-24");
    expect(view.result.current.departure).toBe("now");
    expect(view.result.current.auto).toEqual({ period: true, day_type: true, weather: true });
    expect(values.weather).toBe("晴");
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(client.getConditions).toHaveBeenCalledTimes(3);
  });

  it("取得に失敗したらautoの項目だけ指定なしに戻し、manualは維持して続行する（自動リトライなし）", async () => {
    const view = setup();
    await waitFor(() => expect(client.getConditions).toHaveBeenCalledTimes(1));
    act(() => view.result.current.change({ weather: "雨" }));
    client.getConditions.mockRejectedValueOnce(new ApiError("失敗", "service_busy", 503));
    const values = await reSearch(view);
    expect(values).toMatchObject({ period: "all", day_type: "all", weather: "雨" });
    expect(view.result.current.warning).toContain("自動項目は指定なしに戻しました");
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(client.getConditions).toHaveBeenCalledTimes(2);
  });

  it("その後の通常の入力変更では、従来どおり自動取得する", async () => {
    const view = setup();
    await waitFor(() => expect(client.getConditions).toHaveBeenCalledTimes(1));
    await reSearch(view);
    act(() => view.rerender({ origin: moved, dest: { lat: 35.7, lon: 139.8 } }));
    await waitFor(() => expect(client.getConditions).toHaveBeenCalledTimes(3));
  });
});
