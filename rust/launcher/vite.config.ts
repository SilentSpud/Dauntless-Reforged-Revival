// Builds today's launcher page (legacy/ReforgedLauncher, RA1) for the Tauri launcher, the way its own
// vite.renderer.config.ts builds it for Electron (no assets inlined as data URLs), with two changes made
// here so the page's files stay as they are until RA13 and RR5:
//
//  - the page's Content-Security-Policy meta tag is dropped: Tauri sends the same policy from
//    tauri.conf.json, and with the tag left in every call to the backend hangs, because its
//    default-src 'none' and connect-src 'self' block the isolation frame and the IPC endpoint (DR's L10);
//  - page/bridge.ts runs first and gives the page the `window.launcher` the Electron preload gave it, over
//    Tauri's IPC (page/api.ts).

import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";

const here = fileURLToPath(new URL(".", import.meta.url));
const page = fileURLToPath(new URL("../../legacy/ReforgedLauncher/", import.meta.url));
const bridge = fileURLToPath(new URL("./page/bridge.ts", import.meta.url));
const BRIDGE_ID = "/@launcher-bridge";

function forTauri(): Plugin {
  return {
    name: "rr-tauri-page",
    resolveId(id) {
      return id === BRIDGE_ID ? bridge : null;
    },
    // Before Vite bundles the scripts the page names
    transformIndexHtml: {
      order: "pre",
      handler(html) {
        const withoutMeta = html.replace(/\s*<meta\s+http-equiv="Content-Security-Policy"[^>]*\/>/, "");
        if (withoutMeta === html) throw new Error("index.html: the Content-Security-Policy meta tag was not found");
        const entry = /<script type="module" src="\/src\/renderer\/index\.ts"><\/script>/;
        if (!entry.test(withoutMeta)) throw new Error("index.html: the page's script was not found");
        return withoutMeta.replace(entry, (tag) => `<script type="module" src="${BRIDGE_ID}"></script>\n    ${tag}`);
      },
    },
  };
}

export default defineConfig({
  root: page,
  build: {
    assetsInlineLimit: 0,
    outDir: `${here}dist-page`,
    emptyOutDir: true,
  },
  // Tauri's dev server address is fixed in tauri.conf.json (devUrl)
  server: { port: 5173, strictPort: true, fs: { allow: [page, here] } },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  plugins: [forTauri()],
});
