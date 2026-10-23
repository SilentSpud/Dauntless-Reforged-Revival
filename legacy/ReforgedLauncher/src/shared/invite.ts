// Invite strings, the same two versions as Dauntless Revived's:
//
//  v=1, private mode (the host's PC, reached over Tailscale):
//   reforged-revival://join?v=1&host=<tailnet IPv4 or MagicDNS name>&port=61000&code=<invite code>
//     &name=<url-encoded server name>[&share=<url-encoded Tailscale machine-share URL>]
//   The host must be a Tailscale address (100.64.0.0/10), a *.ts.net name or loopback: v1 is plain
//   HTTP (Tailscale encrypts it) and must never be pointed at the internet.
//
//  v=2, public mode (a server on a public IP, reached through its TLS gateway):
//   reforged-revival://join?v=2&mode=public&host=<public IPv4 or DNS name>&port=<gateway port, default 443>
//     &fp=<SHA-256 of the server's TLS certificate (DER), 64 lower-case hex>&code=<invite code>
//     &name=<url-encoded server name>
//
// Dauntless Revived's own invites (dauntless-revived://join?..., the same two shapes) are read too:
// those servers run Dauntless 1.4.4 (Undaunted), and the launcher plays them the way Dauntless Revived's
// launcher 0.1.35 does (src/main/revived/). The scheme decides which game a server is for.
//
// The same string arrives either pasted by the friend or through the reforged-revival: protocol
// handler. It is parsed strictly: anything that is not exactly one of these shapes is rejected,
// because the host, port and fingerprint decide where the account key is sent later.

import { UNSAFE_CHAR } from "./text";

export const INVITE_SCHEME = "reforged-revival";
export const REVIVED_INVITE_SCHEME = "dauntless-revived";

// Which game a server is for: "reforged", Dauntless 1.14.7 with Mystic Paradox (this project's
// servers); "revived", Dauntless 1.4.4 with Undaunted (Dauntless Revived's servers).
export type GameEdition = "reforged" | "revived";
export const GAME_EDITIONS: readonly GameEdition[] = ["reforged", "revived"];
export const DEFAULT_PUBLIC_PORT = 443;

export type ServerMode = "private" | "public";

export interface Invite {
  game: GameEdition;
  mode: ServerMode;
  host: string;
  port: number;
  code: string;
  name: string;
  share: string | null; // private mode only: the Tailscale share link
  fp: string | null; // public mode only: the pinned certificate fingerprint
}

export type InviteError =
  | "empty"
  | "too_long"
  | "not_invite"
  | "version"
  | "params"
  | "mode"
  | "host"
  | "port"
  | "fp"
  | "code"
  | "name"
  | "share";

export type InviteResult = { ok: true; invite: Invite } | { ok: false; error: InviteError };

const MAX_INVITE_LENGTH = 2048;

const V1_ALLOWED = new Set(["v", "host", "port", "code", "name", "share"]);
const V1_REQUIRED: InviteError[] = ["host", "port", "code", "name"];
const V2_ALLOWED = new Set(["v", "mode", "host", "port", "fp", "code", "name"]);
const V2_REQUIRED: InviteError[] = ["mode", "host", "fp", "code", "name"];

export function isValidIPv4(text: string): boolean {
  const parts = text.split(".");
  if (parts.length !== 4) return false;
  return parts.every((p) => /^(0|[1-9][0-9]{0,2})$/.test(p) && Number(p) <= 255);
}

// IPv4, or a DNS name made of [a-z0-9.-]. Names that a resolver could read as a number
// ("1.2.3", "0x7f.1", "host.123") are refused.
export function isValidHost(host: string): boolean {
  if (typeof host !== "string" || host.length === 0 || host.length > 253) return false;
  if (/^[0-9.]+$/.test(host)) return isValidIPv4(host);
  if (!/^[a-z0-9.-]+$/.test(host)) return false;
  const labels = host.split(".");
  for (const label of labels) {
    if (label.length === 0 || label.length > 63) return false;
    if (label.startsWith("-") || label.endsWith("-")) return false;
  }
  if (/^[0-9]+$/.test(labels[labels.length - 1])) return false;
  if (/^0x/i.test(labels[labels.length - 1])) return false;
  return true;
}

export function isValidPort(text: string): boolean {
  if (!/^[1-9][0-9]{0,4}$/.test(text)) return false;
  const n = Number(text);
  return n >= 1 && n <= 65535;
}

export function isValidInviteCode(code: string): boolean {
  return /^[A-Za-z0-9-]{4,64}$/.test(code);
}

// SHA-256 of the certificate's DER bytes, as 64 lower-case hex characters.
export function isValidFingerprint(fp: unknown): fp is string {
  return typeof fp === "string" && /^[0-9a-f]{64}$/.test(fp);
}

export function isValidShareUrl(text: string): boolean {
  if (text.length > 512) return false;
  let url: URL;
  try {
    url = new URL(text);
  } catch {
    return false;
  }
  return (
    url.protocol === "https:" &&
    url.hostname === "login.tailscale.com" &&
    url.port === "" &&
    url.username === "" &&
    url.password === "" &&
    text.toLowerCase().startsWith("https://login.tailscale.com/")
  );
}

// Server names are shown to the friend, always as text (never as HTML).
export function cleanServerName(raw: string): string | null {
  // Control characters, and bidirectional overrides that could disguise the name.
  if (UNSAFE_CHAR.test(raw)) return null;
  const name = raw.trim().replace(/\s+/g, " ");
  if (name.length < 1 || Array.from(name).length > 64) return null;
  return name;
}

export function parseInvite(input: unknown): InviteResult {
  if (typeof input !== "string") return { ok: false, error: "empty" };
  const text = input.trim();
  if (text.length === 0) return { ok: false, error: "empty" };
  if (text.length > MAX_INVITE_LENGTH) return { ok: false, error: "too_long" };

  const prefix = /^(reforged-revival|dauntless-revived):\/\/join\/?\?/i.exec(text);
  if (!prefix) return { ok: false, error: "not_invite" };
  const game: GameEdition = prefix[1].toLowerCase() === REVIVED_INVITE_SCHEME ? "revived" : "reforged";
  const query = text.slice(prefix[0].length);
  if (query.includes("#")) return { ok: false, error: "params" };

  // Parse by hand so that duplicates and unknown keys can be refused.
  const values = new Map<string, string>();
  for (const pair of query.split("&")) {
    const eq = pair.indexOf("=");
    if (eq <= 0) return { ok: false, error: "params" };
    const key = pair.slice(0, eq);
    const rawValue = pair.slice(eq + 1);
    if (!V2_ALLOWED.has(key) && !V1_ALLOWED.has(key)) return { ok: false, error: "params" };
    if (values.has(key)) return { ok: false, error: "params" };
    let value: string;
    try {
      value = decodeURIComponent(rawValue.replace(/\+/g, " "));
    } catch {
      return { ok: false, error: "params" };
    }
    values.set(key, value);
  }

  const version = values.get("v");
  if (version !== "1" && version !== "2") return { ok: false, error: "version" };
  const allowed = version === "1" ? V1_ALLOWED : V2_ALLOWED;
  for (const key of values.keys()) if (!allowed.has(key)) return { ok: false, error: "params" };
  for (const key of version === "1" ? V1_REQUIRED : V2_REQUIRED) {
    if (!values.has(key)) return { ok: false, error: key };
  }

  if (version === "2" && values.get("mode") !== "public") return { ok: false, error: "mode" };

  const host = (values.get("host") as string).toLowerCase();
  if (!isValidHost(host)) return { ok: false, error: "host" };
  // v1 is plain HTTP, so it may only point into the tailnet. A server on the internet needs a v2
  // (public, pinned TLS) invite.
  if (version === "1" && !isPrivateModeHost(host)) return { ok: false, error: "host" };

  let port = DEFAULT_PUBLIC_PORT;
  if (values.has("port")) {
    const portText = values.get("port") as string;
    if (!isValidPort(portText)) return { ok: false, error: "port" };
    port = Number(portText);
  }

  let fp: string | null = null;
  if (version === "2") {
    const raw = values.get("fp") as string;
    if (!isValidFingerprint(raw)) return { ok: false, error: "fp" };
    fp = raw;
  }

  const code = values.get("code") as string;
  if (!isValidInviteCode(code)) return { ok: false, error: "code" };

  const name = cleanServerName(values.get("name") as string);
  if (name === null) return { ok: false, error: "name" };

  let share: string | null = null;
  if (values.has("share")) {
    const raw = values.get("share") as string;
    if (!isValidShareUrl(raw)) return { ok: false, error: "share" };
    share = raw;
  }

  return {
    ok: true,
    invite: { game, mode: version === "1" ? "private" : "public", host, port, code, name, share, fp },
  };
}

export function isLoopbackHost(host: string): boolean {
  return host === "localhost" || /^127\.[0-9]+\.[0-9]+\.[0-9]+$/.test(host);
}

// Tailscale hands out addresses from 100.64.0.0/10.
export function isTailscaleIPv4(host: string): boolean {
  if (!isValidIPv4(host)) return false;
  const [a, b] = host.split(".").map(Number);
  return a === 100 && b >= 64 && b <= 127;
}

// A MagicDNS name: <machine>.<tailnet>.ts.net.
export function isMagicDnsName(host: string): boolean {
  return isValidHost(host) && host.endsWith(".ts.net") && host.split(".").length >= 3;
}

// Where private mode (plain HTTP, invite v1) may connect: a Tailscale address or MagicDNS name,
// or this PC itself (tests, and a host playing on the server PC).
export function isPrivateModeHost(host: string): boolean {
  return isTailscaleIPv4(host) || isMagicDnsName(host) || isLoopbackHost(host);
}

// Builds an invite string (used by tests and by hosts' tooling for reference).
export function formatInvite(invite: Omit<Invite, "game"> & { game?: GameEdition }): string {
  const parts: string[] = [];
  if (invite.mode === "public") {
    if (!isValidFingerprint(invite.fp)) throw new Error("public invites need a certificate fingerprint");
    parts.push("v=2", "mode=public", `host=${encodeURIComponent(invite.host)}`, `port=${invite.port}`, `fp=${invite.fp}`);
  } else {
    parts.push("v=1", `host=${encodeURIComponent(invite.host)}`, `port=${invite.port}`);
  }
  parts.push(`code=${encodeURIComponent(invite.code)}`, `name=${encodeURIComponent(invite.name)}`);
  if (invite.mode === "private" && invite.share) parts.push(`share=${encodeURIComponent(invite.share)}`);
  return `${invite.game === "revived" ? REVIVED_INVITE_SCHEME : INVITE_SCHEME}://join?${parts.join("&")}`;
}
