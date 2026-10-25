/// <reference types="vite/client" />

// Build-time public values (architecture.md 6). Never put secrets in VITE_*.
interface ImportMetaEnv {
  readonly VITE_ENABLE_SIMULATION?: string;
  readonly VITE_API_MODE?: "demo" | "live";
  readonly VITE_MAP_STYLE_URL?: string;
  readonly VITE_ROUTING_ATTRIBUTION?: string;
  readonly VITE_MAP_ATTRIBUTION?: string;
}
