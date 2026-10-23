// The Host page: everything a host does before friends can play that needs this PC's game, found
// from files in their Dauntless Reforged Revival checkout ("the workspace") and done with the
// host's own tools. The game data and the SDK are generated from the host's own 1.14.7 install;
// none of it is ever downloaded or shared (Phoenix Labs' material).
//
// The workspace layout (Mystic Paradox is part of the checkout; the host's own files in it are git-ignored):
//
//   <workspace>\legacy\ReforgedHost\              the host command-line kit
//   <workspace>\native\paradox-runtime\           the game runtime's source
//       SDK.hpp, SDK\                             the Dumper-7 CppSDK, copied in by "Import SDK"
//       x64\Release\MysticParadox.dll
//   <workspace>\native\runtime-loader\target\release\winmm.dll
//   <workspace>\native\catalog-exporter\x64\Release\CatalogExporter.dll
//   <workspace>\paradox\backend\game-data\, paradox\director\game-data\
//   <workspace>\paradox\Items_Analysis\           CatalogExporter's output, copied in from the game folder
//   <workspace>\.workspace\runtime\               what the content server hands to players
//   <workspace>\.workspace\logs\                  the logs of the builds started here
//
// A checkout from before Mystic Paradox was merged in has upstream/mystic-paradox.json instead, and the
// host kit downloads Mystic Paradox into .workspace\Mystic-Paradox (ParadoxRuntime\, tools\RuntimeLoader\,
// tools\CatalogExporter\, ParadoxBackend\, ParadoxDirector\, Items_Analysis\), patched, until its first update.

import { execFile, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { gunzipSync } from "node:zlib";
import { createWriteStream, existsSync, promises as fsp } from "node:fs";
import path from "node:path";
import { hashFile } from "./verify";
import { GAME_CHANGELIST, RUNTIME_FILE_NAMES } from "./constants";
import { MAX_MULTIPLIER, type GameDataState, type HostKit, type HostMultipliers } from "../shared/types";

export const BACKEND_GAME_DATA = [
  "progression_config.json",
  "slayers_path.json",
  "slayers_path_definitions.json",
  "ladyluck_store.json",
  "platinum_store.json",
  "reward_cache_store.json",
  "inventory_storage_policy.json",
];

export const DIRECTOR_GAME_DATA = [
  "player_hunts_table.json",
  "matchmaker_hunts_table.json",
  "arena_easy_matchmaker_hunts.json",
  "arena_hard_matchmaker_hunts.json",
  "arena_elite_matchmaker_hunts.json",
  "arena_hard_matchmaker_hunts_new.json",
  "arena_elite_matchmaker_hunts_new.json",
];

// The exports the services read, and the flags file CatalogExporter reads from the game's working
// directory (see Mystic Paradox's docs/GENERATING_GAME_DATA.md).
export const EXPORT_FLAGS = [
  "# Written by the Dauntless Reforged Revival launcher (Host page). See docs/sdk-and-game-data.md.",
  "EXPORT_CATALOG=1",
  "EXPORT_SLAYERS_PATH=1",
  "EXPORT_HUNTS=1",
  "EXPORT_STORE_RUNTIME=1",
  "EXPORT_PROGRESSION=1", // the tracks the progression config needs (legacy/ReforgedHost/src/progression.ts)
  "EXPORT_COMBAT=0",
  "EXPORT_SKINS=0",
  "EXPORT_SKINS_RESOLVE_STRINGTABLE=0",
  "EXPORT_WEAPON_SLOTS=0",
  "EXPORT_TABLE_INVENTORY=0",
  "EXPORT_CELLS=0",
  "EXPORT_DROP_TABLES=0",
  "EXPORT_CURRENCY_NAMES=0",
  "",
].join("\r\n");

// Dumper-7 writes to C:\Dumper-7\<game and version>\CppSDK unless its ini says otherwise.
export const DUMPER_OUTPUT_ROOT = "C:\\Dumper-7";

export interface WorkspacePaths {
  root: string;
  hostKit: string;
  // Where Mystic Paradox is: paradox/, or .workspace/Mystic-Paradox in a checkout from before it was merged in.
  upstream: string;
  // Only a downloaded Mystic Paradox has one (written once the patches are applied); null when it is in the checkout.
  patchMarker: string | null;
  runtimeSource: string;
  sdkHeader: string;
  sdkDir: string;
  runtimeSolution: string;
  runtimeDll: string;
  loaderDir: string;
  loaderDll: string;
  exporterProject: string;
  exporterDll: string;
  backendGameData: string;
  directorGameData: string;
  exports: string;
  publish: string;
  logs: string;
}

// The host kit's folder: legacy/ReforgedHost, or ReforgedHost in a checkout from before the Node projects
// moved to legacy/, until its first update.
export function hostKitDir(root: string, p: typeof path.win32 | typeof path.posix = path): string {
  const moved = p.join(root, "legacy", "ReforgedHost");
  const old = p.join(root, "ReforgedHost");
  return !existsSync(p.join(moved, "package.json")) && existsSync(p.join(old, "package.json")) ? old : moved;
}

// A checkout from before Mystic Paradox was merged into paradox/ and native/: the host kit downloads it.
export function downloadsParadox(root: string, p: typeof path.win32 | typeof path.posix = path): boolean {
  return !existsSync(p.join(root, "paradox", "backend", "package.json")) && existsSync(p.join(root, "upstream", "mystic-paradox.json"));
}

export function workspacePaths(root: string, p: typeof path.win32 | typeof path.posix = path): WorkspacePaths {
  if (downloadsParadox(root, p)) return downloadedPaths(root, p);
  const paradox = p.join(root, "paradox");
  const native = p.join(root, "native");
  const runtimeSource = p.join(native, "paradox-runtime");
  return {
    root,
    hostKit: p.join(hostKitDir(root, p), "package.json"),
    upstream: paradox,
    patchMarker: null,
    runtimeSource,
    sdkHeader: p.join(runtimeSource, "SDK.hpp"),
    sdkDir: p.join(runtimeSource, "SDK"),
    runtimeSolution: p.join(runtimeSource, "MysticParadox.sln"),
    runtimeDll: p.join(runtimeSource, "x64", "Release", "MysticParadox.dll"),
    loaderDir: p.join(native, "runtime-loader"),
    loaderDll: p.join(native, "runtime-loader", "target", "release", "winmm.dll"),
    exporterProject: p.join(native, "catalog-exporter", "CatalogExporter.vcxproj"),
    exporterDll: p.join(native, "catalog-exporter", "x64", "Release", "CatalogExporter.dll"),
    backendGameData: p.join(paradox, "backend", "game-data"),
    directorGameData: p.join(paradox, "director", "game-data"),
    exports: p.join(paradox, "Items_Analysis"),
    publish: p.join(root, ".workspace", "runtime"),
    logs: p.join(root, ".workspace", "logs"),
  };
}

function downloadedPaths(root: string, p: typeof path.win32 | typeof path.posix): WorkspacePaths {
  const upstream = p.join(root, ".workspace", "Mystic-Paradox");
  const runtimeSource = p.join(upstream, "ParadoxRuntime");
  return {
    root,
    hostKit: p.join(hostKitDir(root, p), "package.json"),
    upstream,
    patchMarker: p.join(upstream, ".reforged-patches.json"),
    runtimeSource,
    sdkHeader: p.join(runtimeSource, "SDK.hpp"),
    sdkDir: p.join(runtimeSource, "SDK"),
    runtimeSolution: p.join(runtimeSource, "MysticParadox.sln"),
    runtimeDll: p.join(runtimeSource, "x64", "Release", "MysticParadox.dll"),
    loaderDir: p.join(upstream, "tools", "RuntimeLoader"),
    loaderDll: p.join(upstream, "tools", "RuntimeLoader", "target", "release", "winmm.dll"),
    exporterProject: p.join(upstream, "tools", "CatalogExporter", "CatalogExporter.vcxproj"),
    exporterDll: p.join(upstream, "tools", "CatalogExporter", "x64", "Release", "CatalogExporter.dll"),
    backendGameData: p.join(upstream, "ParadoxBackend", "game-data"),
    directorGameData: p.join(upstream, "ParadoxDirector", "game-data"),
    exports: p.join(upstream, "Items_Analysis"),
    publish: p.join(root, ".workspace", "runtime"),
    logs: p.join(root, ".workspace", "logs"),
  };
}

async function isFile(file: string): Promise<boolean> {
  try {
    return (await fsp.stat(file)).isFile();
  } catch {
    return false;
  }
}

async function isDir(dir: string): Promise<boolean> {
  try {
    return (await fsp.stat(dir)).isDirectory();
  } catch {
    return false;
  }
}

export async function countFunctionFiles(sdkDir: string): Promise<number> {
  try {
    return (await fsp.readdir(sdkDir)).filter((n) => n.endsWith("_functions.cpp")).length;
  } catch {
    return 0;
  }
}

// "real" when every file exists and none is byte for byte its shipped *.example.json, "synthetic"
// when an example is in use, "missing" otherwise (the same rule as the backend's status).
export async function detectGameData(dir: string, files: readonly string[]): Promise<GameDataState> {
  let synthetic = false;
  for (const name of files) {
    let data: Buffer;
    try {
      data = await fsp.readFile(path.join(dir, name));
    } catch {
      return "missing";
    }
    try {
      if ((await fsp.readFile(path.join(dir, name.replace(/\.json$/, ".example.json")))).equals(data)) synthetic = true;
    } catch {
      /* no example */
    }
  }
  return synthetic ? "synthetic" : "real";
}

// Dumper-7 outputs: every <root>\<name>\CppSDK that holds an SDK.hpp, newest first.
export async function findDumps(root: string = DUMPER_OUTPUT_ROOT): Promise<{ dir: string; name: string; modified: string }[]> {
  let names: string[];
  try {
    names = (await fsp.readdir(root)).slice(0, 200);
  } catch {
    return [];
  }
  const out: { dir: string; name: string; modified: string; t: number }[] = [];
  for (const name of names) {
    const dir = path.join(root, name, "CppSDK");
    try {
      const st = await fsp.stat(path.join(dir, "SDK.hpp"));
      out.push({ dir, name, modified: st.mtime.toISOString(), t: st.mtimeMs });
    } catch {
      /* not a dump */
    }
  }
  out.sort((a, b) => b.t - a.t);
  return out.map(({ dir, name, modified }) => ({ dir, name, modified }));
}

export interface Toolchain {
  visualStudio: string | null;
  majorVersion: number | null;
  msbuild: string | null;
  cargo: string | null;
}

export type ExecFn = (file: string, args: string[]) => Promise<string | null>;

export const execText: ExecFn = (file, args) =>
  new Promise((resolve) => {
    execFile(file, args, { windowsHide: true, timeout: 20000, maxBuffer: 4 * 1024 * 1024 }, (err, out) => resolve(err ? null : String(out)));
  });

// Visual Studio with the C++ workload and its MSBuild, through vswhere (as Mystic Paradox's
// _build.bat does), and Rust's cargo for the loader.
export async function findToolchain(env: NodeJS.ProcessEnv, platform: NodeJS.Platform = process.platform, exec: ExecFn = execText): Promise<Toolchain> {
  const none: Toolchain = { visualStudio: null, majorVersion: null, msbuild: null, cargo: null };
  if (platform !== "win32") return none;
  const vswhere = path.win32.join(env["ProgramFiles(x86)"] ?? "C:\\Program Files (x86)", "Microsoft Visual Studio", "Installer", "vswhere.exe");
  let visualStudio: string | null = null;
  let majorVersion: number | null = null;
  let msbuild: string | null = null;
  if (existsSync(vswhere)) {
    const json = await exec(vswhere, ["-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-format", "json", "-utf8"]);
    try {
      const list = JSON.parse(json ?? "[]") as { displayName?: unknown; installationVersion?: unknown }[];
      const vs = list[0];
      if (vs && typeof vs.displayName === "string") {
        const version = typeof vs.installationVersion === "string" ? vs.installationVersion : "";
        visualStudio = `${vs.displayName}${version ? ` (${version.split(".").slice(0, 2).join(".")})` : ""}`.slice(0, 80);
        const major = Number(version.split(".")[0]);
        majorVersion = Number.isInteger(major) ? major : null;
      }
    } catch {
      /* no answer */
    }
    const found = await exec(vswhere, ["-latest", "-products", "*", "-requires", "Microsoft.Component.MSBuild", "-find", "MSBuild\\**\\Bin\\MSBuild.exe"]);
    const first = found?.split(/\r?\n/).map((l) => l.trim()).find((l) => /msbuild\.exe$/i.test(l));
    msbuild = first ?? null;
  }
  const cargoCandidates = [env.USERPROFILE ? path.win32.join(env.USERPROFILE, ".cargo", "bin", "cargo.exe") : null, env.CARGO_HOME ? path.win32.join(env.CARGO_HOME, "bin", "cargo.exe") : null].filter((c): c is string => c !== null);
  const cargo = cargoCandidates.find((c) => existsSync(c)) ?? null;
  return { visualStudio, majorVersion, msbuild, cargo };
}

// Mystic Paradox's projects select toolset v145 (Visual Studio 2026); Visual Studio 2022 (17.x)
// builds them with v143, which its build guide asks for.
export function platformToolset(majorVersion: number | null): string {
  return majorVersion !== null && majorVersion >= 18 ? "v145" : "v143";
}

export function msbuildArgs(project: string, toolset: string): string[] {
  return [project, "/m", "/p:Configuration=Release", "/p:Platform=x64", `/p:PlatformToolset=${toolset}`, "/nologo", "/verbosity:minimal"];
}

// The server's host.json (written by npm run setup): what the Host page shows of it. It holds no
// secrets (those are in secrets.env beside it).
// The host kit writes them only for a private server, and leaves them out when both are 1 (off).
function readMultipliers(v: unknown): HostMultipliers | null {
  if (typeof v !== "object" || v === null) return null;
  const { xp, rewards } = v as { xp?: unknown; rewards?: unknown };
  const ok = (n: unknown): n is number => typeof n === "number" && Number.isFinite(n) && n >= 1 && n <= MAX_MULTIPLIER;
  if (!ok(xp) || !ok(rewards) || (xp === 1 && rewards === 1)) return null;
  return { xp, rewards };
}

export async function readServerConfig(root: string): Promise<HostKit["serverConfig"]> {
  const none: HostKit["serverConfig"] = { configured: false, name: null, mode: null, host: null, multipliers: null };
  try {
    const c = JSON.parse(await fsp.readFile(path.join(root, ".workspace", "config", "host.json"), "utf8")) as { name?: unknown; mode?: unknown; host?: unknown; multipliers?: unknown };
    const mode = c.mode === "private" || c.mode === "public" ? c.mode : null;
    if (!mode) return none;
    return { configured: true, name: typeof c.name === "string" ? c.name.slice(0, 80) : null, mode, host: typeof c.host === "string" ? c.host.slice(0, 255) : null, multipliers: mode === "private" ? readMultipliers(c.multipliers) : null };
  } catch {
    return none;
  }
}

export async function detectHostKit(root: string | null, opts: { env: NodeJS.ProcessEnv; platform?: NodeJS.Platform; toolchain?: Toolchain; dumpRoot?: string }): Promise<Omit<HostKit, "job" | "dumperDll" | "requirements" | "update" | "projectTarget">> {
  const toolchain = opts.toolchain ?? (await findToolchain(opts.env, opts.platform));
  const dumps = (opts.platform ?? process.platform) === "win32" || opts.dumpRoot ? await findDumps(opts.dumpRoot) : [];
  const base: Omit<HostKit, "job" | "dumperDll" | "requirements" | "update" | "projectTarget"> = {
    workspace: root,
    workspaceOk: false,
    upstream: { present: false, patched: false },
    sdk: { present: false, functionFiles: 0 },
    dumps,
    builds: { runtime: false, loader: false, exporter: false, published: false },
    toolchain: { visualStudio: toolchain.visualStudio, msbuild: toolchain.msbuild !== null, cargo: toolchain.cargo !== null },
    gameData: { backend: "missing", director: "missing" },
    exports: { present: false, inGame: false },
    serverConfig: { configured: false, name: null, mode: null, host: null, multipliers: null },
  };
  if (root === null) return base;
  const w = workspacePaths(root);
  base.workspaceOk = await isFile(w.hostKit);
  base.upstream.present = await isFile(path.join(w.runtimeSource, "dllmain.cpp"));
  base.upstream.patched = base.upstream.present && (w.patchMarker === null || (await isFile(w.patchMarker)));
  base.sdk.present = await isFile(w.sdkHeader);
  base.sdk.functionFiles = await countFunctionFiles(w.sdkDir);
  base.builds.runtime = await isFile(w.runtimeDll);
  base.builds.loader = await isFile(w.loaderDll);
  base.builds.exporter = await isFile(w.exporterDll);
  base.builds.published = (await Promise.all(RUNTIME_FILE_NAMES.map((n) => isFile(path.join(w.publish, n))))).every(Boolean);
  base.gameData.backend = await detectGameData(w.backendGameData, BACKEND_GAME_DATA);
  base.gameData.director = await detectGameData(w.directorGameData, DIRECTOR_GAME_DATA);
  base.exports.present = await isDir(w.exports);
  base.serverConfig = await readServerConfig(root);
  return base;
}

// ------------------------------------------------------------------ actions

// Copies a folder tree (files and folders only; links are skipped).
export async function copyTree(from: string, to: string, onFile?: (file: string) => void): Promise<number> {
  let count = 0;
  await fsp.mkdir(to, { recursive: true });
  for (const entry of await fsp.readdir(from, { withFileTypes: true })) {
    const a = path.join(from, entry.name);
    const b = path.join(to, entry.name);
    if (entry.isDirectory()) count += await copyTree(a, b, onFile);
    else if (entry.isFile()) {
      await fsp.copyFile(a, b);
      onFile?.(b);
      count++;
    }
  }
  return count;
}

// Copies a Dumper-7 CppSDK into the runtime source, in the layout Mystic Paradox's build expects
// (ParadoxRuntime\SDK.hpp, ParadoxRuntime\SDK\..., and the .inl/.hpp files beside SDK.hpp).
export async function importSdk(dumpDir: string, runtimeSource: string): Promise<number> {
  if (!(await isFile(path.join(dumpDir, "SDK.hpp")))) throw new Error("the folder has no SDK.hpp");
  if (!(await isFile(path.join(runtimeSource, "dllmain.cpp")))) throw new Error("the runtime source is missing");
  const copied = await copyTree(dumpDir, runtimeSource);
  if ((await countFunctionFiles(path.join(runtimeSource, "SDK"))) === 0) throw new Error("the SDK has no *_functions.cpp files");
  return copied;
}

// The shipped examples as the game data, only where a file is missing: enough to start the services
// and test the setup, never to play. Real data is never overwritten.
// A game data pack (ReforgedHost's `npm run gamedata -- --export`, src/gamepack.ts there): gzip-compressed
// JSON { format, version, game, created, files: [{ path, sha256, data }] } holding the game data that host
// generated once. Only the known game-data files can come out of it. It is the host's own data from
// their own game, for their own servers.
export const GAMEDATA_PACK_FORMAT = "dauntless-reforged-revival-gamedata";
export const GAMEDATA_PACK_GAME = `1.14.7 (${GAME_CHANGELIST})`;
const SUPPLIED_SOURCE_FILES = ["progression_config.source.json", "ladyluck_store.source.json"];

export function gameDataPackEntries(w: WorkspacePaths): Map<string, string> {
  const entries = new Map<string, string>();
  for (const name of BACKEND_GAME_DATA) entries.set(`backend/${name}`, path.join(w.backendGameData, name));
  for (const name of DIRECTOR_GAME_DATA) entries.set(`director/${name}`, path.join(w.directorGameData, name));
  for (const name of SUPPLIED_SOURCE_FILES) entries.set(`backend/raw/${name}`, path.join(w.backendGameData, "raw", name));
  return entries;
}

// Checks the whole pack before writing anything; a file that is there and different is kept as <name>.<time>.bak.
export async function importGameDataPack(w: WorkspacePaths, file: string, now = new Date()): Promise<{ written: number; unchanged: number }> {
  let pack: { format?: unknown; version?: unknown; game?: unknown; files?: unknown };
  try {
    pack = JSON.parse(gunzipSync(await fsp.readFile(file)).toString("utf8"));
  } catch {
    throw new Error("pack_invalid");
  }
  if (pack?.format !== GAMEDATA_PACK_FORMAT || pack.version !== 1 || !Array.isArray(pack.files)) throw new Error("pack_invalid");
  if (pack.game !== GAMEDATA_PACK_GAME) throw new Error("pack_game");
  const entries = gameDataPackEntries(w);
  const files = pack.files as { path?: unknown; sha256?: unknown; data?: unknown }[];
  for (const f of files) {
    if (typeof f?.path !== "string" || typeof f.data !== "string" || !entries.has(f.path)) throw new Error("pack_invalid");
    if (createHash("sha256").update(f.data, "utf8").digest("hex") !== f.sha256) throw new Error("pack_damaged");
    try {
      JSON.parse(f.data);
    } catch {
      throw new Error("pack_damaged");
    }
  }
  const stamp = now.toISOString().replace(/[:.]/g, "-");
  let written = 0;
  let unchanged = 0;
  for (const f of files) {
    const target = entries.get(f.path as string)!;
    if (await isFile(target)) {
      if ((await fsp.readFile(target, "utf8")) === f.data) {
        unchanged++;
        continue;
      }
      await fsp.rename(target, `${target}.${stamp}.bak`);
    }
    await fsp.mkdir(path.dirname(target), { recursive: true });
    await fsp.writeFile(target, f.data as string);
    written++;
  }
  return { written, unchanged };
}

export async function useSyntheticData(dir: string, files: readonly string[]): Promise<number> {
  let copied = 0;
  for (const name of files) {
    const target = path.join(dir, name);
    if (await isFile(target)) continue;
    const example = path.join(dir, name.replace(/\.json$/, ".example.json"));
    if (!(await isFile(example))) continue;
    await fsp.copyFile(example, target);
    copied++;
  }
  return copied;
}

export interface PublishedRuntime {
  version: string;
  build: { changelist: number };
  files: { name: string; size: number; sha256: string }[];
}

// Puts the built runtime where the content server hands it out, with runtime.json naming each file
// and its hash (the content server checks them again when it starts).
export async function publishRuntime(w: WorkspacePaths, now: Date = new Date()): Promise<PublishedRuntime> {
  const sources: Record<string, string> = { "MysticParadox.dll": w.runtimeDll, "winmm.dll": w.loaderDll };
  await fsp.mkdir(w.publish, { recursive: true });
  const files: PublishedRuntime["files"] = [];
  for (const name of RUNTIME_FILE_NAMES) {
    const source = sources[name];
    if (!(await isFile(source))) throw new Error(`${name} is not built yet`);
    const target = path.join(w.publish, name);
    await fsp.copyFile(source, `${target}.tmp`);
    await fsp.rename(`${target}.tmp`, target);
    const st = await fsp.stat(target);
    files.push({ name, size: st.size, sha256: await hashFile(target) });
  }
  const id = createHash("sha256").update(files.map((f) => f.sha256).join(":")).digest("hex").slice(0, 8);
  const stamp = now.toISOString().slice(0, 10).replace(/-/g, "");
  const runtime: PublishedRuntime = { version: `${stamp}-${id}`, build: { changelist: GAME_CHANGELIST }, files };
  await fsp.writeFile(path.join(w.publish, "runtime.json"), JSON.stringify(runtime, null, 2));
  return runtime;
}

// Runs a build tool, its output into a log file. Resolves with the exit code and the log's last line.
export function runLogged(command: string, args: string[], cwd: string, logFile: string): Promise<{ code: number; lastLine: string | null }> {
  return new Promise((resolve) => {
    void fsp.mkdir(path.dirname(logFile), { recursive: true }).then(() => {
      const log = createWriteStream(logFile, { flags: "w" });
      log.write(`> ${command} ${args.join(" ")}\r\n(in ${cwd})\r\n\r\n`);
      let tail = "";
      const remember = (chunk: Buffer) => {
        tail = (tail + chunk.toString("utf8")).slice(-4000);
      };
      let child;
      try {
        child = spawn(command, args, { cwd, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
      } catch (e) {
        log.end(`\r\nfailed to start: ${(e as Error).message}\r\n`);
        return resolve({ code: -1, lastLine: (e as Error).message });
      }
      child.stdout.on("data", (c: Buffer) => {
        log.write(c);
        remember(c);
      });
      child.stderr.on("data", (c: Buffer) => {
        log.write(c);
        remember(c);
      });
      child.on("error", (e) => {
        log.end(`\r\nfailed to start: ${e.message}\r\n`);
        resolve({ code: -1, lastLine: e.message });
      });
      child.on("close", (code) => {
        log.end(`\r\nexit code ${code ?? "none"}\r\n`);
        const lines = tail.split(/\r?\n/).map((l) => l.trim()).filter(Boolean);
        resolve({ code: code ?? -1, lastLine: lines.length ? lines[lines.length - 1].slice(0, 300) : null });
      });
    });
  });
}
