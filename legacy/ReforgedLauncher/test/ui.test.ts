// What the page shows for a Snapshot: the PLAY plate, the steps and the Host page's ratings.

import { test } from "node:test";
import assert from "node:assert/strict";
import { hostSteps, primaryButton, shortHash, stepIndex, taskView } from "../src/shared/ui-model";
import type { Snapshot } from "../src/shared/types";

function snap(over: Partial<Snapshot> = {}): Snapshot {
  return {
    phase: "join",
    busy: false,
    server: null,
    savedServers: [],
    connect: { checking: false, problem: null, tailscaleInstalled: true, lastCheckedAt: null },
    account: { username: null, hasKey: false, offerBackup: false },
    game: { root: null, problem: "not_set", versionText: null, versionOk: false, exeVerified: false, detected: [] },
    revived: null,
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

const form = { inviteValid: true, usernameValid: true };

test("the primary button follows the phase", () => {
  assert.deepEqual(primaryButton(snap(), form), { labelKey: "btn_join", action: "join", disabled: false });
  assert.equal(primaryButton(snap(), { ...form, inviteValid: false }).disabled, true);
  assert.equal(primaryButton(snap({ phase: "connect", connect: { checking: false, problem: "cert_mismatch", tailscaleInstalled: true, lastCheckedAt: null } }), form).action, "new_invite");
  assert.equal(primaryButton(snap({ phase: "connect", connect: { checking: false, problem: "unreachable", tailscaleInstalled: true, lastCheckedAt: null } }), form).action, "retry");
  assert.equal(primaryButton(snap({ phase: "register" }), form).action, "register");
  assert.equal(primaryButton(snap({ phase: "locate" }), form).action, "locate");
  assert.equal(primaryButton(snap({ phase: "locate", game: { ...snap().game, problem: "unverified" } }), form).action, "verify");
  const rt = (state: Snapshot["runtime"]["state"]) => primaryButton(snap({ phase: "runtime", runtime: { state, version: "1", files: [], localDir: null } }), form);
  assert.equal(rt("untrusted").action, "trust");
  assert.deepEqual(rt("missing"), { labelKey: "btn_install", action: "install_runtime", disabled: false });
  assert.deepEqual(rt("outdated"), { labelKey: "btn_update", action: "install_runtime", disabled: false });
  assert.deepEqual(rt("unavailable"), { labelKey: "btn_waiting_host", action: "none", disabled: true });
  assert.equal(primaryButton(snap({ phase: "installing" }), form).action, "cancel");
  assert.deepEqual(primaryButton(snap({ phase: "ready" }), form), { labelKey: "btn_play", action: "play", disabled: false });
  assert.equal(primaryButton(snap({ phase: "ready", busy: true }), form).disabled, true);
  assert.equal(primaryButton(snap({ phase: "running" }), form).labelKey, "btn_running");
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
