// The Host page's checks and file steps, on a fake workspace.

import { test, after } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import {
  BACKEND_GAME_DATA,
  countFunctionFiles,
  detectGameData,
  detectHostKit,
  EXPORT_FLAGS,
  findDumps,
  hostKitDir,
  importSdk,
  msbuildArgs,
  platformToolset,
  publishRuntime,
  useSyntheticData,
  workspacePaths,
} from "../src/main/hostkit";
import { parseRuntimeManifest } from "../src/main/hostapi";
import { sha256 } from "./fakes";

const dirs: string[] = [];
after(() => {
  for (const d of dirs) rmSync(d, { recursive: true, force: true });
});

function temp(): string {
  const d = mkdtempSync(path.join(tmpdir(), "rr-host-"));
  dirs.push(d);
  return d;
}

function write(file: string, data: string | Buffer): void {
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, data);
}

test("CatalogExporter flags: the catalog, Slayer's Path, hunts, store and progression tables, and nothing client-only", () => {
  const on = EXPORT_FLAGS.split("\r\n").filter((l) => /=1$/.test(l)).map((l) => l.slice(0, -2));
  assert.deepEqual(on, ["EXPORT_CATALOG", "EXPORT_SLAYERS_PATH", "EXPORT_HUNTS", "EXPORT_STORE_RUNTIME", "EXPORT_PROGRESSION"]);
  assert.ok(EXPORT_FLAGS.includes("EXPORT_SKINS_RESOLVE_STRINGTABLE=0"));
});

test("workspace paths follow the checkout: Mystic Paradox in paradox/ and native/", () => {
  const w = workspacePaths("C:\\rr", path.win32);
  assert.equal(w.upstream, "C:\\rr\\paradox");
  assert.equal(w.patchMarker, null);
  assert.equal(w.runtimeDll, "C:\\rr\\native\\paradox-runtime\\x64\\Release\\MysticParadox.dll");
  assert.equal(w.loaderDll, "C:\\rr\\native\\runtime-loader\\target\\release\\winmm.dll");
  assert.equal(w.exporterDll, "C:\\rr\\native\\catalog-exporter\\x64\\Release\\CatalogExporter.dll");
  assert.equal(w.backendGameData, "C:\\rr\\paradox\\backend\\game-data");
  assert.equal(w.exports, "C:\\rr\\paradox\\Items_Analysis");
  assert.equal(w.publish, "C:\\rr\\.workspace\\runtime");
  assert.equal(w.hostKit, "C:\\rr\\legacy\\ReforgedHost\\package.json");
});

test("a checkout from before Mystic Paradox was merged in keeps the downloaded layout", () => {
  const root = temp();
  write(path.join(root, "upstream", "mystic-paradox.json"), "{}");
  const w = workspacePaths(root);
  assert.equal(w.upstream, path.join(root, ".workspace", "Mystic-Paradox"));
  assert.equal(w.runtimeDll, path.join(root, ".workspace", "Mystic-Paradox", "ParadoxRuntime", "x64", "Release", "MysticParadox.dll"));
  assert.equal(w.loaderDll, path.join(root, ".workspace", "Mystic-Paradox", "tools", "RuntimeLoader", "target", "release", "winmm.dll"));
  assert.equal(w.patchMarker, path.join(root, ".workspace", "Mystic-Paradox", ".reforged-patches.json"));
  write(path.join(root, "paradox", "backend", "package.json"), "{}");
  assert.equal(workspacePaths(root).upstream, path.join(root, "paradox"), "once updated, the checkout's own");
});

test("the host kit is in legacy/, or where it was in a checkout from before the move", () => {
  const root = temp();
  assert.equal(hostKitDir(root), path.join(root, "legacy", "ReforgedHost"));
  write(path.join(root, "ReforgedHost", "package.json"), "{}");
  assert.equal(hostKitDir(root), path.join(root, "ReforgedHost"));
  write(path.join(root, "legacy", "ReforgedHost", "package.json"), "{}");
  assert.equal(hostKitDir(root), path.join(root, "legacy", "ReforgedHost"));
});

test("game data: missing, synthetic (the shipped examples) and real", async () => {
  const dir = temp();
  assert.equal(await detectGameData(dir, BACKEND_GAME_DATA), "missing");
  for (const name of BACKEND_GAME_DATA) write(path.join(dir, name.replace(/\.json$/, ".example.json")), `{"example":"${name}"}`);
  assert.equal(await useSyntheticData(dir, BACKEND_GAME_DATA), BACKEND_GAME_DATA.length);
  assert.equal(await detectGameData(dir, BACKEND_GAME_DATA), "synthetic");
  write(path.join(dir, BACKEND_GAME_DATA[0]), '{"real":true}');
  assert.equal(await useSyntheticData(dir, BACKEND_GAME_DATA), 0, "never overwrites");
  assert.equal(readFileSync(path.join(dir, BACKEND_GAME_DATA[0]), "utf8"), '{"real":true}');
  for (const name of BACKEND_GAME_DATA) write(path.join(dir, name), `{"real":"${name}"}`);
  assert.equal(await detectGameData(dir, BACKEND_GAME_DATA), "real");
});

test("Dumper-7 output: the newest CppSDK is found and imported into the runtime source", async () => {
  const root = temp();
  const older = path.join(root, "4.26.2-647472-Dauntless-old", "CppSDK");
  const newer = path.join(root, "4.26.2-647472-Dauntless", "CppSDK");
  write(path.join(older, "SDK.hpp"), "// old");
  write(path.join(newer, "SDK.hpp"), "// new");
  write(path.join(newer, "SDK", "Engine_functions.cpp"), "// fn");
  write(path.join(newer, "SDK", "Engine_classes.hpp"), "// classes");
  const now = Date.now() / 1000;
  const { utimesSync } = await import("node:fs");
  utimesSync(path.join(older, "SDK.hpp"), now - 3600, now - 3600);
  const dumps = await findDumps(root);
  assert.equal(dumps[0].dir, newer);
  const ws = temp();
  const w = workspacePaths(ws);
  await assert.rejects(importSdk(newer, w.runtimeSource), /runtime source/);
  write(path.join(w.runtimeSource, "dllmain.cpp"), "// runtime");
  assert.ok((await importSdk(newer, w.runtimeSource)) >= 3);
  assert.equal(readFileSync(w.sdkHeader, "utf8"), "// new");
  assert.equal(await countFunctionFiles(w.sdkDir), 1);
});

test("publishRuntime writes both files and a runtime.json the launcher accepts", async () => {
  const w = workspacePaths(temp());
  await assert.rejects(publishRuntime(w), /not built/);
  write(w.runtimeDll, Buffer.alloc(5000, 1));
  write(w.loaderDll, Buffer.alloc(3000, 2));
  const r = await publishRuntime(w, new Date("2026-10-14T12:00:00Z"));
  assert.match(r.version, /^20261014-[0-9a-f]{8}$/);
  const json = JSON.parse(readFileSync(path.join(w.publish, "runtime.json"), "utf8"));
  const m = parseRuntimeManifest(json);
  assert.ok(m, "the content server's manifest is what the launcher parses");
  assert.equal(m.changelist, 647472);
  assert.equal(m.files.find((f) => f.name === "winmm.dll")?.sha256, sha256(Buffer.alloc(3000, 2)));
  assert.ok(existsSync(path.join(w.publish, "MysticParadox.dll")));
});

test("detectHostKit reads a workspace without a toolchain", async () => {
  const root = temp();
  write(path.join(root, "legacy", "ReforgedHost", "package.json"), "{}");
  const w = workspacePaths(root);
  write(path.join(w.runtimeSource, "dllmain.cpp"), "//");
  const kit = await detectHostKit(root, { env: {}, platform: "linux", toolchain: { visualStudio: null, majorVersion: null, msbuild: null, cargo: null }, dumpRoot: path.join(root, "no-dumps") });
  assert.equal(kit.workspaceOk, true);
  assert.deepEqual(kit.upstream, { present: true, patched: true });
  assert.deepEqual(kit.sdk, { present: false, functionFiles: 0 });
  assert.equal(kit.builds.published, false);
  assert.deepEqual(kit.toolchain, { visualStudio: null, msbuild: false, cargo: false });
});

test("build settings: v145 for Visual Studio 2026, v143 otherwise; Release x64 with the static CRT", () => {
  assert.equal(platformToolset(18), "v145");
  assert.equal(platformToolset(17), "v143");
  assert.equal(platformToolset(null), "v143");
  const args = msbuildArgs("C:\\x\\MysticParadox.sln", "v143");
  assert.ok(args.includes("/p:Configuration=Release") && args.includes("/p:Platform=x64") && args.includes("/p:PlatformToolset=v143"));
});
