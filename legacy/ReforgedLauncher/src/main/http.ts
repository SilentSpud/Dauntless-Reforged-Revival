// Requests to the host from the invite, and nowhere else. Redirects are never followed, so a
// request that carries the account key cannot be bounced to another address.
//
// Private mode (invite v1): plain HTTP over Tailscale, which encrypts the traffic itself.
// Public mode (invite v2): HTTP/1.1 over TLS to the server's gateway, pinned to the certificate
// fingerprint from the invite (see pinned.ts).

import http from "node:http";
import { createWriteStream, type WriteStream } from "node:fs";
import { isPrivateModeHost, isValidFingerprint, isValidHost } from "../shared/invite";
import { isPinError, pinnedCreateConnection, type PinnedTarget } from "./pinned";

export interface Endpoint {
  host: string;
  port: number;
  pin: string | null; // certificate fingerprint for public mode, null for plain HTTP
}

export class HttpError extends Error {
  constructor(
    public readonly kind: "timeout" | "network" | "too_large" | "aborted" | "bad_url" | "pin",
    message: string,
  ) {
    super(message);
    this.name = "HttpError";
  }
}

export interface HttpResponse {
  status: number;
  headers: http.IncomingHttpHeaders;
  body: Buffer;
}

export interface RequestOptions {
  method?: "GET" | "POST" | "HEAD";
  headers?: Record<string, string>;
  body?: string;
  timeoutMs: number;
  maxBytes: number;
  signal?: AbortSignal;
}

export function checkEndpoint(ep: Endpoint): void {
  if (!isValidHost(ep.host) || !Number.isInteger(ep.port) || ep.port < 1 || ep.port > 65535) {
    throw new HttpError("bad_url", "invalid endpoint");
  }
  if (ep.pin !== null && !isValidFingerprint(ep.pin)) throw new HttpError("bad_url", "invalid certificate fingerprint");
  // Plain HTTP (private mode) only into the tailnet or to this PC; anything else must be pinned TLS.
  if (ep.pin === null && !isPrivateModeHost(ep.host)) throw new HttpError("bad_url", "plain HTTP only to Tailscale or loopback");
}

// The URL handed to http.request. For pinned endpoints the TLS socket comes from pinned.ts, so the
// scheme here only describes the HTTP layer.
export function endpointUrl(ep: Endpoint, pathAndQuery: string): string {
  checkEndpoint(ep);
  if (!pathAndQuery.startsWith("/")) throw new HttpError("bad_url", "path must start with /");
  return `http://${ep.host}:${ep.port}${pathAndQuery}`;
}

export function hostHeader(ep: Endpoint): string {
  return ep.pin !== null && ep.port === 443 ? ep.host : `${ep.host}:${ep.port}`;
}

export function pinnedTarget(ep: Endpoint): PinnedTarget | null {
  return ep.pin === null ? null : { host: ep.host, port: ep.port, fingerprint: ep.pin };
}

export function request(ep: Endpoint, pathAndQuery: string, opts: RequestOptions): Promise<HttpResponse> {
  const url = endpointUrl(ep, pathAndQuery);
  const target = pinnedTarget(ep);
  return new Promise((resolve, reject) => {
    let settled = false;
    const done = (fn: () => void) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      opts.signal?.removeEventListener("abort", onAbort);
      fn();
    };
    const headers: Record<string, string> = { accept: "application/json", host: hostHeader(ep), ...(opts.headers ?? {}) };
    if (opts.body !== undefined) {
      headers["content-type"] = "application/json";
      headers["content-length"] = String(Buffer.byteLength(opts.body));
    }
    const req = target
      ? http.request(url, { method: opts.method ?? "GET", headers, createConnection: pinnedCreateConnection(target, opts.timeoutMs) as never })
      : http.request(url, { method: opts.method ?? "GET", headers, agent: false });
    const timer = setTimeout(() => {
      req.destroy();
      done(() => reject(new HttpError("timeout", "request timed out")));
    }, opts.timeoutMs);
    const onAbort = () => {
      req.destroy();
      done(() => reject(new HttpError("aborted", "request aborted")));
    };
    if (opts.signal) {
      if (opts.signal.aborted) {
        onAbort();
        return;
      }
      opts.signal.addEventListener("abort", onAbort);
    }
    req.on("response", (res) => {
      const chunks: Buffer[] = [];
      let size = 0;
      res.on("data", (chunk: Buffer) => {
        size += chunk.length;
        if (size > opts.maxBytes) {
          req.destroy();
          done(() => reject(new HttpError("too_large", "response too large")));
          return;
        }
        chunks.push(chunk);
      });
      res.on("end", () => done(() => resolve({ status: res.statusCode ?? 0, headers: res.headers, body: Buffer.concat(chunks) })));
      res.on("error", (e) => done(() => reject(new HttpError("network", e.message))));
    });
    req.on("error", (e) => done(() => reject(isPinError(e) ? new HttpError("pin", e.message) : new HttpError("network", e.message))));
    if (opts.body !== undefined) req.write(opts.body);
    req.end();
  });
}

export function parseJsonBody(res: HttpResponse): unknown {
  const type = String(res.headers["content-type"] ?? "");
  if (!type.includes("json") && res.body.length === 0) return undefined;
  try {
    return JSON.parse(res.body.toString("utf8"));
  } catch {
    return undefined;
  }
}

export interface FileRequestOptions {
  headers?: Record<string, string>;
  timeoutMs: number; // no data for this long ends the request
  maxBytes: number;
  signal?: AbortSignal;
  onBytes?: (n: number) => void;
}

// GET into a file, streamed: for the runtime files, which are larger than a JSON answer. The same
// rules as request(): only the invite's host, pinned TLS in public mode, no redirects. Resolves
// with the HTTP status; the file holds the body only for a 200.
export function requestToFile(ep: Endpoint, pathAndQuery: string, dest: string, opts: FileRequestOptions): Promise<number> {
  const url = endpointUrl(ep, pathAndQuery);
  const target = pinnedTarget(ep);
  return new Promise((resolve, reject) => {
    let settled = false;
    let out: WriteStream | null = null;
    const done = (fn: () => void) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      opts.signal?.removeEventListener("abort", onAbort);
      fn();
    };
    const fail = (e: Error) => {
      req.destroy();
      if (out) out.destroy();
      done(() => reject(e));
    };
    const headers: Record<string, string> = { accept: "application/octet-stream", host: hostHeader(ep), ...(opts.headers ?? {}) };
    const req = target
      ? http.request(url, { method: "GET", headers, createConnection: pinnedCreateConnection(target, opts.timeoutMs) as never })
      : http.request(url, { method: "GET", headers, agent: false });
    let timer = setTimeout(() => fail(new HttpError("timeout", "request timed out")), opts.timeoutMs);
    const touch = () => {
      clearTimeout(timer);
      timer = setTimeout(() => fail(new HttpError("timeout", "download stalled")), opts.timeoutMs);
    };
    const onAbort = () => fail(new HttpError("aborted", "request aborted"));
    if (opts.signal) {
      if (opts.signal.aborted) return onAbort();
      opts.signal.addEventListener("abort", onAbort);
    }
    req.on("response", (res) => {
      touch();
      if (res.statusCode !== 200) {
        res.resume();
        res.on("end", () => done(() => resolve(res.statusCode ?? 0)));
        return;
      }
      const declared = Number(res.headers["content-length"] ?? NaN);
      if (Number.isFinite(declared) && declared > opts.maxBytes) return fail(new HttpError("too_large", "response too large"));
      out = createWriteStream(dest);
      let size = 0;
      res.on("data", (chunk: Buffer) => {
        touch();
        size += chunk.length;
        if (size > opts.maxBytes) return fail(new HttpError("too_large", "response too large"));
        opts.onBytes?.(chunk.length);
      });
      res.on("error", (e) => fail(new HttpError("network", e.message)));
      out.on("error", (e) => fail(e));
      out.on("finish", () => done(() => resolve(200)));
      res.on("aborted", () => fail(new HttpError("network", "connection closed")));
      res.pipe(out);
    });
    req.on("error", (e) => fail(isPinError(e) ? new HttpError("pin", e.message) : new HttpError("network", e.message)));
    req.end();
  });
}
