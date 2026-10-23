// Opens the launcher's page in a plain browser with a fake main process (preview/mock-launcher.ts):
//   npm run preview                then open http://localhost:5199/?scene=ready&lang=pt-BR
// Scenes: join, tailscale, cert, register, locate, wrong, trust, noruntime, installing, ready,
// running, synthetic, error, host-empty, host-running, host-noproject, revived, revived-repair, revived-download, revived-ready. Add &view=news|server|host|settings|credits, &tab=launcher,
// &modal=trust or &streamer=1. For looking at the design only: nothing here talks to a server.
import { createServer } from "vite";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const port = Number(process.env.PREVIEW_PORT ?? 5199);

const server = await createServer({
  root,
  configFile: path.join(root, "vite.renderer.config.ts"),
  server: { port, strictPort: true, host: "127.0.0.1" },
  plugins: [
    {
      name: "reforged-preview-mock",
      transformIndexHtml(html) {
        return html.replace('<script type="module" src="/src/renderer/index.ts"></script>', '<script type="module" src="/preview/mock-launcher.ts"></script>\n    <script type="module" src="/src/renderer/index.ts"></script>');
      },
    },
  ],
});
await server.listen();
console.log(`Launcher preview: http://127.0.0.1:${port}/?scene=ready&lang=en`);
