// Finding the player's own Dauntless 1.14.7 install. The launcher never downloads the game: it
// uses the copy the player installed from Epic (with Legendary and the 1.14.7 manifest, see
// docs/players.md), wherever it is.
//
// A pasted or picked folder is tried in these layouts only; nothing is searched:
//
//   <folder>                    the game root, the "Dauntless" folder that holds Archon\
//   <folder>\Dauntless          the folder Legendary was given as --base-path
//   the folder above <folder>   when <folder> is Archon
//   three folders up            when <folder> is Archon\Binaries\Win64 (or the exe itself)
//
// Only drive-letter paths are accepted on Windows, and that is checked before any file-system call:
// looking at a UNC path (\\host\share), a device path (\\?\, \\.\) or a root-relative one (\Games)
// would make Windows contact another machine, possibly with the player's network credentials.
//
// Installs are also found by asking the stores: the Epic Games Launcher's manifests, Legendary's
// installed.json and Heroic's copy of it. Those name the install folder and, for Legendary and
// Heroic, the installed version.

import { promises as fsp } from "node:fs";
import path from "node:path";
import { EPIC_APP_NAME, EXE_NAME, EXE_RELATIVE_PATH, GAME_CHANGELIST, GAME_VERSION_TEXT, VERSION_FILE } from "./constants";
import type { DetectedGame, GameSource } from "../shared/types";

export const MAX_GAME_INPUT = 260;
export const MAX_GAME_ROOT = 180;

export type GameFolderResult = { ok: true; root: string } | { ok: false; reason: "invalid_path" | "not_found" | "too_long" };

function pathsFor(platform: NodeJS.Platform): typeof path.win32 | typeof path.posix {
  return platform === "win32" ? path.win32 : path.posix;
}

// The folder as a clean absolute path, or null. Explorer's "Copy as path" adds quotes.
export function cleanGameFolderInput(input: string, platform: NodeJS.Platform = process.platform): string | null {
  let s = typeof input === "string" ? input.trim() : "";
  if (s.length >= 2 && s.startsWith('"') && s.endsWith('"')) s = s.slice(1, -1).trim();
  if (s.length === 0 || s.length > MAX_GAME_INPUT || s.includes("\0")) return null;
  if (platform === "win32") {
    if (!/^[A-Za-z]:[\\/]/.test(s)) return null;
    return path.win32.resolve(s);
  }
  if (!path.posix.isAbsolute(s)) return null;
  return path.posix.resolve(s);
}

export function gameRootCandidates(folder: string, platform: NodeJS.Platform = process.platform): string[] {
  const p = pathsFor(platform);
  let base = folder;
  // The executable itself was picked or pasted.
  if (p.basename(base).toLowerCase() === EXE_NAME.toLowerCase()) base = p.dirname(base);
  const out = [base, p.join(base, "Dauntless")];
  const name = p.basename(base).toLowerCase();
  if (name === "archon") out.push(p.dirname(base));
  if (name === "win64") out.push(p.resolve(base, "..", "..", ".."));
  return [...new Set(out)];
}

export async function hasGameExe(root: string, platform: NodeJS.Platform = process.platform): Promise<boolean> {
  const p = pathsFor(platform);
  try {
    return (await fsp.stat(p.join(root, ...EXE_RELATIVE_PATH.split("/")))).isFile();
  } catch {
    return false;
  }
}

export async function locateExistingGame(input: string, platform: NodeJS.Platform = process.platform): Promise<GameFolderResult> {
  const folder = cleanGameFolderInput(input, platform);
  if (folder === null) return { ok: false, reason: "invalid_path" };
  let tooLong = false;
  for (const root of gameRootCandidates(folder, platform)) {
    if (!(await hasGameExe(root, platform))) continue;
    if (root.length <= MAX_GAME_ROOT) return { ok: true, root };
    tooLong = true;
  }
  return { ok: false, reason: tooLong ? "too_long" : "not_found" };
}

// ------------------------------------------------------------------ Version.txt

export interface VersionInfo {
  text: string | null;
  ok: boolean; // names 1.14.7 and changelist 647472
}

export function isSupportedVersionText(text: string): boolean {
  if (text === GAME_VERSION_TEXT) return true;
  return /rel-1\.14\.7/i.test(text) && new RegExp(`(^|[^0-9])${GAME_CHANGELIST}([^0-9]|$)`).test(text);
}

export async function readGameVersion(root: string, platform: NodeJS.Platform = process.platform): Promise<VersionInfo> {
  const p = pathsFor(platform);
  try {
    const handle = await fsp.open(p.join(root, VERSION_FILE), "r");
    try {
      const buf = Buffer.alloc(256);
      const { bytesRead } = await handle.read(buf, 0, buf.length, 0);
      const text = buf.subarray(0, bytesRead).toString("utf8").replace(/^\uFEFF/, "").trim().split(/\r?\n/)[0]?.trim() ?? "";
      const clean = /^[\x20-\x7e]{1,128}$/.test(text) ? text : null;
      return { text: clean, ok: clean !== null && isSupportedVersionText(clean) };
    } finally {
      await handle.close();
    }
  } catch {
    return { text: null, ok: false };
  }
}

// ------------------------------------------------------------------ store detection

export interface StoreEnv {
  ProgramData?: string;
  USERPROFILE?: string;
  APPDATA?: string;
  LEGENDARY_CONFIG_PATH?: string;
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

async function readJson(file: string, maxBytes = 4 * 1024 * 1024): Promise<unknown> {
  try {
    const st = await fsp.stat(file);
    if (!st.isFile() || st.size > maxBytes) return null;
    return JSON.parse((await fsp.readFile(file, "utf8")).replace(/^\uFEFF/, ""));
  } catch {
    return null;
  }
}

// Legendary's installed.json (and Heroic's copy of it): {"Jackal": {"install_path", "version", ...}}.
export function parseLegendaryInstalled(raw: unknown, source: GameSource): DetectedGame[] {
  if (!isObject(raw)) return [];
  const out: DetectedGame[] = [];
  for (const [key, entry] of Object.entries(raw)) {
    if (!isObject(entry)) continue;
    const name = typeof entry.app_name === "string" ? entry.app_name : key;
    if (name !== EPIC_APP_NAME) continue;
    if (typeof entry.install_path !== "string" || entry.install_path.length === 0 || entry.install_path.length > MAX_GAME_INPUT) continue;
    out.push({ root: entry.install_path, source, version: typeof entry.version === "string" ? entry.version.slice(0, 64) : null });
  }
  return out;
}

// One Epic Games Launcher manifest (<ProgramData>\Epic\EpicGamesLauncher\Data\Manifests\*.item).
export function parseEpicItem(raw: unknown): DetectedGame | null {
  if (!isObject(raw)) return null;
  const isDauntless = raw.AppName === EPIC_APP_NAME || raw.MainGameAppName === EPIC_APP_NAME || raw.DisplayName === "Dauntless";
  if (!isDauntless || typeof raw.InstallLocation !== "string" || raw.InstallLocation.length === 0 || raw.InstallLocation.length > MAX_GAME_INPUT) return null;
  return { root: raw.InstallLocation, source: "epic", version: typeof raw.AppVersionString === "string" ? raw.AppVersionString.slice(0, 64) : null };
}

export async function detectStoreInstalls(env: StoreEnv, platform: NodeJS.Platform = process.platform): Promise<DetectedGame[]> {
  if (platform !== "win32") return [];
  const p = path.win32;
  const found: DetectedGame[] = [];
  const legendaryDirs = new Set<string>();
  if (env.LEGENDARY_CONFIG_PATH) legendaryDirs.add(env.LEGENDARY_CONFIG_PATH);
  if (env.USERPROFILE) legendaryDirs.add(p.join(env.USERPROFILE, ".config", "legendary"));
  for (const dir of legendaryDirs) found.push(...parseLegendaryInstalled(await readJson(p.join(dir, "installed.json")), "legendary"));
  if (env.APPDATA) found.push(...parseLegendaryInstalled(await readJson(p.join(env.APPDATA, "heroic", "legendaryConfig", "legendary", "installed.json")), "heroic"));
  if (env.ProgramData) {
    const manifests = p.join(env.ProgramData, "Epic", "EpicGamesLauncher", "Data", "Manifests");
    let names: string[] = [];
    try {
      names = (await fsp.readdir(manifests)).filter((n) => n.toLowerCase().endsWith(".item")).slice(0, 500);
    } catch {
      names = [];
    }
    for (const name of names) {
      const item = parseEpicItem(await readJson(p.join(manifests, name), 1024 * 1024));
      if (item) found.push(item);
    }
  }
  // Keep only installs that are really there (the manifest of an uninstalled game can linger), and
  // name each folder once.
  const out: DetectedGame[] = [];
  for (const d of found) {
    const located = await locateExistingGame(d.root, platform);
    if (!located.ok || out.some((o) => o.root.toLowerCase() === located.root.toLowerCase())) continue;
    const version = await readGameVersion(located.root, platform);
    out.push({ root: located.root, source: d.source, version: d.version ?? version.text });
  }
  return out;
}
