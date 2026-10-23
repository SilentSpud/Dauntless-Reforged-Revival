// TLS connections to a public-mode server, pinned to the certificate fingerprint from the invite.
//
// The server's certificate is self-signed, so there is no CA to check. Instead the handshake is
// finished first, the SHA-256 of the certificate the server presented (its DER bytes) is compared
// with the invite's fingerprint, and only a matching socket is handed out. Nothing, not even a
// request line, is written to a socket before that check passed.

import http from "node:http";
import net from "node:net";
import tls from "node:tls";
import { createHash } from "node:crypto";
import type { Duplex } from "node:stream";

export interface PinnedTarget {
  host: string;
  port: number;
  fingerprint: string; // 64 lower-case hex
}

export class CertificatePinError extends Error {
  readonly code = "CERT_PIN_MISMATCH";
  constructor(public readonly actual: string | null) {
    super("the server's certificate does not match the invite");
    this.name = "CertificatePinError";
  }
}

export class ConnectTimeoutError extends Error {
  readonly code = "ETIMEDOUT";
  constructor() {
    super("connection timed out");
    this.name = "ConnectTimeoutError";
  }
}

export function certificateFingerprint(der: Buffer): string {
  return createHash("sha256").update(der).digest("hex");
}

// The DER bytes of the certificate the peer presented, or null.
export function peerCertificateDer(socket: tls.TLSSocket): Buffer | null {
  try {
    const x509 = socket.getPeerX509Certificate();
    if (x509) return x509.raw;
  } catch {
    /* fall back below */
  }
  const cert = socket.getPeerCertificate(false);
  return cert && Buffer.isBuffer(cert.raw) && cert.raw.length > 0 ? cert.raw : null;
}

export interface PinnedConnectOptions {
  timeoutMs?: number;
  signal?: AbortSignal;
}

export function pinnedConnect(target: PinnedTarget, opts: PinnedConnectOptions = {}): Promise<tls.TLSSocket> {
  return new Promise((resolve, reject) => {
    if (opts.signal?.aborted) return reject(new Error("aborted"));
    let settled = false;
    const socket = tls.connect({
      host: target.host,
      port: target.port,
      // SNI only for names; an IP address must not be sent as a server name.
      servername: net.isIP(target.host) === 0 ? target.host : undefined,
      // The certificate is checked by its fingerprint below, not against a CA or a host name.
      rejectUnauthorized: false,
      checkServerIdentity: () => undefined,
      minVersion: "TLSv1.2",
      ALPNProtocols: ["http/1.1"],
    });
    const finish = (err: Error | null) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      opts.signal?.removeEventListener("abort", onAbort);
      socket.removeListener("error", onError);
      if (err) {
        socket.destroy();
        reject(err);
      } else {
        resolve(socket);
      }
    };
    const onError = (e: Error) => finish(e);
    const onAbort = () => finish(new Error("aborted"));
    const timer = setTimeout(() => finish(new ConnectTimeoutError()), opts.timeoutMs ?? 10000);
    opts.signal?.addEventListener("abort", onAbort, { once: true });
    socket.once("error", onError);
    socket.once("secureConnect", () => {
      const der = peerCertificateDer(socket);
      const actual = der ? certificateFingerprint(der) : null;
      if (actual !== target.fingerprint) return finish(new CertificatePinError(actual));
      // Errors after this point belong to whoever uses the socket.
      socket.on("error", () => undefined);
      finish(null);
    });
  });
}

type CreateConnectionCallback = (err: Error | null, stream: Duplex) => void;
const NO_SOCKET = undefined as unknown as Duplex;

// An http.Agent whose sockets are pinned TLS connections. Requests made with it are plain HTTP/1.1
// over the verified TLS socket (keep-alive and pooling work as usual).
export class PinnedAgent extends http.Agent {
  constructor(
    private readonly target: PinnedTarget,
    options: http.AgentOptions = {},
    private readonly connectTimeoutMs = 10000,
  ) {
    super(options);
  }

  // http.Agent calls this with a callback and accepts an asynchronous answer.
  createConnection(_options: http.ClientRequestArgs, callback?: CreateConnectionCallback): Duplex | null | undefined {
    pinnedConnect(this.target, { timeoutMs: this.connectTimeoutMs }).then(
      (socket) => callback?.(null, socket),
      (e: Error) => callback?.(e, NO_SOCKET),
    );
    return undefined;
  }
}

// For a single request without an agent (http.request's createConnection option).
export function pinnedCreateConnection(target: PinnedTarget, timeoutMs = 10000) {
  return (_options: unknown, callback: CreateConnectionCallback): undefined => {
    pinnedConnect(target, { timeoutMs }).then(
      (socket) => callback(null, socket),
      (e: Error) => callback(e, NO_SOCKET),
    );
    return undefined;
  };
}

export function isPinError(e: unknown): e is CertificatePinError {
  return e instanceof CertificatePinError || (e as { code?: unknown })?.code === "CERT_PIN_MISMATCH";
}
