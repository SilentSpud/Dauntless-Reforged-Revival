// Argument checks for every IPC call from the renderer. Anything unexpected is refused before it
// reaches the controller.

import {
  GAME_LANGUAGES,
  GRAPHICS_PRESETS,
  LANGUAGES,
  MAX_MULTIPLIER,
  RUNTIME_PROFILES,
  type ExternalTarget,
  type HostSetupRequest,
  type GameLanguage,
  type GraphicsPreset,
  type HostAction,
  type HostFolder,
  type HostMultipliers,
  type Language,
  type RuntimeProfile,
  type Settings,
} from "../shared/types";

export function boundedString(v: unknown, max: number): string | null {
  return typeof v === "string" && v.length <= max ? v : null;
}

export function settingsPatch(v: unknown): Partial<Settings> | null {
  if (typeof v !== "object" || v === null || Array.isArray(v)) return null;
  const out: Partial<Settings> = {};
  for (const [k, val] of Object.entries(v as Record<string, unknown>)) {
    if (k === "graphics" && GRAPHICS_PRESETS.includes(val as GraphicsPreset)) out.graphics = val as GraphicsPreset;
    else if (k === "windowed" && typeof val === "boolean") out.windowed = val;
    else if (k === "streamerMode" && typeof val === "boolean") out.streamerMode = val;
    else if (k === "hostTools" && typeof val === "boolean") out.hostTools = val;
    else if (k === "language" && LANGUAGES.includes(val as Language)) out.language = val as Language;
    else if (k === "gameLanguage" && GAME_LANGUAGES.includes(val as GameLanguage)) out.gameLanguage = val as GameLanguage;
    else if (k === "runtimeProfile" && RUNTIME_PROFILES.includes(val as RuntimeProfile)) out.runtimeProfile = val as RuntimeProfile;
    else return null;
  }
  return out;
}

// Every link name the page may send (a Record, so the compiler catches a name missing here). Only
// these exact strings pass: never a URL.
const TARGETS: Readonly<Record<ExternalTarget, true>> = {
  tailscale_download: true,
  tailscale_share: true,
  vc_redist: true,
  server_source: true,
  project_source: true,
  project_license: true,
  project_issues: true,
  project_docs_players: true,
  project_docs_hosting: true,
  project_docs_sdk: true,
  installing_guide: true,
  legendary: true,
  dumper7: true,
  visual_studio: true,
  rust: true,
  nodejs: true,
  mongodb: true,
  git: true,
  visual_studio_installer: true,
  rust_installer: true,
  dumper7_zip: true,
  revived_source: true,
  mystic_source: true,
  undaunted_source: true,
};

function pick<T extends string>(table: Readonly<Record<T, true>>, v: unknown): T | null {
  return typeof v === "string" && Object.prototype.hasOwnProperty.call(table, v) ? (v as T) : null;
}

export function externalTarget(v: unknown): ExternalTarget | null {
  return pick(TARGETS, v);
}

const HOST_ACTIONS: Readonly<Record<HostAction, true>> = {
  inject_dumper: true,
  import_sdk: true,
  build_runtime: true,
  build_loader: true,
  build_exporter: true,
  inject_exporter: true,
  copy_exports: true,
  synthetic_data: true,
  publish_runtime: true,
  generate_gamedata: true,
  export_gamedata: true,
};

export function hostAction(v: unknown): HostAction | null {
  return pick(HOST_ACTIONS, v);
}

const HOST_FOLDERS: Readonly<Record<HostFolder, true>> = {
  workspace: true,
  runtime_source: true,
  dumps: true,
  game_data: true,
  runtime_publish: true,
  host_logs: true,
  gamedata_packs: true,
};

export function hostFolder(v: unknown): HostFolder | null {
  return pick(HOST_FOLDERS, v);
}

// Text the host types (a server name, who an invite is for): it goes to the host kit as one argument
// (never through a shell), so only its length and control characters matter.
function plainText(v: unknown, max: number): string | null {
  if (typeof v !== "string") return null;
  const t = v.trim();
  return t.length <= max && !/[\u0000-\u001f\u007f]/.test(t) ? t : null;
}

export function hostSetupRequest(v: unknown): HostSetupRequest | null {
  if (typeof v !== "object" || v === null || Array.isArray(v)) return null;
  const o = v as Record<string, unknown>;
  const name = plainText(o.name, 60);
  if (!name) return null;
  if (o.mode !== "private" && o.mode !== "public") return null;
  if (o.mode === "private") return { name, mode: "private", publicHost: null };
  const host = typeof o.publicHost === "string" ? o.publicHost.trim() : "";
  if (!/^[A-Za-z0-9](?:[A-Za-z0-9.-]{0,251}[A-Za-z0-9])?$/.test(host)) return null;
  return { name, mode: "public", publicHost: host };
}

// The test multipliers: null turns them off, otherwise XP and rewards from 1 to 100 (two decimals at most).
// Undefined: refused.
export function hostMultipliers(v: unknown): HostMultipliers | null | undefined {
  if (v === null) return null;
  if (typeof v !== "object" || Array.isArray(v)) return undefined;
  const o = v as Record<string, unknown>;
  const n = (x: unknown) => (typeof x === "number" && Number.isFinite(x) && x >= 1 && x <= MAX_MULTIPLIER ? Math.round(x * 100) / 100 : null);
  const xp = n(o.xp);
  const rewards = n(o.rewards);
  if (xp === null || rewards === null) return undefined;
  return xp === 1 && rewards === 1 ? null : { xp, rewards };
}

// Who an invite is for: optional, a short note.
export function inviteFor(v: unknown): string | null | undefined {
  if (v === null || v === undefined || v === "") return null;
  const t = plainText(v, 40);
  return t === null ? undefined : t || null;
}

// Hidden override for the relay port (tests and rehearsals on a PC where 61100 is taken).
export function relayPortOverride(v: string | undefined): number | undefined {
  if (typeof v !== "string" || !/^[1-9][0-9]{3,4}$/.test(v.trim())) return undefined;
  const n = Number(v.trim());
  return n >= 1024 && n <= 65535 ? n : undefined;
}

// Only the launcher's own page may call in: the packaged file:// page, or the Vite dev server.
export function isTrustedPageUrl(url: string, devServerUrl: string | undefined, rendererIndexUrl: string): boolean {
  if (typeof url !== "string") return false;
  if (devServerUrl) {
    try {
      return new URL(url).origin === new URL(devServerUrl).origin;
    } catch {
      return false;
    }
  }
  const strip = (u: string) => u.split("#")[0].split("?")[0].toLowerCase();
  return strip(url) === strip(rendererIndexUrl);
}
