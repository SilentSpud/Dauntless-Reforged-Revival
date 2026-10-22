// What the page shows for the Rust controller's snapshots (Reforged's test/ui.test.ts, RA13): the PLAY
// plate, the steps and the Host page's ratings, from legacy/ReforgedLauncher/src/shared/ui-model.ts. The
// snapshots are typed as the bridge generates them (page/generated/types.ts, from bridge/types.rs), so a
// field the Rust snapshot lacks or renames fails the typecheck here before the page sees it.

import { test } from "node:test";
import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import type { Snapshot as RustSnapshot } from "../page/generated/types.ts";
import type { Snapshot as PageSnapshot } from "../../../legacy/ReforgedLauncher/src/shared/types.ts";

// The legacy page imports its own files without an extension (Vite and tsc resolve them); Node needs `.ts`
registerHooks({
  resolve(specifier, context, nextResolve) {
    try {
      return nextResolve(specifier, context);
    } catch (error) {
      if (!specifier.startsWith(".") || specifier.endsWith(".ts")) throw error;
      return nextResolve(`${specifier}.ts`, context);
    }
  },
});
const { hostSteps, primaryButton, shortHash, stepIndex, taskView } = await import("../../../legacy/ReforgedLauncher/src/shared/ui-model.ts");

function snap(over: Partial<RustSnapshot> = {}): RustSnapshot {
  return {
    phase: "join",
    busy: false,
    server: null,
    savedServers: [],
    connect: { checking: false, problem: null, tailscaleInstalled: true, lastCheckedAt: null },
    account: { username: null, hasKey: false, offerBackup: false },
    game: { root: null, problem: "not_set", versionText: null, versionOk: false, exeVerified: false, detected: [] },
    runtime: { state: "unavailable", version: null, files: [], localDir: null },
    task: null,
    running: { game: false, relayPort: null },
    settings: { graphics: -1, windowed: false, language: "en", gameLanguage: "auto", runtimeProfile: "production", streamerMode: false, hostTools: false },
    app: { version: "0.1.0", packaged: false, updateReady: false, platform: "win32" },
    status: null,
    statusUnsupported: false,
    lastError: null,
    notice: null,
    ...over,
  };
}

// The page reads the Rust snapshot through its own type; the 1.4.4 parts (`revived`, `game`) are absent
const page = (s: RustSnapshot): PageSnapshot => ({ revived: null, ...s }) as unknown as PageSnapshot;
const form = { inviteValid: true, usernameValid: true };
const plate = (over: Partial<RustSnapshot> = {}, f = form) => primaryButton(page(snap(over)), f);

test("the primary button follows the phase", () => {
  assert.deepEqual(plate(), { labelKey: "btn_join", action: "join", disabled: false });
  assert.equal(plate({}, { ...form, inviteValid: false }).disabled, true);
  assert.equal(plate({ phase: "connect", connect: { checking: false, problem: "cert_mismatch", tailscaleInstalled: true, lastCheckedAt: null } }).action, "new_invite");
  assert.equal(plate({ phase: "connect", connect: { checking: false, problem: "unreachable", tailscaleInstalled: true, lastCheckedAt: null } }).action, "retry");
  assert.equal(plate({ phase: "register" }).action, "register");
  assert.equal(plate({ phase: "locate" }).action, "locate");
  assert.equal(plate({ phase: "locate", game: { ...snap().game, problem: "unverified" } }).action, "verify");
  const rt = (state: RustSnapshot["runtime"]["state"]) => plate({ phase: "runtime", runtime: { state, version: "1", files: [], localDir: null } });
  assert.equal(rt("untrusted").action, "trust");
  assert.deepEqual(rt("missing"), { labelKey: "btn_install", action: "install_runtime", disabled: false });
  assert.deepEqual(rt("outdated"), { labelKey: "btn_update", action: "install_runtime", disabled: false });
  assert.deepEqual(rt("unavailable"), { labelKey: "btn_waiting_host", action: "none", disabled: true });
  assert.equal(plate({ phase: "installing" }).action, "cancel");
  assert.deepEqual(plate({ phase: "ready" }), { labelKey: "btn_play", action: "play", disabled: false });
  assert.equal(plate({ phase: "ready", busy: true }).disabled, true);
  assert.equal(plate({ phase: "running" }).labelKey, "btn_running");
});

test("steps: Join, Register, Game, Runtime, Play", () => {
  assert.equal(stepIndex("join"), 0);
  assert.equal(stepIndex("connect"), 0);
  assert.equal(stepIndex("register"), 1);
  assert.equal(stepIndex("locate"), 2);
  assert.equal(stepIndex("runtime"), 3);
  assert.equal(stepIndex("installing"), 3);
  assert.equal(stepIndex("ready"), 4);
});

test("taskView and shortHash", () => {
  const v = taskView({ kind: "runtime", totalBytes: 2_000_000, doneBytes: 1_000_000, filesTotal: 2, filesDone: 1, bytesPerSecond: 500_000, etaSeconds: 2, currentFile: "winmm.dll" }, "en");
  assert.equal(v.titleKey, "task_runtime");
  assert.equal(v.percent, 50);
  assert.equal(v.filesText, "1/2");
  assert.ok(v.speedText && v.etaText);
  assert.equal(taskView({ kind: "verify", totalBytes: 10, doneBytes: 5, filesTotal: 1, filesDone: 0, bytesPerSecond: 0, etaSeconds: null, currentFile: null }, "pt-BR").speedText, null);
  assert.equal(shortHash("f3f95fbf22709ef077a11632ed95d675b7209b7e0172d9ecb9ba7d02a1fbb66b"), "f3f9 5fbf 2270 9ef0…");
});

test("hostSteps rates each Host step from what is on disk", () => {
  const none = hostSteps({ workspaceOk: false, upstream: { present: false, patched: false }, sdk: { present: false, functionFiles: 0 }, builds: { runtime: false, loader: false, exporter: false, published: false }, gameData: { backend: "missing", director: "missing" } });
  assert.deepEqual(none, { workspace: "todo", sdk: "todo", build: "todo", gameData: "todo", publish: "todo" });
  const partial = hostSteps({ workspaceOk: true, upstream: { present: true, patched: false }, sdk: { present: true, functionFiles: 120 }, builds: { runtime: true, loader: false, exporter: false, published: false }, gameData: { backend: "synthetic", director: "real" } });
  assert.deepEqual(partial, { workspace: "warn", sdk: "done", build: "todo", gameData: "warn", publish: "todo" });
  const all = hostSteps({ workspaceOk: true, upstream: { present: true, patched: true }, sdk: { present: true, functionFiles: 1 }, builds: { runtime: true, loader: true, exporter: true, published: true }, gameData: { backend: "real", director: "real" } });
  assert.deepEqual(all, { workspace: "done", sdk: "done", build: "done", gameData: "done", publish: "done" });
});

test("a saved 1.4.4 server comes from the Rust snapshot marked unsupported (RD22)", () => {
  const saved = { id: "0123456789abcdef01234567", mode: "private" as const, host: "100.64.0.9", port: 61000, name: "Old Hunts", current: false, username: null, unsupported: true };
  const s = page(snap({ savedServers: [saved] }));
  assert.equal(s.savedServers[0].unsupported, true);
});
