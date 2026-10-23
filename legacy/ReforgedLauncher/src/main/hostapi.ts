// Calls to the host: the backend's launcher API (/reforged/api, the same contract as Dauntless
// Revived's /undaunted/api) and the content server behind the gateway (/content/v1).
//
// The account key is sent only in the x-reforged-user-api-key header, only to the invite's host
// (in public mode only over the TLS connection pinned to the invite's certificate fingerprint):
// GetUserInfo, ServerStatus (for the player list) and the runtime downloads.

import { Endpoint, HttpError, HttpResponse, parseJsonBody, request } from "./http";
import { parseServerStatus, ServerStatus, cleanText } from "../shared/status";
import { isPlausibleAccountKey } from "../shared/username";
import { isPrivateModeHost } from "../shared/invite";
import type { ErrorCode, NewsItem, RuntimeFileInfo } from "../shared/types";
import { CONNECT_TIMEOUT_MS, MAX_RUNTIME_FILE_BYTES, RUNTIME_FILE_NAMES } from "./constants";
import { UNSAFE_CHARS_MULTILINE } from "../shared/text";

export const KEY_HEADER = "x-reforged-user-api-key";

// Where a server's launcher API is and the header its account key goes in: this project's servers, or
// Dauntless Revived's (Dauntless 1.4.4, src/main/revived/), whose API is the same contract.
export interface ApiFlavor {
  base: string;
  keyHeader: string;
}
export const REFORGED_FLAVOR: ApiFlavor = { base: "/reforged/api", keyHeader: KEY_HEADER };
export const REVIVED_FLAVOR: ApiFlavor = { base: "/undaunted/api", keyHeader: "x-undaunted-user-api-key" };

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function isPin(e: unknown): boolean {
  return e instanceof HttpError && e.kind === "pin";
}

// The key goes only where every other key-carrying request goes: to the invite's host, over the TLS
// connection pinned to the invite's certificate (public mode) or plain HTTP inside the tailnet or
// to this PC (private mode, which request() enforces as well).
export function keyHeaders(ep: Endpoint, key: string | null | undefined, flavor: ApiFlavor = REFORGED_FLAVOR): Record<string, string> {
  if (!isPlausibleAccountKey(key)) return {};
  if (ep.pin === null && !isPrivateModeHost(ep.host)) return {};
  return { [flavor.keyHeader]: key };
}

// ---------------------------------------------------------------- ServerStatus

export type StatusResult =
  | { kind: "ok"; status: ServerStatus }
  | { kind: "unsupported" } // something like a Reforged/Revived server, without ServerStatus
  | { kind: "bad_answer" }
  | { kind: "cert_mismatch" } // public mode: the certificate is not the one from the invite
  | { kind: "unreachable" };

export interface StatusOptions {
  // The account key for this server. The server lists who is online only for a registered player;
  // without a key (or with one it does not accept) it answers the same status with "limited": true.
  key?: string | null;
  timeoutMs?: number;
  flavor?: ApiFlavor;
}

export async function fetchServerStatus(ep: Endpoint, opts: StatusOptions = {}): Promise<StatusResult> {
  const timeoutMs = opts.timeoutMs ?? CONNECT_TIMEOUT_MS;
  let res: HttpResponse;
  try {
    const flavor = opts.flavor ?? REFORGED_FLAVOR;
    res = await request(ep, `${flavor.base}/ServerStatus`, { headers: keyHeaders(ep, opts.key, flavor), timeoutMs, maxBytes: 1024 * 1024 });
  } catch (e) {
    return isPin(e) ? { kind: "cert_mismatch" } : { kind: "unreachable" };
  }
  if (res.status === 200) {
    const status = parseServerStatus(parseJsonBody(res));
    return status ? { kind: "ok", status } : { kind: "bad_answer" };
  }
  if (res.status === 404) {
    try {
      const legacy = await request(ep, "/dauntless-status", { timeoutMs, maxBytes: 64 * 1024 });
      const body = parseJsonBody(legacy);
      if (legacy.status === 200 && isObject(body) && "show-status" in body) return { kind: "unsupported" };
    } catch (e) {
      return isPin(e) ? { kind: "cert_mismatch" } : { kind: "unreachable" };
    }
  }
  return { kind: "bad_answer" };
}

// ---------------------------------------------------------------- Register / GetUserInfo

export type RegisterResult = { ok: true; key: string } | { ok: false; error: ErrorCode; message?: string };

export async function registerAccount(ep: Endpoint, username: string, inviteCode: string, flavor: ApiFlavor = REFORGED_FLAVOR): Promise<RegisterResult> {
  let res: HttpResponse;
  try {
    res = await request(ep, `${flavor.base}/Register`, {
      method: "POST",
      body: JSON.stringify({ Username: username, InviteCode: inviteCode }),
      timeoutMs: 20000,
      maxBytes: 64 * 1024,
    });
  } catch (e) {
    return { ok: false, error: isPin(e) ? "cert_mismatch" : "server_unreachable" };
  }
  const body = parseJsonBody(res);
  const serverError = isObject(body) && typeof body.error === "string" ? body.error : null;
  const message = isObject(body) ? cleanText(body.message, 200) ?? undefined : undefined;
  switch (res.status) {
    case 200: {
      const key = isObject(body) ? body.UUK : undefined;
      if (!isPlausibleAccountKey(key)) return { ok: false, error: "server_error" };
      return { ok: true, key };
    }
    case 400:
      if (serverError === "username_invalid" || serverError === "registration_closed" || serverError === "bad_request") {
        return { ok: false, error: serverError, message };
      }
      return { ok: false, error: "bad_request", message };
    case 401:
      return { ok: false, error: "invite_invalid" };
    case 409:
      return { ok: false, error: "username_taken" };
    case 413:
    case 429:
      return { ok: false, error: "rate_limited" };
    default:
      return { ok: false, error: "server_error" };
  }
}

export interface UserInfo {
  userId: string;
  username: string;
  isAdmin: boolean;
}

export type UserInfoResult =
  | { ok: true; info: UserInfo }
  | { ok: false; error: "account_banned" | "key_rejected" | "server_unreachable" | "server_error" | "cert_mismatch" | "rate_limited"; detail?: string };

export async function fetchUserInfo(ep: Endpoint, key: string, flavor: ApiFlavor = REFORGED_FLAVOR): Promise<UserInfoResult> {
  if (!isPlausibleAccountKey(key)) return { ok: false, error: "key_rejected" };
  let res: HttpResponse;
  try {
    res = await request(ep, `${flavor.base}/GetUserInfo`, { headers: keyHeaders(ep, key, flavor), timeoutMs: 10000, maxBytes: 64 * 1024 });
  } catch (e) {
    return { ok: false, error: isPin(e) ? "cert_mismatch" : "server_unreachable" };
  }
  if (res.status === 403) {
    const body = parseJsonBody(res);
    if (isObject(body) && (body.error === "account_banned" || body.error === "account_disabled")) {
      return { ok: false, error: "account_banned", detail: cleanText(body.reason, 500) ?? undefined };
    }
  }
  if (res.status === 401 || res.status === 403) return { ok: false, error: "key_rejected" };
  if (res.status === 429) return { ok: false, error: "rate_limited" };
  if (res.status !== 200) return { ok: false, error: "server_error" };
  const body = parseJsonBody(res);
  if (!isObject(body)) return { ok: false, error: "server_error" };
  const userId = cleanText(body.UserId, 128);
  const username = cleanText(body.Username, 32);
  // The user id goes on the game's command line (-AUTH_LOGIN, -epicuserid): plain characters only.
  if (userId === null || username === null || !/^[A-Za-z0-9_-]{1,128}$/.test(userId)) return { ok: false, error: "server_error" };
  return { ok: true, info: { userId, username, isAdmin: body.IsAdmin === true } };
}

// ---------------------------------------------------------------- the game runtime

export interface RuntimeManifest {
  version: string;
  changelist: number | null;
  files: RuntimeFileInfo[];
}

const RUNTIME_NAMES: ReadonlySet<string> = new Set(RUNTIME_FILE_NAMES);

// GET /content/v1/runtime/manifest: {version, build: {changelist}, files: [{name, size, sha256}]}.
// Only the two known runtime files are accepted, each at most once, with a plain SHA-256: the
// launcher never writes a file the server names freely.
export function parseRuntimeManifest(raw: unknown): RuntimeManifest | null {
  if (!isObject(raw) || !Array.isArray(raw.files)) return null;
  const version = cleanText(raw.version, 48);
  if (version === null) return null;
  const files: RuntimeFileInfo[] = [];
  const seen = new Set<string>();
  for (const f of raw.files) {
    if (!isObject(f) || typeof f.name !== "string" || !RUNTIME_NAMES.has(f.name) || seen.has(f.name)) return null;
    if (typeof f.sha256 !== "string" || !/^[0-9a-f]{64}$/.test(f.sha256)) return null;
    if (typeof f.size !== "number" || !Number.isSafeInteger(f.size) || f.size < 1 || f.size > MAX_RUNTIME_FILE_BYTES) return null;
    seen.add(f.name);
    files.push({ name: f.name, size: f.size, sha256: f.sha256 });
  }
  // Both files, or the runtime would not load.
  if (files.length !== RUNTIME_NAMES.size) return null;
  files.sort((a, b) => (a.name < b.name ? -1 : 1));
  const build = isObject(raw.build) ? raw.build : {};
  const changelist = typeof build.changelist === "number" && Number.isSafeInteger(build.changelist) ? build.changelist : null;
  return { version, changelist, files };
}

export type RuntimeManifestResult =
  | { kind: "ok"; manifest: RuntimeManifest }
  | { kind: "none" } // the server has no runtime to hand out yet (404)
  | { kind: "bad_answer" }
  | { kind: "cert_mismatch" }
  | { kind: "unreachable" };

export async function fetchRuntimeManifest(ep: Endpoint): Promise<RuntimeManifestResult> {
  let res: HttpResponse;
  try {
    res = await request(ep, "/content/v1/runtime/manifest", { timeoutMs: 10000, maxBytes: 64 * 1024 });
  } catch (e) {
    return isPin(e) ? { kind: "cert_mismatch" } : { kind: "unreachable" };
  }
  if (res.status === 404 || res.status === 503) return { kind: "none" };
  if (res.status !== 200) return { kind: "bad_answer" };
  const manifest = parseRuntimeManifest(parseJsonBody(res));
  return manifest ? { kind: "ok", manifest } : { kind: "bad_answer" };
}

// ---------------------------------------------------------------- news and the art pack

export function cleanNewsBody(v: unknown): string | null {
  if (typeof v !== "string") return null;
  const text = v
    .replace(/\r\n?/g, "\n")
    .replace(UNSAFE_CHARS_MULTILINE, "")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
  if (text.length === 0) return null;
  return text.length > 4000 ? text.slice(0, 4000) + "…" : text;
}

export function parseNews(raw: unknown): NewsItem[] {
  if (!isObject(raw) || !Array.isArray(raw.items)) return [];
  const items: NewsItem[] = [];
  for (const item of raw.items.slice(0, 30)) {
    if (!isObject(item)) continue;
    const title = cleanText(item.title, 120);
    const body = cleanNewsBody(item.body);
    if (title === null || body === null) continue;
    const t = typeof item.date === "string" ? Date.parse(item.date) : NaN;
    items.push({ date: Number.isFinite(t) ? new Date(t).toISOString() : null, title, body });
  }
  return items;
}

export async function fetchNews(ep: Endpoint, fallback: NewsItem[] = []): Promise<NewsItem[]> {
  try {
    const res = await request(ep, "/content/v1/news", { timeoutMs: 10000, maxBytes: 512 * 1024 });
    return res.status === 200 ? parseNews(parseJsonBody(res)) : fallback;
  } catch {
    return fallback;
  }
}

export interface BrandingIndex {
  backgrounds: { path: string; credit: string | null }[];
  accent: string | null;
}

const BRANDING_PATH = /^\/content\/v1\/branding\/[A-Za-z0-9_-][A-Za-z0-9_.-]{0,100}\.(jpg|jpeg|png|webp)$/i;

export function parseBranding(raw: unknown): BrandingIndex {
  const out: BrandingIndex = { backgrounds: [], accent: null };
  if (!isObject(raw)) return out;
  if (Array.isArray(raw.backgrounds)) {
    for (const b of raw.backgrounds.slice(0, 8)) {
      if (!isObject(b) || typeof b.url !== "string" || !BRANDING_PATH.test(b.url) || b.url.includes("..")) continue;
      out.backgrounds.push({ path: b.url, credit: cleanText(b.credit, 120) });
    }
  }
  if (typeof raw.accent === "string" && /^#[0-9a-fA-F]{6}$/.test(raw.accent)) out.accent = raw.accent.toLowerCase();
  return out;
}

export async function fetchBrandingIndex(ep: Endpoint): Promise<BrandingIndex> {
  try {
    const res = await request(ep, "/content/v1/branding", { timeoutMs: 10000, maxBytes: 64 * 1024 });
    return res.status === 200 ? parseBranding(parseJsonBody(res)) : { backgrounds: [], accent: null };
  } catch {
    return { backgrounds: [], accent: null };
  }
}

export type ImageType = "image/jpeg" | "image/png" | "image/webp";

// Decides the image type from its first bytes; the server's Content-Type is not trusted.
export function sniffImage(data: Buffer): ImageType | null {
  if (data.length >= 3 && data[0] === 0xff && data[1] === 0xd8 && data[2] === 0xff) return "image/jpeg";
  if (data.length >= 8 && data.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]))) return "image/png";
  if (data.length >= 12 && data.toString("latin1", 0, 4) === "RIFF" && data.toString("latin1", 8, 12) === "WEBP") return "image/webp";
  return null;
}

export const MAX_ART_BYTES = 15 * 1024 * 1024;

export async function fetchBrandingImage(ep: Endpoint, urlPath: string): Promise<{ type: ImageType; data: Buffer } | null> {
  if (!BRANDING_PATH.test(urlPath) || urlPath.includes("..")) return null;
  try {
    const res = await request(ep, urlPath, { timeoutMs: 30000, maxBytes: MAX_ART_BYTES });
    if (res.status !== 200) return null;
    const type = sniffImage(res.body);
    return type ? { type, data: res.body } : null;
  } catch {
    return null;
  }
}
