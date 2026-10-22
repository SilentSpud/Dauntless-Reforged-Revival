// The public-mode relay (contract C4) against a local HTTPS "gateway" with a throwaway
// self-signed certificate. Ports: 62401 upstream, 62402-62404 relays, 62409 nothing listening.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import https from "node:https";
import net from "node:net";
import { createHash, randomBytes } from "node:crypto";
import type { Duplex } from "node:stream";
import { filterHeaders, Relay, RelayError } from "../src/main/relay";
import { makeTestCert, type TestCert } from "./certs";

const UPSTREAM_PORT = 62401;
const RELAY_PORT = 62402;
const WRONG_PIN_RELAY_PORT = 62403;
const DOWN_RELAY_PORT = 62404;
const NOTHING_PORT = 62409;

interface Seen {
  method: string;
  url: string;
  rawHeaders: string[];
  headers: http.IncomingHttpHeaders;
}

let cert: TestCert;
let otherCert: TestCert;
let upstream: https.Server;
let relay: Relay;
const seen: Seen[] = [];
let tlsConnections = 0;
const pinErrors: (string | null)[] = [];

function wsAccept(key: string): string {
  return createHash("sha1").update(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").digest("base64");
}

// Minimal WebSocket echo: reads masked client text frames, answers "echo:<text>".
function wsEcho(socket: Duplex): void {
  let buf = Buffer.alloc(0);
  socket.on("data", (chunk: Buffer) => {
    buf = Buffer.concat([buf, chunk]);
    for (;;) {
      if (buf.length < 2) return;
      const opcode = buf[0] & 0x0f;
      let len = buf[1] & 0x7f;
      let off = 2;
      if (len === 126) {
        if (buf.length < 4) return;
        len = buf.readUInt16BE(2);
        off = 4;
      }
      const masked = (buf[1] & 0x80) !== 0;
      if (buf.length < off + (masked ? 4 : 0) + len) return;
      const mask = masked ? buf.subarray(off, off + 4) : null;
      off += masked ? 4 : 0;
      const payload = Buffer.from(buf.subarray(off, off + len));
      if (mask) for (let i = 0; i < payload.length; i++) payload[i] ^= mask[i % 4];
      buf = buf.subarray(off + len);
      if (opcode === 0x8) {
        socket.end(Buffer.from([0x88, 0x00]));
        return;
      }
      if (opcode === 0x1) {
        const out = Buffer.from("echo:" + payload.toString("utf8"));
        socket.write(Buffer.concat([Buffer.from([0x81, out.length]), out]));
      }
    }
  });
}

function readBody(req: http.IncomingMessage): Promise<Buffer> {
  return new Promise((resolve) => {
    const chunks: Buffer[] = [];
    req.on("data", (c: Buffer) => chunks.push(c));
    req.on("end", () => resolve(Buffer.concat(chunks)));
  });
}

before(async () => {
  cert = makeTestCert();
  otherCert = makeTestCert();
  upstream = https.createServer({ cert: cert.certPem, key: cert.keyPem });
  upstream.on("secureConnection", () => tlsConnections++);
  upstream.on("request", async (req, res) => {
    seen.push({ method: req.method ?? "", url: req.url ?? "", rawHeaders: req.rawHeaders, headers: req.headers });
    const url = new URL(req.url ?? "/", "http://x");
    if (url.pathname === "/echo") {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ method: req.method, url: req.url, rawHeaders: req.rawHeaders }));
      return;
    }
    if (url.pathname === "/body") {
      const body = await readBody(req);
      res.writeHead(200, { "content-type": "application/json" });
      res.end(
        JSON.stringify({
          length: body.length,
          sha256: createHash("sha256").update(body).digest("hex"),
          te: req.headers["transfer-encoding"] ?? null,
          cl: req.headers["content-length"] ?? null,
        }),
      );
      return;
    }
    if (url.pathname === "/headers") {
      res.writeHead(200, "Fine Thanks", [
        "Set-Cookie", "a=1; Path=/",
        "Set-Cookie", "b=2; Path=/",
        "X-Custom", "yes",
        "Keep-Alive", "timeout=5",
        "Connection", "x-private",
        "X-Private", "hop-only",
        "Content-Type", "text/plain",
      ]);
      res.end("ok");
      return;
    }
    if (url.pathname === "/teapot") {
      res.writeHead(418, "I'm a teapot", { "content-length": "0" });
      res.end();
      return;
    }
    if (url.pathname === "/head") {
      res.writeHead(200, { "content-length": "1234", "content-type": "application/octet-stream" });
      res.end();
      return;
    }
    if (url.pathname === "/stream") {
      res.writeHead(200, { "content-type": "text/plain" });
      res.write("one|");
      setTimeout(() => res.write("two|"), 300);
      setTimeout(() => res.end("three"), 600);
      return;
    }
    if (url.pathname === "/stream-in") {
      res.writeHead(200, { "content-type": "text/plain" });
      let total = 0;
      let first = true;
      req.on("data", (c: Buffer) => {
        total += c.length;
        if (first) {
          first = false;
          res.write("got-first|");
        }
      });
      req.on("end", () => res.end(`done:${total}`));
      return;
    }
    res.writeHead(404).end();
  });
  upstream.on("upgrade", (req, socket, _head) => {
    seen.push({ method: req.method ?? "", url: req.url ?? "", rawHeaders: req.rawHeaders, headers: req.headers });
    const key = req.headers["sec-websocket-key"];
    // "/ws" for the tests' own client; "//" is the game's chat connection (libwebsockets 3.0)
    if ((req.url !== "/ws" && req.url !== "//") || typeof key !== "string") {
      socket.end("HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
      return;
    }
    const asked = req.headers["sec-websocket-protocol"];
    socket.write(
      "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n" +
        (typeof asked === "string" ? `Sec-WebSocket-Protocol: ${asked.split(",")[0].trim()}\r\n` : "") +
        `Sec-WebSocket-Accept: ${wsAccept(key)}\r\nX-Upstream: yes\r\n\r\n`,
    );
    wsEcho(socket);
  });
  await new Promise<void>((resolve) => upstream.listen(UPSTREAM_PORT, "127.0.0.1", resolve));
  relay = new Relay({ qosPort: null, 
    target: { host: "127.0.0.1", port: UPSTREAM_PORT, fingerprint: cert.fingerprint },
    port: RELAY_PORT,
    onPinError: (actual) => pinErrors.push(actual),
  });
  await relay.start();
});

after(async () => {
  await relay.stop();
  upstream.closeAllConnections();
  await new Promise<void>((resolve) => upstream.close(() => resolve()));
  cert.cleanup();
  otherCert.cleanup();
});

interface Res {
  status: number;
  message: string;
  headers: http.IncomingHttpHeaders;
  rawHeaders: string[];
  body: Buffer;
}

function call(port: number, opts: { method?: string; path: string; headers?: http.OutgoingHttpHeaders | string[]; body?: Buffer | string; agent?: http.Agent }): Promise<Res> {
  return new Promise((resolve, reject) => {
    const req = http.request({ host: "127.0.0.1", port, method: opts.method ?? "GET", path: opts.path, headers: opts.headers, agent: opts.agent ?? false });
    req.on("response", (res) => {
      const chunks: Buffer[] = [];
      res.on("data", (c: Buffer) => chunks.push(c));
      res.on("end", () => resolve({ status: res.statusCode ?? 0, message: res.statusMessage ?? "", headers: res.headers, rawHeaders: res.rawHeaders, body: Buffer.concat(chunks) }));
    });
    req.on("error", reject);
    if (opts.body !== undefined) req.write(opts.body);
    req.end();
  });
}

function headerValues(raw: string[], name: string): string[] {
  const out: string[] = [];
  for (let i = 0; i < raw.length; i += 2) if (raw[i].toLowerCase() === name) out.push(raw[i + 1]);
  return out;
}

test("forwards method, path, query, headers and body over the pinned TLS connection", async () => {
  const body = randomBytes(256 * 1024);
  const before = seen.length;
  const res = await call(RELAY_PORT, {
    method: "POST",
    path: "/body?x=1&y=%20z",
    headers: [
      "Host", `127.0.0.1:${RELAY_PORT}`,
      "Content-Type", "application/octet-stream",
      "Content-Length", String(body.length),
      "X-Test", "a",
      "X-Dup", "one",
      "X-Dup", "two",
      "Authorization", "bearer game-token",
      "Connection", "keep-alive, X-Conn-Named",
      "X-Conn-Named", "must-not-pass",
      "Proxy-Authorization", "Basic abc",
      "TE", "trailers",
    ],
    body,
  });
  assert.equal(res.status, 200);
  const json = JSON.parse(res.body.toString());
  assert.equal(json.length, body.length);
  assert.equal(json.sha256, createHash("sha256").update(body).digest("hex"));
  assert.equal(json.cl, String(body.length));
  const up = seen[before];
  assert.equal(up.method, "POST");
  assert.equal(up.url, "/body?x=1&y=%20z");
  assert.deepEqual(headerValues(up.rawHeaders, "host"), [`127.0.0.1:${UPSTREAM_PORT}`]);
  assert.deepEqual(headerValues(up.rawHeaders, "x-test"), ["a"]);
  assert.deepEqual(headerValues(up.rawHeaders, "x-dup"), ["one", "two"]);
  assert.deepEqual(headerValues(up.rawHeaders, "authorization"), ["bearer game-token"]);
  assert.deepEqual(headerValues(up.rawHeaders, "x-conn-named"), []);
  assert.deepEqual(headerValues(up.rawHeaders, "proxy-authorization"), []);
  assert.deepEqual(headerValues(up.rawHeaders, "te"), []);
});

test("a chunked request body is streamed on as chunked", async () => {
  const parts = [randomBytes(10_000), randomBytes(50_000), randomBytes(3)];
  const all = Buffer.concat(parts);
  const res = await new Promise<Res>((resolve, reject) => {
    const req = http.request({ host: "127.0.0.1", port: RELAY_PORT, method: "POST", path: "/body", agent: false });
    req.on("response", (r) => {
      const chunks: Buffer[] = [];
      r.on("data", (c: Buffer) => chunks.push(c));
      r.on("end", () => resolve({ status: r.statusCode ?? 0, message: "", headers: r.headers, rawHeaders: r.rawHeaders, body: Buffer.concat(chunks) }));
    });
    req.on("error", reject);
    for (const p of parts) req.write(p);
    req.end();
  });
  const json = JSON.parse(res.body.toString());
  assert.equal(json.te, "chunked");
  assert.equal(json.cl, null);
  assert.equal(json.length, all.length);
  assert.equal(json.sha256, createHash("sha256").update(all).digest("hex"));
});

test("response: status message, repeated headers kept, hop-by-hop headers dropped", async () => {
  const res = await call(RELAY_PORT, { path: "/headers" });
  assert.equal(res.status, 200);
  assert.equal(res.message, "Fine Thanks");
  assert.deepEqual(res.headers["set-cookie"], ["a=1; Path=/", "b=2; Path=/"]);
  assert.equal(res.headers["x-custom"], "yes");
  assert.equal(res.headers["x-private"], undefined);
  assert.equal(res.body.toString(), "ok");
  const teapot = await call(RELAY_PORT, { path: "/teapot" });
  assert.equal(teapot.status, 418);
  assert.equal(teapot.message, "I'm a teapot");
});

test("HEAD keeps the content-length and sends no body", async () => {
  const res = await call(RELAY_PORT, { method: "HEAD", path: "/head" });
  assert.equal(res.status, 200);
  assert.equal(res.headers["content-length"], "1234");
  assert.equal(res.body.length, 0);
});

test("responses are streamed, not buffered", async () => {
  const started = Date.now();
  const arrivals: { at: number; text: string }[] = [];
  await new Promise<void>((resolve, reject) => {
    const req = http.request({ host: "127.0.0.1", port: RELAY_PORT, path: "/stream", agent: false });
    req.on("response", (r) => {
      r.on("data", (c: Buffer) => arrivals.push({ at: Date.now() - started, text: c.toString() }));
      r.on("end", () => resolve());
    });
    req.on("error", reject);
    req.end();
  });
  assert.equal(arrivals.map((a) => a.text).join(""), "one|two|three");
  assert.ok(arrivals.length >= 3, `expected separate chunks, got ${arrivals.length}`);
  assert.ok(arrivals[0].at < 280, `first chunk after ${arrivals[0].at} ms`);
  assert.ok(arrivals[arrivals.length - 1].at >= 550, `last chunk after ${arrivals[arrivals.length - 1].at} ms`);
});

test("request bodies are streamed: the server sees the first part before the client finished", async () => {
  const text = await new Promise<string>((resolve, reject) => {
    const req = http.request({ host: "127.0.0.1", port: RELAY_PORT, method: "POST", path: "/stream-in", agent: false });
    let out = "";
    req.on("response", (r) => {
      r.on("data", (c: Buffer) => {
        out += c.toString();
        // Only now, after the server answered the first part, send the rest.
        if (out === "got-first|") req.end("part2");
      });
      r.on("end", () => resolve(out));
    });
    req.on("error", reject);
    req.write("part1");
  });
  assert.equal(text, "got-first|done:10");
});

test("sequential requests reuse one pinned TLS connection (keep-alive)", async () => {
  const agent = new http.Agent({ keepAlive: true, maxSockets: 1 });
  const startCount = tlsConnections;
  for (let i = 0; i < 5; i++) {
    const res = await call(RELAY_PORT, { path: `/echo?i=${i}`, agent });
    assert.equal(res.status, 200);
  }
  agent.destroy();
  assert.ok(tlsConnections - startCount <= 1, `opened ${tlsConnections - startCount} upstream TLS connections`);
});

test("WebSocket upgrades are carried to wss on the server", async () => {
  const before = seen.length;
  const ws = new WebSocket(`ws://127.0.0.1:${RELAY_PORT}/ws`);
  const reply = await new Promise<string>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("no reply")), 5000);
    ws.addEventListener("open", () => ws.send("hello aether"));
    ws.addEventListener("message", (e) => {
      clearTimeout(timer);
      resolve(String(e.data));
    });
    ws.addEventListener("error", () => reject(new Error("websocket error")));
  });
  assert.equal(reply, "echo:hello aether");
  ws.close();
  const up = seen.slice(before).find((s) => s.url === "/ws");
  assert.ok(up, "upstream saw the upgrade");
  assert.equal(up.headers.host, `127.0.0.1:${UPSTREAM_PORT}`);
  assert.equal(String(up.headers.upgrade).toLowerCase(), "websocket");
  assert.match(String(up.headers.connection), /upgrade/i);
  assert.ok(up.headers["sec-websocket-key"]);
});

test("the game's chat connection as libwebsockets 3.0 sends it: GET //, a loopback Origin without a port, protocol xmpp", async () => {
  const before = seen.length;
  const response = await new Promise<string>((resolve, reject) => {
    const sock = net.connect(RELAY_PORT, "127.0.0.1", () => {
      sock.write(
        "GET // HTTP/1.1\r\nPragma: no-cache\r\nCache-Control: no-cache\r\nHost: 127.0.0.1\r\nOrigin: http://127.0.0.1\r\n" +
          "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: xmpp\r\nSec-WebSocket-Version: 13\r\n\r\n",
      );
    });
    let out = "";
    const timer = setTimeout(() => {
      sock.destroy();
      resolve(out);
    }, 3000);
    sock.on("data", (d: Buffer) => {
      out += d.toString("latin1");
      if (out.includes("\r\n\r\n")) {
        clearTimeout(timer);
        sock.destroy();
        resolve(out);
      }
    });
    sock.on("error", reject);
  });
  assert.match(response, /^HTTP\/1\.1 101 /);
  assert.match(response, /\r\nSec-WebSocket-Protocol: xmpp\r\n/i, "the server's protocol comes back");
  const up = seen.slice(before).find((s) => s.url === "//");
  assert.ok(up, "the server saw the request target // unchanged");
  assert.equal(up.headers.host, `127.0.0.1:${UPSTREAM_PORT}`);
  assert.equal(up.headers["sec-websocket-protocol"], "xmpp");
});

test("a server with a different certificate gets nothing: 502, no request, no WebSocket", async () => {
  const wrong = new Relay({ qosPort: null, 
    target: { host: "127.0.0.1", port: UPSTREAM_PORT, fingerprint: otherCert.fingerprint },
    port: WRONG_PIN_RELAY_PORT,
    onPinError: (actual) => pinErrors.push(actual),
  });
  await wrong.start();
  try {
    const before = seen.length;
    pinErrors.length = 0;
    const res = await call(WRONG_PIN_RELAY_PORT, {
      method: "POST",
      path: "/body",
      headers: { "content-type": "application/json", authorization: "bearer secret-token" },
      body: JSON.stringify({ secret: "must not leave the PC" }),
    });
    assert.equal(res.status, 502);
    assert.deepEqual(JSON.parse(res.body.toString()), { error: "certificate_mismatch" });
    assert.equal(seen.length, before, "no HTTP request reached the server");
    assert.deepEqual(pinErrors, [cert.fingerprint], "the error names the certificate that was actually presented");

    const ws = new WebSocket(`ws://127.0.0.1:${WRONG_PIN_RELAY_PORT}/ws`);
    const outcome = await new Promise<string>((resolve) => {
      ws.addEventListener("open", () => resolve("open"));
      ws.addEventListener("error", () => resolve("error"));
      ws.addEventListener("close", () => resolve("closed"));
    });
    assert.notEqual(outcome, "open");
    assert.equal(seen.length, before, "no upgrade reached the server");
  } finally {
    await wrong.stop();
  }
});

test("requests that look like they come from a web page are refused", async () => {
  const before = seen.length;
  const evilHost = await call(RELAY_PORT, { path: "/echo", headers: { host: "evil.example" } });
  assert.equal(evilHost.status, 403);
  const fetchMeta = await call(RELAY_PORT, { path: "/echo", headers: { "sec-fetch-site": "cross-site", "sec-fetch-mode": "no-cors" } });
  assert.equal(fetchMeta.status, 403);
  const origin = await call(RELAY_PORT, { path: "/echo", headers: { origin: "https://evil.example" } });
  assert.equal(origin.status, 403);
  assert.equal(seen.length, before);
  const localOrigin = await call(RELAY_PORT, { path: "/echo", headers: { origin: `http://127.0.0.1:${RELAY_PORT}` } });
  assert.equal(localOrigin.status, 200);
  const localhostName = await call(RELAY_PORT, { path: "/echo", headers: { host: `localhost:${RELAY_PORT}` } });
  assert.equal(localhostName.status, 200);
});

test("a port that is already taken is reported as port_busy", async () => {
  const second = new Relay({ qosPort: null,  target: { host: "127.0.0.1", port: UPSTREAM_PORT, fingerprint: cert.fingerprint }, port: RELAY_PORT });
  await assert.rejects(second.start(), (e: unknown) => e instanceof RelayError && e.code === "port_busy");
  // Something that is not a relay at all holding the port.
  const squatter = net.createServer();
  await new Promise<void>((resolve) => squatter.listen(DOWN_RELAY_PORT, "127.0.0.1", resolve));
  const third = new Relay({ qosPort: null,  target: { host: "127.0.0.1", port: UPSTREAM_PORT, fingerprint: cert.fingerprint }, port: DOWN_RELAY_PORT });
  await assert.rejects(third.start(), (e: unknown) => e instanceof RelayError && e.code === "port_busy");
  await new Promise<void>((resolve) => squatter.close(() => resolve()));
});

test("a server that is down gives 502 upstream_unreachable, and stop() frees the port", async () => {
  const down = new Relay({ qosPort: null,  target: { host: "127.0.0.1", port: NOTHING_PORT, fingerprint: cert.fingerprint }, port: DOWN_RELAY_PORT, connectTimeoutMs: 2000 });
  await down.start();
  const res = await call(DOWN_RELAY_PORT, { path: "/anything" });
  assert.equal(res.status, 502);
  assert.deepEqual(JSON.parse(res.body.toString()), { error: "upstream_unreachable" });
  await down.stop();
  assert.equal(down.running, false);
  await assert.rejects(call(DOWN_RELAY_PORT, { path: "/anything" }));
  // The port can be used again.
  const again = new Relay({ qosPort: null,  target: { host: "127.0.0.1", port: NOTHING_PORT, fingerprint: cert.fingerprint }, port: DOWN_RELAY_PORT });
  await again.start();
  await again.stop();
});

test("filterHeaders drops hop-by-hop and connection-named headers, keeps upgrades when asked", () => {
  const raw = ["Host", "a", "Connection", "Upgrade, X-Named", "Upgrade", "websocket", "X-Named", "1", "Keep-Alive", "timeout=5", "X-Keep", "2", "Transfer-Encoding", "chunked"];
  assert.deepEqual(filterHeaders(raw, { host: "server:443" }), ["Host", "server:443", "X-Keep", "2"]);
  assert.deepEqual(filterHeaders(raw, { host: "server", keepUpgrade: true }), ["Host", "server", "Upgrade", "websocket", "X-Keep", "2", "Connection", "Upgrade"]);
});

test("a WebSocket from a web page (foreign Origin or Host) is refused before it reaches the server", async () => {
  const before = seen.length;
  const handshake = (headers: string) =>
    new Promise<string>((resolve, reject) => {
      const sock = net.connect(RELAY_PORT, "127.0.0.1", () => {
        sock.write(`GET /ws HTTP/1.1\r\n${headers}Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n`);
      });
      let out = "";
      sock.on("data", (d: Buffer) => (out += d.toString("latin1")));
      sock.on("close", () => resolve(out));
      sock.on("error", reject);
      setTimeout(() => sock.destroy(), 3000);
    });
  const foreignOrigin = await handshake(`Host: 127.0.0.1:${RELAY_PORT}\r\nOrigin: https://evil.example\r\n`);
  assert.match(foreignOrigin, /^HTTP\/1\.1 403 /);
  const rebound = await handshake(`Host: evil.example:${RELAY_PORT}\r\nOrigin: http://evil.example:${RELAY_PORT}\r\n`);
  assert.match(rebound, /^HTTP\/1\.1 403 /);
  assert.equal(seen.length, before);
});
