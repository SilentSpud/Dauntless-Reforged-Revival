// The renderer's globals: Vite's asset imports and the bridge from preload.ts.

/// <reference types="vite/client" />

import type { LauncherApi } from "../preload";

declare global {
  interface Window {
    launcher: LauncherApi;
  }
}

export {};
