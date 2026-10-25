export function cacheName(version: string): string {
  return `acciless-navi-shell-${version}`;
}

export function oldShellCaches(names: readonly string[], current: string): string[] {
  return names.filter((name) => name.startsWith("acciless-navi-shell-") && name !== current);
}

export function isShellRequest(
  url: string, method: string, origin: string, assets: readonly string[],
): boolean {
  const target = new URL(url, origin);
  return method === "GET" && target.origin === origin &&
    target.pathname !== "/api" && !target.pathname.startsWith("/api/") &&
    !target.search && assets.includes(target.pathname);
}

export function canReloadForUpdate(waiting: boolean, reasons: readonly string[]): boolean {
  return waiting && reasons.length === 0;
}
