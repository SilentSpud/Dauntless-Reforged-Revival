// A local stand-in for a Reforged server as the launcher sees it through the gateway: the backend's
// launcher API (/reforged/api/*) and the content server (/content/v1/*) on one address. It listens
// on 127.0.0.1 only, on the test's own port, in plain HTTP (private mode) or TLS (public mode).

import http from "node:http";
import https from "node:https";
import { createHash } from "node:crypto";
import type { AddressInfo } from "node:net";

export interface TlsOptions {
  cert: string;
  key: string;
}

export function sha256(data: Buffer): string {
  return createHash("sha256").update(data).digest("hex");
}

export function pattern(n: number, seed: number): Buffer {
  const b = Buffer.alloc(n);
  for (let i = 0; i < n; i++) b[i] = (i * 31 + seed * 7) & 0xff;
  return b;
}

export interface RuntimeFile {
  name: string;
  data: Buffer;
}

export function makeRuntime(seed = 1): RuntimeFile[] {
  return [
    { name: "MysticParadox.dll", data: pattern(200_000 + seed, seed) },
    { name: "winmm.dll", data: pattern(40_000 + seed, seed + 50) },
  ];
}

export function runtimeManifestFor(files: RuntimeFile[], version = "20261014-abcdef12") {
  return {
    version,
    build: { name: "rel-1.14.7-Archon", changelist: 647472 },
    files: files.map((f) => ({ name: f.name, size: f.data.length, sha256: sha256(f.data) })).sort((a, b) => (a.name < b.name ? -1 : 1)),
  };
}

export interface FakeServerOptions {
  name?: string;
  registration?: "OPEN" | "INVITECODE" | "NONE";
  validCodes?: Set<string>;
  existingUsers?: Map<string, string>; // lower-case username -> key
  banned?: Map<string, string>; // key -> reason
  runtime?: RuntimeFile[] | null; // null: the host has not published one
  runtimeOverride?: unknown; // a manifest that does not match the files
  corruptRuntime?: boolean; // serve flipped bytes
  gameData?: "real" | "synthetic" | "missing";
  statusMissing?: boolean;
  news?: unknown;
  // A Dauntless Revived server: /undaunted/api with its key header, and the 1.4.4 game files
  // (/content/v1/manifest and /content/v1/files/<path>) on the same address.
  revived?: { manifest: unknown; files: Map<string, Buffer>; regions?: unknown };
}

export interface Seen {
  path: string;
  key: string | undefined;
  status: number;
}

export class FakeServer {
  readonly seen: Seen[] = [];
  readonly registrations: { username: string; code: string }[] = [];
  readonly users = new Map<string, { username: string; id: string }>(); // key -> user
  private server: http.Server | null = null;
  port = 0;
  private counter = 0;

  constructor(public opts: FakeServerOptions = {}) {
    for (const [lower, key] of opts.existingUsers ?? []) this.users.set(key, { username: lower, id: `uid-${lower}` });
  }

  start(port = 0, tls?: TlsOptions): Promise<number> {
    return new Promise((resolve, reject) => {
      const handler: http.RequestListener = (req, res) => this.handle(req, res);
      this.server = tls ? https.createServer({ cert: tls.cert, key: tls.key }, handler) : http.createServer(handler);
      this.server.on("error", reject);
      this.server.listen(port, "127.0.0.1", () => {
        this.port = (this.server!.address() as AddressInfo).port;
        resolve(this.port);
      });
    });
  }

  stop(): Promise<void> {
    return new Promise((resolve) => {
      if (!this.server) return resolve();
      this.server.closeAllConnections();
      this.server.close(() => resolve());
    });
  }

  private json(res: http.ServerResponse, status: number, body: unknown, log: Seen): void {
    log.status = status;
    res.writeHead(status, { "content-type": "application/json" });
    res.end(JSON.stringify(body));
  }

  handle(req: http.IncomingMessage, res: http.ServerResponse): void {
    const url = new URL(req.url ?? "/", "http://x");
    const rv = this.opts.revived;
    const api = rv ? "/undaunted/api" : "/reforged/api";
    const header = req.headers[rv ? "x-undaunted-user-api-key" : "x-reforged-user-api-key"];
    const key = typeof header === "string" ? header : undefined;
    const log: Seen = { path: url.pathname, key, status: 0 };
    this.seen.push(log);
    const user = key ? this.users.get(key) : undefined;
    switch (url.pathname) {
      case "/dauntless-status":
        return this.json(res, 200, { "show-status": true, name: this.opts.name ?? "Test Server" }, log);
      case `${api}/ServerStatus`: {
        if (this.opts.statusMissing) return this.json(res, 404, {}, log);
        const full = user !== undefined;
        return this.json(
          res,
          200,
          {
            name: this.opts.name ?? "Test Server",
            online: true,
            version: "0.1.0",
            commit: "abc1234",
            sourceUrl: "https://github.com/FreezaMotoqueiro/Dauntless-Reforged-Revival",
            registration: this.opts.registration ?? "INVITECODE",
            playersOnline: full ? 1 : 0,
            players: full ? [{ name: "Aurora", where: "city", instance: "city-8777" }] : [],
            instances: full ? [{ id: "city-8777", kind: "city", title: "Ramsgate", map: "Hub", behemoth: null, players: 1, maxPlayers: 100, startedAt: new Date().toISOString() }] : [],
            uptimeSeconds: 600,
            game: { build: "rel-1.14.7-Archon", changelist: 647472, executableSha256: null },
            gameData: this.opts.gameData ?? "real",
            chat: true,
            ...(rv ? { contentPort: this.port } : {}),
            limited: !full,
          },
          log,
        );
      }
      case `${api}/GetUserInfo`: {
        if (key && this.opts.banned?.has(key)) return this.json(res, 403, { error: "account_banned", reason: this.opts.banned.get(key) }, log);
        if (!user) return this.json(res, 401, { error: "unauthorized" }, log);
        return this.json(res, 200, { UserId: user.id, Username: user.username, IsAdmin: false }, log);
      }
      case `${api}/Register`: {
        if (req.method !== "POST") return this.json(res, 405, {}, log);
        let body = "";
        req.on("data", (c) => (body += c));
        req.on("end", () => {
          let parsed: { Username?: unknown; InviteCode?: unknown };
          try {
            parsed = JSON.parse(body);
          } catch {
            return this.json(res, 400, { error: "bad_request" }, log);
          }
          const { Username: username, InviteCode: code } = parsed;
          if (this.opts.registration === "NONE") return this.json(res, 400, { error: "registration_closed" }, log);
          if (typeof username !== "string" || !/^[A-Za-z0-9_]{3,16}$/.test(username)) return this.json(res, 400, { error: "username_invalid" }, log);
          for (const u of this.users.values()) if (u.username.toLowerCase() === username.toLowerCase()) return this.json(res, 409, { error: "username_taken" }, log);
          if ((this.opts.registration ?? "INVITECODE") === "INVITECODE") {
            if (typeof code !== "string" || !this.opts.validCodes?.has(code)) return this.json(res, 401, { error: "invite_invalid" }, log);
            this.opts.validCodes.delete(code);
          }
          this.registrations.push({ username, code: String(code) });
          const newKey = `RRK_${(++this.counter).toString(16).padStart(48, "0")}`;
          this.users.set(newKey, { username, id: `uid-${this.counter}` });
          return this.json(res, 200, { UUK: newKey }, log);
        });
        return;
      }
      case "/content/v1/manifest":
        if (!rv) return this.json(res, 404, {}, log);
        return this.json(res, 200, rv.manifest, log);
      case `${api}/HuntRegion`:
        if (!rv || !user) return this.json(res, 404, {}, log);
        if (req.method === "POST") {
          let body = "";
          req.on("data", (c) => (body += c));
          req.on("end", () => this.json(res, 200, { region: (JSON.parse(body) as { region: string }).region }, log));
          return;
        }
        return rv.regions === undefined ? this.json(res, 404, {}, log) : this.json(res, 200, rv.regions, log);
      case `${api}/PlayingEnded`:
        return this.json(res, user ? 200 : 401, {}, log);
      case "/content/v1/news":
        return this.json(res, 200, this.opts.news ?? { items: [] }, log);
      case "/content/v1/branding":
        return this.json(res, 200, { backgrounds: [], accent: null }, log);
      case "/content/v1/runtime/manifest": {
        if (this.opts.runtimeOverride !== undefined) return this.json(res, 200, this.opts.runtimeOverride, log);
        if (!this.opts.runtime) return this.json(res, 404, { error: "no_runtime" }, log);
        return this.json(res, 200, runtimeManifestFor(this.opts.runtime), log);
      }
    }
    if (rv && url.pathname.startsWith("/content/v1/files/")) {
      if (!user) return this.json(res, 401, { error: "unauthorized" }, log);
      const rel = url.pathname.slice("/content/v1/files/".length).split("/").map(decodeURIComponent).join("/");
      const data = rv.files.get(rel);
      if (!data) return this.json(res, 404, {}, log);
      log.status = 200;
      res.writeHead(200, { "content-type": "application/octet-stream", "content-length": String(data.length), etag: `"${sha256(data)}"` });
      res.end(data);
      return;
    }
    if (url.pathname.startsWith("/content/v1/runtime/files/")) {
      if (!user) return this.json(res, 401, { error: "unauthorized" }, log);
      const name = decodeURIComponent(url.pathname.slice("/content/v1/runtime/files/".length));
      const file = this.opts.runtime?.find((f) => f.name === name);
      if (!file) return this.json(res, 404, {}, log);
      let data = file.data;
      if (this.opts.corruptRuntime) {
        data = Buffer.from(data);
        data[0] ^= 0xff;
      }
      log.status = 200;
      res.writeHead(200, { "content-type": "application/octet-stream", "content-length": String(data.length) });
      res.end(data);
      return;
    }
    log.status = 404;
    res.writeHead(404).end();
  }
}
