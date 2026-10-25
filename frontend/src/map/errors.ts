import { mapLayers } from "./layers";

// styleはsprite失敗にも付くため、定義の取得・検証が済んでいるかも呼出元で確認する。
export function isFatalMapError(event: {
  error?: {
    message?: string;
  };
  sourceId?: unknown;
  source?: unknown;
  tile?: unknown;
  style?: unknown;
}, styleAvailable: boolean): boolean {
  const message = event.error?.message ?? "";
  if (/webgl|gl context|context lost/i.test(message)) return true;
  const sourceId = event.sourceId ?? (
    typeof event.source === "object" && event.source !== null && "id" in event.source
      ? event.source.id : undefined
  );
  // アプリのsourceが壊れた場合は、背景の部分欠落として継続しない。
  if (mapLayers.some((layer) => "source" in layer && layer.source === sourceId)) return true;
  if (event.tile != null || event.sourceId != null || event.source != null) return false;
  // MapLibreは初回・完全再生成時のルートstyle取得・検証に失敗するとgetStyle()が未定義になる。
  // 差分更新の失敗では旧定義が残るため、その場合も経路・位置の描画を続ける。
  // 定義がある場合のsprite/glyph等の補助リソース失敗では経路・位置の描画を続ける。
  return event.style != null && !styleAvailable;
}
