// Downloads Dauntless 1.4.4 files from a Dauntless Revived host's content server, as Dauntless Revived's
// launcher 0.1.35 does (src/main/downloader.ts there, unchanged but for the imports).
//
// - Only files from the compiled-in manifest are requested, and each one is written to
//   <file>.part first. A file only gets its real name after its size and SHA-256 match.
// - Interrupted files resume with an HTTP Range request (the .part is re-hashed first).
// - Up to 4 files at once, retries with exponential backoff, pause / resume / cancel.
// - In public mode every connection is TLS pinned to the invite's certificate (pinned.ts); a
//   certificate that does not match stops the whole job before any request is sent.

import http from "node:http";
import https from "node:https";
import { createHash, type Hash } from "node:crypto";
import { createReadStream, createWriteStream, promises as fsp, type WriteStream } from "node:fs";
import path from "node:path";
import { endpointUrl, hostHeader, pinnedTarget, type Endpoint } from "../http";
import { isPinError, PinnedAgent } from "../pinned";
import { REVIVED_KEY_HEADER } from "./constants";
import { resolveInside, type ManifestFile } from "./manifest";

export type DownloadErrorCode =
  | "key_rejected"
  | "file_missing_on_server"
  | "file_different_on_server"
  | "download_failed"
  | "disk_space"
  | "disk_error"
  | "cert_mismatch"
  | "cancelled";

export class DownloadError extends Error {
  constructor(
    public readonly code: DownloadErrorCode,
    public readonly filePath?: string,
    message?: string,
  ) {
    super(message ?? code);
    this.name = "DownloadError";
  }
}

// Thrown inside one attempt; decides whether the file is retried.
class AttemptError extends Error {
  constructor(
    public readonly retry: boolean,
    message: string,
    public readonly fatal?: DownloadError,
    // The server is busy (429 / 503 with Retry-After): wait this long, not counted as a failure.
    public readonly busyWaitMs?: number,
  ) {
    super(message);
  }
}

// Retry-After in seconds, kept between 1 and 30 s.
export function retryAfterMs(v: string | string[] | undefined): number {
  const n = Number(Array.isArray(v) ? v[0] : v);
  if (!Number.isFinite(n) || n <= 0) return 2000;
  return Math.min(30, Math.max(1, n)) * 1000;
}

const MAX_BUSY_WAITS = 60;

class Interrupted extends Error {
  constructor() {
    super("interrupted");
  }
}

export interface DownloadProgress {
  paused: boolean;
  totalBytes: number;
  doneBytes: number;
  filesTotal: number;
  filesDone: number;
  bytesPerSecond: number;
  etaSeconds: number | null;
  currentFile: string | null;
}

export interface DownloadOptions {
  endpoint: Endpoint;
  downloadBaseUrl?: string;
  key: string;
  installDir: string;
  files: ManifestFile[];
  concurrency?: number;
  maxAttempts?: number;
  backoffMs?: (attempt: number) => number;
  stallTimeoutMs?: number;
  progressIntervalMs?: number;
  onProgress?: (p: DownloadProgress) => void;
  onFileVerified?: (f: ManifestFile) => void;
  onRetry?: (f: ManifestFile, attempt: number, reason: string) => void;
}

// Game hashes remain pinned in the launcher even when the host advertises a CDN.
export function contentDownloadBase(raw: unknown): string | undefined {
  if (!raw || typeof raw !== "object" || !("downloadBaseUrl" in raw) || raw.downloadBaseUrl === undefined) return undefined;
  if (typeof raw.downloadBaseUrl !== "string") throw new Error("Invalid content download URL");
  const url = new URL(raw.downloadBaseUrl);
  if (url.protocol !== "https:" || url.username || url.password || url.search || url.hash) throw new Error("Invalid content download URL");
  return url.href.replace(/\/+$/, "") + "/";
}

export function fileUrlPath(rel: string): string {
  return "/content/v1/files/" + rel.split("/").map(encodeURIComponent).join("/");
}

export function defaultBackoff(attempt: number): number {
  const base = Math.min(30000, 1000 * 2 ** Math.max(0, attempt - 1));
  return Math.round(base * (0.75 + Math.random() * 0.5));
}

// Bytes per second over the last few seconds.
export class SpeedMeter {
  private buckets: { t: number; bytes: number }[] = [];
  constructor(private readonly windowMs = 5000, private readonly now: () => number = Date.now) {}
  add(bytes: number): void {
    const t = this.now();
    const last = this.buckets[this.buckets.length - 1];
    if (last && t - last.t < 250) last.bytes += bytes;
    else this.buckets.push({ t, bytes });
    this.trim(t);
  }
  rate(): number {
    const t = this.now();
    this.trim(t);
    if (this.buckets.length === 0) return 0;
    const span = Math.max(1000, t - this.buckets[0].t);
    const bytes = this.buckets.reduce((s, b) => s + b.bytes, 0);
    return (bytes * 1000) / span;
  }
  reset(): void {
    this.buckets = [];
  }
  private trim(t: number): void {
    while (this.buckets.length > 0 && t - this.buckets[0].t > this.windowMs) this.buckets.shift();
  }
}

function parseContentRange(v: string | undefined): { start: number; end: number; total: number } | null {
  if (!v) return null;
  const m = /^bytes (\d+)-(\d+)\/(\d+)$/.exec(v.trim());
  if (!m) return null;
  return { start: Number(m[1]), end: Number(m[2]), total: Number(m[3]) };
}

function etagHash(v: string | undefined): string | null {
  if (!v) return null;
  return v.trim().replace(/^W\//, "").replace(/^"|"$/g, "").toLowerCase();
}

function closeStream(ws: WriteStream): Promise<void> {
  return new Promise((resolve, reject) => {
    if (ws.closed) return resolve();
    ws.once("error", reject);
    ws.end(() => resolve());
  });
}

function sleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) return reject(new Interrupted());
    const t = setTimeout(() => {
      signal.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    const onAbort = () => {
      clearTimeout(t);
      reject(new Interrupted());
    };
    signal.addEventListener("abort", onAbort, { once: true });
  });
}

function hashExisting(file: string, hash: Hash, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const rs = createReadStream(file, { highWaterMark: 1024 * 1024 });
    const onAbort = () => {
      rs.destroy();
      reject(new Interrupted());
    };
    signal.addEventListener("abort", onAbort, { once: true });
    rs.on("data", (c) => hash.update(c as Buffer));
    rs.on("error", (e) => {
      signal.removeEventListener("abort", onAbort);
      reject(e);
    });
    rs.on("end", () => {
      signal.removeEventListener("abort", onAbort);
      resolve();
    });
  });
}

function isDiskError(e: unknown): DownloadError | null {
  const code = (e as NodeJS.ErrnoException)?.code;
  if (code === "ENOSPC") return new DownloadError("disk_space");
  if (code === "EPERM" || code === "EACCES" || code === "EBUSY" || code === "EROFS") {
    return new DownloadError("disk_error", undefined, code);
  }
  return null;
}

export class DownloadJob {
  private readonly queue: ManifestFile[];
  private readonly totalBytes: number;
  private completedBytes = 0;
  private filesDone = 0;
  private readonly inFlight = new Map<string, { bytes: number; controller: AbortController }>();
  // Bytes already on disk (in .part files) for files that are queued but not being fetched.
  private readonly partial = new Map<string, number>();
  private readonly attempts = new Map<string, number>();
  private readonly busyWaits = new Map<string, number>();
  private readonly speed = new SpeedMeter();
  private pausedFlag = false;
  private cancelled = false;
  private fatal: DownloadError | null = null;
  private resumeWaiters: (() => void)[] = [];
  private progressTimer: NodeJS.Timeout | null = null;
  private readonly agent: http.Agent;
  private readonly downloadBaseUrl?: string;
  private readonly concurrency: number;
  private readonly maxAttempts: number;
  private readonly backoff: (attempt: number) => number;
  private readonly stallTimeoutMs: number;
  private running = false;

  constructor(private readonly opts: DownloadOptions) {
    this.concurrency = Math.max(1, Math.min(opts.concurrency ?? 4, 4));
    this.maxAttempts = Math.max(1, opts.maxAttempts ?? 6);
    this.backoff = opts.backoffMs ?? defaultBackoff;
    this.stallTimeoutMs = opts.stallTimeoutMs ?? 30000;
    // Biggest files first, so the long transfers overlap instead of finishing last.
    this.queue = [...opts.files].sort((a, b) => b.size - a.size);
    this.totalBytes = opts.files.reduce((s, f) => s + f.size, 0);
    const target = pinnedTarget(opts.endpoint);
    this.downloadBaseUrl = contentDownloadBase({ downloadBaseUrl: opts.downloadBaseUrl });
    this.agent = this.downloadBaseUrl
      ? new https.Agent({ keepAlive: true, maxSockets: this.concurrency })
      : target
      ? new PinnedAgent(target, { keepAlive: true, maxSockets: this.concurrency })
      : new http.Agent({ keepAlive: true, maxSockets: this.concurrency });
  }

  get paused(): boolean {
    return this.pausedFlag;
  }

  pause(): void {
    if (this.pausedFlag || this.cancelled) return;
    this.pausedFlag = true;
    this.speed.reset();
    for (const f of this.inFlight.values()) f.controller.abort();
    this.emitProgress();
  }

  resume(): void {
    if (!this.pausedFlag || this.cancelled) return;
    this.pausedFlag = false;
    const waiters = this.resumeWaiters;
    this.resumeWaiters = [];
    waiters.forEach((w) => w());
    this.emitProgress();
  }

  cancel(): void {
    if (this.cancelled) return;
    this.cancelled = true;
    for (const f of this.inFlight.values()) f.controller.abort();
    const waiters = this.resumeWaiters;
    this.resumeWaiters = [];
    waiters.forEach((w) => w());
  }

  progress(): DownloadProgress {
    let doneBytes = this.completedBytes;
    let current: string | null = null;
    let biggest = -1;
    for (const b of this.partial.values()) doneBytes += b;
    for (const [p, f] of this.inFlight) {
      doneBytes += f.bytes;
      if (f.bytes > biggest) {
        biggest = f.bytes;
        current = p;
      }
    }
    const rate = this.pausedFlag ? 0 : this.speed.rate();
    const left = Math.max(0, this.totalBytes - doneBytes);
    return {
      paused: this.pausedFlag,
      totalBytes: this.totalBytes,
      doneBytes: Math.min(doneBytes, this.totalBytes),
      filesTotal: this.opts.files.length,
      filesDone: this.filesDone,
      bytesPerSecond: rate,
      etaSeconds: rate > 0 ? left / rate : null,
      currentFile: current,
    };
  }

  private emitProgress(): void {
    this.opts.onProgress?.(this.progress());
  }

  async run(): Promise<void> {
    if (this.running) throw new Error("already running");
    this.running = true;
    for (const f of this.queue) {
      try {
        const st = await fsp.stat(resolveInside(this.opts.installDir, f.path) + ".part");
        if (st.size > 0 && st.size <= f.size) this.partial.set(f.path, st.size);
      } catch {
        /* no partial file */
      }
    }
    this.progressTimer = setInterval(() => this.emitProgress(), this.opts.progressIntervalMs ?? 250);
    try {
      await Promise.all(Array.from({ length: this.concurrency }, () => this.worker()));
      if (this.cancelled) throw new DownloadError("cancelled");
      if (this.fatal) throw this.fatal;
    } finally {
      if (this.progressTimer) clearInterval(this.progressTimer);
      this.agent.destroy();
      this.emitProgress();
    }
  }

  private waitWhilePaused(): Promise<void> {
    if (!this.pausedFlag) return Promise.resolve();
    return new Promise((resolve) => this.resumeWaiters.push(resolve));
  }

  private stopFor(e: DownloadError): void {
    if (!this.fatal) this.fatal = e;
    for (const f of this.inFlight.values()) f.controller.abort();
    const waiters = this.resumeWaiters;
    this.resumeWaiters = [];
    waiters.forEach((w) => w());
  }

  private async worker(): Promise<void> {
    for (;;) {
      await this.waitWhilePaused();
      if (this.cancelled || this.fatal) return;
      const file = this.queue.shift();
      if (!file) return;
      const controller = new AbortController();
      const slot = { bytes: 0, controller };
      this.inFlight.set(file.path, slot);
      try {
        await this.attempt(file, slot);
        this.inFlight.delete(file.path);
        this.completedBytes += file.size;
        this.filesDone++;
        this.opts.onFileVerified?.(file);
      } catch (e) {
        this.inFlight.delete(file.path);
        if (this.cancelled || this.fatal) return;
        if (e instanceof Interrupted || this.pausedFlag) {
          if (slot.bytes > 0) this.partial.set(file.path, slot.bytes);
          this.queue.unshift(file); // paused: picked up again on resume, from its .part
          continue;
        }
        const disk = isDiskError(e);
        if (disk) {
          this.stopFor(new DownloadError(disk.code, file.path, disk.message));
          return;
        }
        if (e instanceof AttemptError && e.fatal) {
          this.stopFor(e.fatal);
          return;
        }
        if (e instanceof AttemptError && e.busyWaitMs !== undefined && (this.busyWaits.get(file.path) ?? 0) < MAX_BUSY_WAITS) {
          this.busyWaits.set(file.path, (this.busyWaits.get(file.path) ?? 0) + 1);
          if (slot.bytes > 0) this.partial.set(file.path, slot.bytes);
          await this.pausableSleep(e.busyWaitMs);
          this.queue.unshift(file);
          continue;
        }
        const n = (this.attempts.get(file.path) ?? 0) + 1;
        this.attempts.set(file.path, n);
        const reason = e instanceof Error ? e.message : String(e);
        if (n >= this.maxAttempts || (e instanceof AttemptError && !e.retry)) {
          this.stopFor(new DownloadError("download_failed", file.path, reason));
          return;
        }
        this.opts.onRetry?.(file, n, reason);
        await this.pausableSleep(this.backoff(n));
        this.queue.unshift(file);
      }
    }
  }

  // Waits, but returns early on pause, cancel or a fatal error.
  private async pausableSleep(ms: number): Promise<void> {
    const wait = new AbortController();
    const poll = setInterval(() => {
      if (this.pausedFlag || this.cancelled || this.fatal) wait.abort();
    }, 100);
    try {
      await sleep(ms, wait.signal);
    } catch {
      /* interrupted by pause or cancel */
    } finally {
      clearInterval(poll);
    }
  }

  // One try at one file. Resolves once <file> exists with the right size and hash.
  private async attempt(file: ManifestFile, slot: { bytes: number; controller: AbortController }): Promise<void> {
    const signal = slot.controller.signal;
    const dest = resolveInside(this.opts.installDir, file.path);
    const part = dest + ".part";
    await fsp.mkdir(path.dirname(dest), { recursive: true });

    let offset = 0;
    try {
      const st = await fsp.stat(part);
      offset = st.size;
    } catch {
      offset = 0;
    }
    if (offset > file.size) {
      await fsp.unlink(part);
      offset = 0;
    }
    slot.bytes = offset;
    this.partial.delete(file.path);
    // Empty files need no transfer; the hash check below still applies.
    if (file.size === 0) await fsp.writeFile(part, Buffer.alloc(0));

    let hash = createHash("sha256");
    if (offset > 0) await hashExisting(part, hash, signal);

    if (offset < file.size) {
      offset = await this.fetchInto(file, part, offset, hash, slot, (h) => (hash = h));
    }

    const st = await fsp.stat(part);
    const digest = hash.digest("hex");
    if (st.size !== file.size || digest !== file.sha256) {
      await fsp.unlink(part).catch(() => undefined);
      slot.bytes = 0;
      throw new AttemptError(true, `verification failed for ${file.path}`);
    }
    await fsp.rename(part, dest);
  }

  private fetchInto(
    file: ManifestFile,
    part: string,
    offset: number,
    hash: Hash,
    slot: { bytes: number; controller: AbortController },
    replaceHash: (h: Hash) => void,
  ): Promise<number> {
    const signal = slot.controller.signal;
    const cdn = this.downloadBaseUrl !== undefined;
    const url = cdn
      ? new URL(file.path.split("/").map(encodeURIComponent).join("/"), this.downloadBaseUrl)
      : endpointUrl(this.opts.endpoint, fileUrlPath(file.path));
    const headers: Record<string, string> = {
      // Never send account credentials or the gateway Host header to the CDN.
      ...(cdn ? {} : { host: hostHeader(this.opts.endpoint), [REVIVED_KEY_HEADER]: this.opts.key }),
      "accept-encoding": "identity",
    };
    if (offset > 0) {
      headers.range = `bytes=${offset}-`;
      if (!cdn) headers["if-range"] = `"${file.sha256}"`;
    }

    return new Promise<number>((resolve, reject) => {
      let ws: WriteStream | null = null;
      let settled = false;
      let stallTimer: NodeJS.Timeout | null = null;
      const finish = (err: Error | null, value?: number) => {
        if (settled) return;
        settled = true;
        if (stallTimer) clearTimeout(stallTimer);
        signal.removeEventListener("abort", onAbort);
        if (err) req.destroy();
        const close = ws ? closeStream(ws).catch(() => undefined) : Promise.resolve();
        close.then(() => (err ? reject(err) : resolve(value as number)));
      };
      const armStall = () => {
        if (stallTimer) clearTimeout(stallTimer);
        stallTimer = setTimeout(() => finish(new AttemptError(true, "stalled")), this.stallTimeoutMs);
      };
      const onAbort = () => finish(new Interrupted());
      signal.addEventListener("abort", onAbort, { once: true });

      const req = (cdn ? https : http).request(url, { method: "GET", headers, agent: this.agent });
      armStall();
      req.on("error", (e) => {
        if (signal.aborted) return finish(new Interrupted());
        if (isPinError(e)) return finish(new AttemptError(false, "certificate mismatch", new DownloadError("cert_mismatch", file.path)));
        finish(new AttemptError(true, e.message));
      });
      req.on("response", (res) => {
        const status = res.statusCode ?? 0;
        if (status === 401 || status === 403) {
          res.resume();
          return finish(new AttemptError(false, "key rejected", new DownloadError("key_rejected", file.path)));
        }
        if (status === 404) {
          res.resume();
          return finish(new AttemptError(false, "not on server", new DownloadError("file_missing_on_server", file.path)));
        }
        if (status === 416) {
          res.resume();
          fsp.unlink(part).catch(() => undefined).then(() => {
            slot.bytes = 0;
            finish(new AttemptError(true, "range not satisfiable"));
          });
          return;
        }
        if (status === 429 || status === 503) {
          // 429 too_many_streams / 503 server_busy or auth_unavailable: come back later. A file that
          // stays unavailable still runs out of busy waits and then counts as failed.
          res.resume();
          return finish(new AttemptError(true, `HTTP ${status}`, undefined, retryAfterMs(res.headers["retry-after"])));
        }
        if (status !== 200 && status !== 206) {
          res.resume();
          return finish(new AttemptError(true, `HTTP ${status}`));
        }
        const tag = etagHash(res.headers.etag);
        // R2 ETags are MD5/multipart identifiers, not our manifest SHA-256.
        if (!cdn && tag !== null && tag !== file.sha256) {
          res.resume();
          return finish(new AttemptError(false, "server file differs", new DownloadError("file_different_on_server", file.path)));
        }

        let start = offset;
        if (status === 206) {
          const range = parseContentRange(res.headers["content-range"]);
          if (!range || range.start !== offset || range.end !== file.size - 1 || range.total !== file.size) {
            res.resume();
            return finish(new AttemptError(true, "bad content-range"));
          }
        } else if (offset > 0) {
          // The server sent the whole file: start over.
          start = 0;
          const fresh = createHash("sha256");
          replaceHash(fresh);
          hash = fresh;
          slot.bytes = 0;
        }
        const expected = file.size - start;
        const len = res.headers["content-length"];
        if (len !== undefined && Number(len) !== expected) {
          res.resume();
          return finish(new AttemptError(true, "unexpected content-length"));
        }

        ws = createWriteStream(part, { flags: start > 0 ? "a" : "w" });
        ws.on("error", (e) => finish(e));
        let received = 0;
        res.on("data", (chunk: Buffer) => {
          if (settled) return;
          received += chunk.length;
          if (received > expected) {
            return finish(new AttemptError(true, "server sent too much"));
          }
          armStall();
          hash.update(chunk);
          slot.bytes += chunk.length;
          this.speed.add(chunk.length);
          if (!ws!.write(chunk)) {
            res.pause();
            ws!.once("drain", () => res.resume());
          }
        });
        res.on("error", (e) => finish(signal.aborted ? new Interrupted() : new AttemptError(true, e.message)));
        res.on("close", () => {
          if (!res.complete) finish(signal.aborted ? new Interrupted() : new AttemptError(true, "connection closed"));
        });
        res.on("end", () => {
          if (received !== expected) return finish(new AttemptError(true, "connection closed early"));
          finish(null, file.size);
        });
      });
      req.end();
    });
  }
}
