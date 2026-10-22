// Gives today's page the `window.launcher` the Electron preload gave it (RA2): page/api.ts over Tauri's
// IPC. vite.config.ts runs this module before the page's own script, which reads `window.launcher` once
// when it loads. RA13 switches the page to import the bridge itself (DR's backend.ts), and this file goes.

import { api } from "./api.ts";

// Tauri sets `__TAURI_INTERNALS__` before any page script runs
if ("__TAURI_INTERNALS__" in window) {
  Object.defineProperty(window, "launcher", { value: Object.freeze(api), writable: false, configurable: false });
  // No browser context menu, as in the Electron launcher (debug builds still open devtools with F12)
  document.addEventListener("contextmenu", (event) => event.preventDefault());
}
