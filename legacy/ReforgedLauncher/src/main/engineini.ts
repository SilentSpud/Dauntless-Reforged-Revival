// Rewrites the game's user config before each launch, as Dauntless Revived's launcher does:
//
//  - Engine.ini: the launcher owns two sections and replaces them on every launch; every other
//    section of the file is kept as it was.
//      [OnlineSubsystemMcp.XMPP]  chat and presence pointed at the launcher's relay
//                                 (ws://127.0.0.1:<relay port>, no TLS on this PC) instead of Epic's
//                                 old server, which would otherwise receive the account id and token.
//                                 The runtime answers the same keys itself; this is the fallback.
//      [SystemSettings]           the forced graphics level, only when one is chosen.
//  - GameUserSettings.ini: when a graphics level is forced, its sg.*Quality lines are set to match,
//    and an impossible saved window size is repaired (display-repair.ts).
//  - Game.ini (Dauntless 1.14.7 only): Dauntless Revived's [OnlineSubsystemPhoenix] endpoints, which
//    point the game at 127.0.0.1, are removed (see PHOENIX_ENDPOINTS_SECTION).

import { repairDisplaySettings } from "./display-repair";
import { promises as fsp } from "node:fs";
import path from "node:path";
import { isValidHost } from "../shared/invite";
import type { GraphicsPreset } from "../shared/types";

export const SCALABILITY_GROUPS = ["ViewDistance", "AntiAliasing", "Shadow", "PostProcess", "Texture", "Effects", "Foliage", "Shading"];

export function defaultConfigDir(env: NodeJS.ProcessEnv = process.env): string {
  const base = env.LOCALAPPDATA ?? path.join(env.USERPROFILE ?? "C:\\", "AppData", "Local");
  return path.join(base, "Archon", "Saved", "Config", "WindowsClient");
}

export function decodeIni(data: Buffer): string {
  if (data.length >= 2 && data[0] === 0xff && data[1] === 0xfe) return data.subarray(2).toString("utf16le");
  if (data.length >= 3 && data[0] === 0xef && data[1] === 0xbb && data[2] === 0xbf) return data.subarray(3).toString("utf8");
  return data.toString("utf8");
}

// Plain ASCII unless the kept lines contain other characters; then UTF-16 with a byte order mark,
// which Unreal reads correctly, instead of turning them into "?".
export function encodeIni(lines: string[]): Buffer {
  const text = lines.map((l) => l + "\r\n").join("");
  if (/^[\x00-\x7f]*$/.test(text)) return Buffer.from(text, "latin1");
  return Buffer.concat([Buffer.from([0xff, 0xfe]), Buffer.from(text, "utf16le")]);
}

export function splitLines(text: string): string[] {
  if (text.length === 0) return [];
  const lines = text.split(/\r\n|\n|\r/);
  if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

// memory: Dauntless 1.4.4 (Dauntless Revived servers) always gets the streaming and garbage-collection
// lines Dauntless Revived's launcher writes for it.
export function systemSettingsLines(graphics: GraphicsPreset, memory = false): string[] {
  if (graphics < 0 && !memory) return [];
  const sys = ["[SystemSettings]"];
  if (memory) sys.push("r.Streaming.PoolSize=3000", "r.Streaming.LimitPoolSizeToVRAM=1", "gc.TimeBetweenPurgingPendingKillObjects=10", "s.ForceGCAfterLevelStreamedOut=1");
  if (graphics < 0) return sys;
  for (const g of SCALABILITY_GROUPS) sys.push(`sg.${g}Quality=${graphics}`);
  sys.push("sg.ResolutionQuality=100", "r.ScreenPercentage=100");
  return sys;
}

export function xmppLines(host: string, port: number): string[] {
  if (!isValidHost(host)) throw new Error("invalid host");
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error("invalid port");
  return ["[OnlineSubsystemMcp.XMPP]", `ServerAddr="ws://${host}"`, `ServerPort=${port}`, "bUseSSL=false"];
}

const OWNED_SECTIONS = /^\[(SystemSettings|OnlineSubsystemMcp\.XMPP)\]/i;

export function rewriteEngineIniText(existing: string[], host: string, port: number, graphics: GraphicsPreset, memory = false): string[] {
  const keep: string[] = [];
  let skip = false;
  for (const l of existing) {
    if (OWNED_SECTIONS.test(l)) {
      skip = true;
      continue;
    }
    if (skip && /^\[/.test(l)) skip = false;
    if (!skip) keep.push(l);
  }
  // Drop the blank lines a removed section leaves at the top.
  while (keep.length > 0 && keep[0].trim() === "") keep.shift();
  const sys = systemSettingsLines(graphics, memory);
  return [...(sys.length ? [...sys, ""] : []), ...xmppLines(host, port), "", ...keep];
}

// Dauntless Revived's launcher, in its local-server mode, writes every Phoenix service endpoint into
// Game.ini as http://127.0.0.1:61000/... (the [OnlineSubsystemPhoenix] section). Every Dauntless
// version reads the same config folder, so Dauntless 1.14.7 then sends its own login checks
// (CheckCrossPlayProgressionEndpoint, PhoenixStatusMessageEndpoint, ...) to 127.0.0.1, which the
// runtime does not redirect and where nothing answers: the login stops at
// CheckCrossPlayAndCrossProgression ("An error occurred while communicating with the game servers").
// 1.14.7 needs none of these keys (the runtime redirects the game's own Steelyard addresses), so the
// section is removed before a 1.14.7 launch. Revived's launcher writes it again before its own.
export const PHOENIX_ENDPOINTS_SECTION = "OnlineSubsystemPhoenix";

// The lines without one section (its header and everything up to the next header), and how many
// keys it held.
export function removeIniSection(lines: string[], section: string): { lines: string[]; removed: number } {
  const out: string[] = [];
  let skip = false;
  let removed = 0;
  for (const l of lines) {
    const header = /^\s*\[([^\]]*)\]/.exec(l);
    if (header) skip = header[1].trim().toLowerCase() === section.toLowerCase();
    if (!skip) out.push(l);
    else if (!header && /^\s*[^\s;#][^=]*=/.test(l)) removed++;
  }
  return { lines: out, removed };
}

export function rewriteGameUserSettingsText(lines: string[], graphics: GraphicsPreset): string[] {
  if (graphics < 0) return lines;
  return lines.map((l) => {
    for (const k of SCALABILITY_GROUPS) {
      if (new RegExp(`^sg\\.${k}Quality=`, "i").test(l)) return `sg.${k}Quality=${graphics}`;
    }
    return l;
  });
}

async function readLines(file: string): Promise<string[] | null> {
  try {
    return splitLines(decodeIni(await fsp.readFile(file)));
  } catch (e) {
    if ((e as NodeJS.ErrnoException).code === "ENOENT") return null;
    throw e;
  }
}

async function writeAtomically(file: string, data: Buffer): Promise<void> {
  const tmp = `${file}.rr-tmp`;
  await fsp.writeFile(tmp, data);
  await fsp.rename(tmp, file);
}

export interface ApplyConfigOptions {
  host: string; // where chat/presence (XMPP) connects: the relay (1.4.4 private mode: the host itself)
  port: number;
  memory?: boolean; // Dauntless 1.4.4: its memory lines in [SystemSettings]
  graphics: GraphicsPreset;
  configDir?: string;
  safeWindow?: boolean;
  // Dauntless 1.14.7: remove Dauntless Revived's [OnlineSubsystemPhoenix] endpoints from Game.ini.
  clearPhoenixEndpoints?: boolean;
}

export async function applyGameConfig(opts: ApplyConfigOptions): Promise<{ engineIni: string; displayRepaired: boolean; phoenixEndpointsRemoved: number }> {
  const dir = opts.configDir ?? defaultConfigDir();
  await fsp.mkdir(dir, { recursive: true });
  let phoenixEndpointsRemoved = 0;
  if (opts.clearPhoenixEndpoints) {
    const gameIni = path.join(dir, "Game.ini");
    const gameLines = await readLines(gameIni);
    if (gameLines !== null) {
      const cleaned = removeIniSection(gameLines, PHOENIX_ENDPOINTS_SECTION);
      if (cleaned.lines.length !== gameLines.length) {
        await fsp.copyFile(gameIni, `${gameIni}.before-reforged`);
        await writeAtomically(gameIni, encodeIni(cleaned.lines));
        phoenixEndpointsRemoved = cleaned.removed;
      }
    }
  }
  const engine = path.join(dir, "Engine.ini");
  const existing = (await readLines(engine)) ?? [];
  await writeAtomically(engine, encodeIni(rewriteEngineIniText(existing, opts.host, opts.port, opts.graphics, opts.memory === true)));
  const gus = path.join(dir, "GameUserSettings.ini");
  const lines = await readLines(gus);
  let displayRepaired = false;
  if (lines !== null || opts.safeWindow) {
    const display = repairDisplaySettings(lines ?? [], opts.safeWindow);
    displayRepaired = display.repaired;
    if (displayRepaired && lines !== null) await fsp.copyFile(gus, `${gus}.before-display-repair-${Date.now()}`);
    const next = opts.graphics >= 0 ? rewriteGameUserSettingsText(display.lines, opts.graphics) : display.lines;
    if (displayRepaired || opts.graphics >= 0) await writeAtomically(gus, encodeIni(next));
  }
  return { engineIni: engine, displayRepaired, phoenixEndpointsRemoved };
}
