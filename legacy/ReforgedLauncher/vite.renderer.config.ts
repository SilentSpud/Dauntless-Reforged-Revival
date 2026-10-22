import { defineConfig } from "vite";

// The page is plain TypeScript and CSS (no framework). Assets are never inlined as data URLs,
// so the Content-Security-Policy in index.html can stay strict.
export default defineConfig({
  build: {
    assetsInlineLimit: 0,
  },
});
