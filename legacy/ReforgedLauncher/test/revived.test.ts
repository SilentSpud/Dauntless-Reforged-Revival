// Dauntless 1.4.4 for Dauntless Revived servers (src/main/revived/): the pins from their launcher
// 0.1.35, its command line, its config lines, and finding a 1.4.4 their launcher downloaded already.

import { test, after } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { GAME_MANIFEST, compareManifests } from "../src/main/revived/manifest";
import { REVIVED_DLLS, REVIVED_EXE_RELATIVE_PATH, REVIVED_EXE_SHA256, REVIVED_EXE_SIZE } from "../src/main/revived/constants";
import { buildRevivedArgs, missingDirectX } from "../src/main/revived/launch";
import { detectRevivedInstalls } from "../src/main/revived/detect";
import { ensureDllCache, installDlls, dllsInstalled } from "../src/main/revived/dlls";
import { rewriteEngineIniText } from "../src/main/engineini";
import { maskArgs } from "../src/main/launch";
import { pattern, sha256 } from "./fakes";

const dirs: string[] = [];
after(() => {
  for (const d of dirs) rmSync(d, { recursive: true, force: true });
});
function temp(): string {
  const d = mkdtempSync(path.join(tmpdir(), "rr-revived-"));
  dirs.push(d);
  return d;
}

test("the compiled-in 1.4.4 manifest is 0.1.35's: 410 files, the pinned executable, no client DLL in it", () => {
  assert.ok(GAME_MANIFEST);
  assert.equal(GAME_MANIFEST!.files.length, 410);
  assert.equal(GAME_MANIFEST!.totalBytes, 10_893_512_875);
  const exe = GAME_MANIFEST!.files.find((f) => f.path === REVIVED_EXE_RELATIVE_PATH);
  assert.deepEqual(exe, { path: REVIVED_EXE_RELATIVE_PATH, size: REVIVED_EXE_SIZE, sha256: REVIVED_EXE_SHA256 });
  for (const d of REVIVED_DLLS) {
    assert.ok(!GAME_MANIFEST!.files.some((f) => f.path.toLowerCase().endsWith(d.name.toLowerCase())));
    assert.match(d.url, /^https:\/\/raw\.githubusercontent\.com\/mixutin\/dauntless-revived\/0e2b1e6730b9f14f1020d7c903a6955464fa4586\//);
  }
  // A server whose list differs in one hash is refused.
  const other = { ...GAME_MANIFEST!, files: GAME_MANIFEST!.files.map((f, i) => (i === 7 ? { ...f, sha256: "0".repeat(64) } : f)) };
  assert.equal(compareManifests(GAME_MANIFEST!, GAME_MANIFEST!).equal, true);
  assert.equal(compareManifests(GAME_MANIFEST!, other).equal, false);
});

test("the 1.4.4 command line: the server first, the key hidden in logs, -NoEAC", () => {
  const key = "RRK_" + "a".repeat(40);
  const args = buildRevivedArgs({ host: "100.88.12.40", port: 61000, key, windowed: true, gameLocale: "pt-BR" });
  assert.equal(args[0], "100.88.12.40:61000");
  assert.equal(args[1], `-AUTH_PASSWORD=${key}`);
  assert.ok(args.includes("-epiclocale=pt-BR") && args.includes("-NoEAC") && args.includes("-windowed"));
  assert.ok(!maskArgs(args).join(" ").includes(key));
  assert.throws(() => buildRevivedArgs({ host: "bad host", port: 61000, key, windowed: false }));
  assert.deepEqual(missingDirectX({ WINDIR: "C:\\\\Windows" }, () => false, "win32"), ["XINPUT1_3.dll", "X3DAudio1_7.dll", "XAPOFX1_5.dll"]);
  assert.deepEqual(missingDirectX({}, () => false, "linux"), []);
});

test("Engine.ini for 1.4.4: the memory lines always, chat at the host", () => {
  const lines = rewriteEngineIniText(["[Core.Log]", "LogX=Log"], "100.88.12.40", 61099, -1, true);
  assert.deepEqual(lines.slice(0, 5), ["[SystemSettings]", "r.Streaming.PoolSize=3000", "r.Streaming.LimitPoolSizeToVRAM=1", "gc.TimeBetweenPurgingPendingKillObjects=10", "s.ForceGCAfterLevelStreamedOut=1"]);
  assert.ok(lines.includes('ServerAddr="ws://100.88.12.40"') && lines.includes("ServerPort=61099"));
  assert.ok(lines.includes("[Core.Log]"));
  // 1.14.7 is unchanged: no section without a forced graphics level.
  assert.ok(!rewriteEngineIniText([], "127.0.0.1", 61100, -1).includes("[SystemSettings]"));
});

test("a 1.4.4 their launcher downloaded is found: its settings' folder, or its default one", async () => {
  const appData = temp();
  const localAppData = temp();
  const own = path.join(temp(), "DR Game");
  const exe = (root: string, size: number) => {
    const file = path.join(root, ...REVIVED_EXE_RELATIVE_PATH.split("/"));
    mkdirSync(path.dirname(file), { recursive: true });
    writeFileSync(file, Buffer.alloc(size));
  };
  assert.deepEqual(await detectRevivedInstalls({ APPDATA: appData, LOCALAPPDATA: localAppData }), []);
  mkdirSync(path.join(appData, "Dauntless Revived Launcher"), { recursive: true });
  writeFileSync(path.join(appData, "Dauntless Revived Launcher", "settings.json"), JSON.stringify({ installDir: own }));
  exe(own, REVIVED_EXE_SIZE);
  exe(path.join(localAppData, "DauntlessRevived", "Game"), 1234); // not the 1.4.4 executable
  const found = await detectRevivedInstalls({ APPDATA: appData, LOCALAPPDATA: localAppData });
  assert.deepEqual(found, [{ root: own, source: "revived", version: "1.4.4" }]);
});

test("the client DLLs: kept only when their hash is the pinned one, then copied next to the game", async () => {
  const good = pattern(900, 2);
  const dlls = [{ name: "dxgi.dll", sha256: sha256(good), size: good.length, url: "https://raw.example.invalid/dxgi.dll" }];
  const cache = temp();
  await assert.rejects(ensureDllCache(cache, async () => Buffer.from("not it"), dlls), /not the pinned one/);
  await ensureDllCache(cache, async () => good, dlls);
  await ensureDllCache(cache, async () => {
    throw new Error("not asked again");
  }, dlls);
  const root = temp();
  assert.equal(await dllsInstalled(root, dlls), false);
  await installDlls(cache, root, dlls);
  assert.equal(await dllsInstalled(root, dlls), true);
});
