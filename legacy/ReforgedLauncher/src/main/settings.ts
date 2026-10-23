// Launcher settings in userData/settings.json. No secrets in here: the key lives in the KeyStore.

import { promises as fsp, readFileSync } from "node:fs";
import path from "node:path";
import { isPrivateModeHost, isValidFingerprint, isValidHost, isValidInviteCode, isValidShareUrl, cleanServerName, type GameEdition, type ServerMode } from "../shared/invite";
import { isValidUsername } from "../shared/username";
import {
  DEFAULT_GRAPHICS,
  GAME_LANGUAGES,
  GRAPHICS_PRESETS,
  LANGUAGES,
  RUNTIME_PROFILES,
  type GameLanguage,
  type GraphicsPreset,
  type Language,
  type RuntimeProfile,
} from "../shared/types";

export interface StoredServer {
  game: GameEdition; // "revived": a Dauntless Revived server (Dauntless 1.4.4); older settings have none: "reforged"
  mode: ServerMode;
  host: string;
  port: number;
  name: string;
  share: string | null; // private mode: Tailscale share link
  fp: string | null; // public mode: pinned certificate fingerprint
  code: string | null; // invite code not used yet
}

export interface StoredSettings {
  version: 1;
  server: StoredServer | null;
  savedServers: StoredServer[]; // servers joined before, to switch back without the invite
  gameRoot: string | null; // the "Dauntless" folder (the one that holds Archon\)
  revivedRoot: string | null; // Dauntless 1.4.4, for Dauntless Revived servers: the folder that holds Archon\
  revivedVerified: string | null; // the 1.4.4 folder whose every file was last checked against the manifest
  graphics: GraphicsPreset;
  windowed: boolean;
  streamerMode: boolean; // hide addresses, fingerprints, paths and player names on screen
  language: Language;
  gameLanguage: GameLanguage;
  runtimeProfile: RuntimeProfile;
  hostTools: boolean;
  usernames: Record<string, string>; // per server id, for display only
  backupOffered: Record<string, boolean>; // per server id: the one-time backup offer was shown
  trustedRuntimes: Record<string, string[]>; // per server id: the runtime file sets the player accepted
  localRuntimeDir: string | null; // a folder with a runtime the host built (instead of the server's)
  hostWorkspace: string | null; // Host page: a Dauntless Reforged Revival checkout
  dumperDll: string | null; // Host page: the Dumper-7 DLL the host picked
}

export function defaultSettings(language: Language): StoredSettings {
  return {
    version: 1,
    server: null,
    savedServers: [],
    gameRoot: null,
    revivedRoot: null,
    revivedVerified: null,
    graphics: DEFAULT_GRAPHICS,
    windowed: false,
    streamerMode: false,
    language,
    gameLanguage: "auto",
    runtimeProfile: "production",
    hostTools: false,
    usernames: {},
    backupOffered: {},
    trustedRuntimes: {},
    localRuntimeDir: null,
    hostWorkspace: null,
    dumperDll: null,
  };
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

export function validDir(v: unknown, platform: NodeJS.Platform = process.platform, max = 200): string | null {
  if (typeof v !== "string" || v.length <= 3 || v.length >= max || v.includes("\0")) return null;
  if (platform === "win32") return /^[A-Za-z]:[\\/]/.test(v) && path.win32.isAbsolute(v) ? v : null;
  return path.posix.isAbsolute(v) ? v : null;
}

function validDll(v: unknown, platform: NodeJS.Platform): string | null {
  const p = validDir(v, platform, 260);
  return p !== null && /\.dll$/i.test(p) ? p : null;
}

export const MAX_SAVED_SERVERS = 20;
const MAX_TRUSTED_PER_SERVER = 8;

function sanitizeServer(sv: unknown): StoredServer | null {
  if (!isObject(sv)) return null;
  const name = typeof sv.name === "string" ? cleanServerName(sv.name) : null;
  const mode: ServerMode = sv.mode === "public" ? "public" : "private";
  // Private mode is plain HTTP: only into the tailnet (or loopback), as parseInvite enforces.
  const hostOk = typeof sv.host === "string" && isValidHost(sv.host) && (mode === "public" || isPrivateModeHost(sv.host));
  const portOk = Number.isInteger(sv.port) && (sv.port as number) >= 1 && (sv.port as number) <= 65535;
  // A public server without a valid fingerprint is dropped: it could never be reached safely.
  const fpOk = mode === "private" || isValidFingerprint(sv.fp);
  if (!hostOk || !portOk || !fpOk || !name) return null;
  return {
    game: sv.game === "revived" ? "revived" : "reforged",
    mode,
    host: sv.host as string,
    port: sv.port as number,
    name,
    share: mode === "private" && typeof sv.share === "string" && isValidShareUrl(sv.share) ? sv.share : null,
    fp: mode === "public" ? (sv.fp as string) : null,
    code: typeof sv.code === "string" && isValidInviteCode(sv.code) ? sv.code : null,
  };
}

// The same server (and the same account key slot): mode, host and port.
export function sameServer(a: StoredServer, b: StoredServer): boolean {
  return a.mode === b.mode && a.host === b.host && a.port === b.port;
}

export function sanitizeSettings(raw: unknown, language: Language, platform: NodeJS.Platform = process.platform): StoredSettings {
  const s = defaultSettings(language);
  if (!isObject(raw)) return s;
  s.server = sanitizeServer(raw.server);
  if (Array.isArray(raw.savedServers)) {
    for (const entry of raw.savedServers) {
      const sv = sanitizeServer(entry);
      if (sv && !s.savedServers.some((o) => sameServer(o, sv)) && s.savedServers.length < MAX_SAVED_SERVERS) s.savedServers.push(sv);
    }
  }
  // The current server is always in the list, as it is now (a newer invite or name replaces the old entry).
  if (s.server) {
    const current = s.server;
    const i = s.savedServers.findIndex((o) => sameServer(o, current));
    if (i >= 0) s.savedServers[i] = { ...current };
    else s.savedServers.unshift({ ...current });
    s.savedServers = s.savedServers.slice(0, MAX_SAVED_SERVERS);
  }
  s.gameRoot = validDir(raw.gameRoot, platform);
  s.revivedRoot = validDir(raw.revivedRoot, platform);
  s.revivedVerified = validDir(raw.revivedVerified, platform);
  if (GRAPHICS_PRESETS.includes(raw.graphics as GraphicsPreset)) s.graphics = raw.graphics as GraphicsPreset;
  s.windowed = raw.windowed === true;
  s.streamerMode = raw.streamerMode === true;
  s.hostTools = raw.hostTools === true;
  if (LANGUAGES.includes(raw.language as Language)) s.language = raw.language as Language;
  if (GAME_LANGUAGES.includes(raw.gameLanguage as GameLanguage)) s.gameLanguage = raw.gameLanguage as GameLanguage;
  if (RUNTIME_PROFILES.includes(raw.runtimeProfile as RuntimeProfile)) s.runtimeProfile = raw.runtimeProfile as RuntimeProfile;
  if (isObject(raw.usernames)) {
    for (const [k, v] of Object.entries(raw.usernames)) if (/^[0-9a-f]{24}$/.test(k) && isValidUsername(v)) s.usernames[k] = v;
  }
  if (isObject(raw.backupOffered)) {
    for (const [k, v] of Object.entries(raw.backupOffered)) if (/^[0-9a-f]{24}$/.test(k) && v === true) s.backupOffered[k] = true;
  }
  if (isObject(raw.trustedRuntimes)) {
    for (const [k, v] of Object.entries(raw.trustedRuntimes)) {
      if (!/^[0-9a-f]{24}$/.test(k) || !Array.isArray(v)) continue;
      const ids = v.filter((id): id is string => typeof id === "string" && /^[0-9a-f]{64}$/.test(id));
      if (ids.length > 0) s.trustedRuntimes[k] = [...new Set(ids)].slice(-MAX_TRUSTED_PER_SERVER);
    }
  }
  s.localRuntimeDir = validDir(raw.localRuntimeDir, platform, 260);
  s.hostWorkspace = validDir(raw.hostWorkspace, platform, 200);
  s.dumperDll = validDll(raw.dumperDll, platform);
  return s;
}

export class SettingsStore {
  private data: StoredSettings;
  private readonly file: string;

  constructor(dir: string, language: Language, private readonly platform: NodeJS.Platform = process.platform) {
    this.file = path.join(dir, "settings.json");
    let raw: unknown = null;
    try {
      raw = JSON.parse(readFileSync(this.file, "utf8"));
    } catch {
      raw = null;
    }
    this.data = sanitizeSettings(raw, language, this.platform);
  }

  get(): StoredSettings {
    return this.data;
  }

  async update(fn: (s: StoredSettings) => void): Promise<StoredSettings> {
    const copy: StoredSettings = JSON.parse(JSON.stringify(this.data));
    fn(copy);
    this.data = sanitizeSettings(copy, copy.language, this.platform);
    await fsp.mkdir(path.dirname(this.file), { recursive: true });
    const tmp = `${this.file}.tmp`;
    await fsp.writeFile(tmp, JSON.stringify(this.data, null, 2));
    await fsp.rename(tmp, this.file);
    return this.data;
  }
}
