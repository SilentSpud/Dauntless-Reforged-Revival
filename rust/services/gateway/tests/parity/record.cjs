// Records what the Node gateway answers to a request script (<name>.requests.json), for the Rust
// gateway's parity test (tests/parity.rs; MIGRATION_PLAN.md S2).
//
//   cd legacy/UndauntedGateway && npm ci && npx tsc -p tsconfig.test.json
//   node ../../rust/services/gateway/tests/parity/record.cjs <name>
//
// Each request is raw HTTP/1.1 text sent on its own TLS connection (from "local" when given); an
// upgrade that gets its 101 then sends "send" and reads the echo. Recorded per request: the status,
// some headers, the body (parsed when JSON), and the access-log lines it produced (without the time
// and duration). The fake upstreams are kept in step with tests/parity.rs.
"use strict";
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const net = require("node:net");
const tls = require("node:tls");
const http = require("node:http");
const crypto = require("node:crypto");
const { spawnSync } = require("node:child_process");

const name = process.argv[2];
if (!name) throw new Error("usage: record.cjs <name>");
const here = __dirname;
const script = JSON.parse(fs.readFileSync(path.join(here, `${name}.requests.json`), "utf8"));
const gatewayDir = path.resolve(here, "../../../../../legacy/UndauntedGateway");
const build = path.join(gatewayDir, "build", "src");
const { Gateway } = require(path.join(build, "gateway.js"));
const { AllowlistFeed } = require(path.join(build, "feed.js"));
const { LoadGatewayConfig } = require(path.join(build, "config.js"));
const { SetLogSink, SetLogLevel } = require(path.join(build, "log.js"));

const SECRET = "parity-gateway-secret-0123456789abcdef0123456789abcdef";
const HELPER_SECRET = "parity-helper-secret-0123456789abcdef0123456789abcdef";
const WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// Kept in step with echo() in tests/parity.rs: what an upstream saw, headers sorted, without the
// connection header (each side's HTTP client sets its own).
function echo(upstream, req, body) {
  const headers = {};
  for (const key of Object.keys(req.headers).sort()) {
    if (key !== "connection") headers[key] = req.headers[key];
  }
  return JSON.stringify({ upstream, method: req.method, url: req.url, headers, bodyLength: body.length, bodySha: crypto.createHash("sha256").update(body).digest("hex") });
}

function listen(server) {
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));
}

function upstream(label, handle) {
  return http.createServer((req, res) => {
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));
    req.on("error", () => undefined);
    req.on("end", () => {
      const body = Buffer.concat(chunks);
      if (handle && handle(req, res, body)) return;
      const text = echo(label, req, body);
      res.writeHead(200, { "content-type": "application/json", "x-powered-by": "Express", "content-length": Buffer.byteLength(text) });
      res.end(text);
    });
  });
}

// Kept in step with metagame() in tests/parity.rs.
const metagame = upstream("metagame", (req, res) => {
  const pathOnly = req.url.split("?")[0];
  const status = /^\/status\/(\d+)$/.exec(pathOnly);
  if (status) {
    const code = Number(status[1]);
    res.writeHead(code, { "content-type": "text/plain", "x-powered-by": "Express", "x-dauntless-gateway": "leak", "set-cookie": ["a=1", "b=2"] });
    res.end(code === 204 ? undefined : `status ${code}`);
    return true;
  }
  if (pathOnly === "/heartbeat") {
    const ok = req.headers["authorization"] === "Bearer good-token";
    res.writeHead(ok ? 200 : 401, { "content-type": "text/plain", "content-length": ok ? 5 : 0 });
    res.end(ok ? "20000" : "");
    return true;
  }
  if (pathOnly === "/account/api/oauth/token") {
    res.writeHead(200, { "content-type": "application/json", "content-length": 20 });
    res.end('{"access_token":"x"}');
    return true;
  }
  return false;
});
const content = upstream("content");
const allowed = [];
const helper = upstream("helper", (req, res, body) => {
  allowed.push(JSON.parse(body.toString("utf8")).ip);
  res.writeHead(200, { "content-type": "application/json", "content-length": 2 });
  res.end("{}");
  return true;
});

// Kept in step with ws_upstream() in tests/parity.rs.
const ws = http.createServer((_req, res) => {
  res.writeHead(426);
  res.end();
});
ws.on("upgrade", (req, socket) => {
  socket.on("error", () => undefined);
  socket.on("end", () => socket.end());
  if (req.url === "/refuse") {
    socket.end("HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 7\r\nConnection: close\r\n\r\nno room");
    return;
  }
  const accept = crypto.createHash("sha1").update(String(req.headers["sec-websocket-key"]) + WS_GUID).digest("base64");
  const asked = req.headers["sec-websocket-protocol"];
  const protocol = typeof asked === "string" ? `Sec-WebSocket-Protocol: ${asked.split(",")[0].trim()}\r\n` : "";
  socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n${protocol}Sec-WebSocket-Accept: ${accept}\r\n\r\n`);
  socket.on("data", (chunk) => socket.write(Buffer.concat([Buffer.from("echo:"), chunk])));
});

// Kept in step with HEADERS in tests/parity.rs.
const HEADERS = ["content-type", "content-length", "transfer-encoding", "cache-control", "connection", "retry-after", "x-powered-by", "x-dauntless-gateway", "set-cookie", "upgrade", "sec-websocket-accept", "sec-websocket-protocol"];

// Kept in step with parse() in tests/parity.rs.
function parse(label, data) {
  const text = data.toString("latin1");
  const end = text.indexOf("\r\n\r\n");
  if (end < 0) return { request: label, status: 0, headers: {}, body: text };
  const lines = text.slice(0, end).split("\r\n");
  const status = Number(lines[0].split(" ")[1]);
  const found = {};
  for (const line of lines.slice(1)) {
    const colon = line.indexOf(":");
    const key = line.slice(0, colon).trim().toLowerCase();
    const value = line.slice(colon + 1).trim();
    found[key] = found[key] === undefined ? value : `${found[key]}, ${value}`;
  }
  const headers = {};
  for (const key of HEADERS) if (found[key] !== undefined) headers[key] = key === "connection" || key === "upgrade" ? found[key].toLowerCase() : found[key];
  let body = data.subarray(Buffer.byteLength(text.slice(0, end + 4), "latin1"));
  if ((found["transfer-encoding"] ?? "").toLowerCase() === "chunked") {
    const parts = [];
    let at = 0;
    while (at < body.length) {
      const lineEnd = body.indexOf("\r\n", at);
      const size = parseInt(body.subarray(at, lineEnd).toString("latin1"), 16);
      if (!(size > 0)) break;
      parts.push(body.subarray(lineEnd + 2, lineEnd + 2 + size));
      at = lineEnd + 2 + size + 2;
    }
    body = Buffer.concat(parts);
  }
  const bodyText = body.toString("utf8");
  let parsed = bodyText;
  try { parsed = JSON.parse(bodyText); } catch { /* not JSON */ }
  return { request: label, status, headers, body: parsed };
}

// One request on its own TLS connection: everything read until the gateway closes (or 3 s).
function exchange(port, step) {
  return new Promise((resolve) => {
    const raw = net.connect({ host: "127.0.0.1", port, localAddress: step.local });
    raw.on("error", () => undefined);
    const socket = tls.connect({ socket: raw, rejectUnauthorized: false });
    const chunks = [];
    let sent = false;
    let finished = false;
    const finish = () => {
      if (finished) return;
      finished = true;
      socket.destroy();
      resolve(Buffer.concat(chunks));
    };
    socket.on("secureConnect", () => socket.write(step.raw));
    socket.on("data", (chunk) => {
      chunks.push(chunk);
      const all = Buffer.concat(chunks).toString("latin1");
      if (step.send && !sent && all.startsWith("HTTP/1.1 101") && all.includes("\r\n\r\n")) {
        sent = true;
        socket.write(step.send);
        setTimeout(finish, 300);
      }
    });
    socket.on("error", finish);
    socket.on("close", finish);
    setTimeout(finish, 3000);
  });
}

async function main() {
  const lines = [];
  SetLogLevel("info");
  SetLogSink((raw) => lines.push(JSON.parse(raw)));
  const ports = {
    metagame: await listen(metagame),
    content: await listen(content),
    ws: await listen(ws),
    helper: await listen(helper),
  };
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "dr-gateway-parity-"));
  const made = spawnSync(process.execPath, [path.join(gatewayDir, "tools", "make-cert.js"), "--host", "127.0.0.1", "--out", dir, "--json"], { encoding: "utf8" });
  if (made.status !== 0) throw new Error(made.stderr);
  const cert = JSON.parse(made.stdout);
  const config = LoadGatewayConfig({
    GATEWAY_BIND: "127.0.0.1",
    GATEWAY_PORT: "1",
    GATEWAY_CERT: cert.cert,
    GATEWAY_KEY: cert.key,
    GATEWAY_SECRET: SECRET,
    GATEWAY_METAGAME_URL: `http://127.0.0.1:${ports.metagame}`,
    GATEWAY_CONTENT_URL: `http://127.0.0.1:${ports.content}`,
    GATEWAY_WS_URL: `http://127.0.0.1:${ports.ws}`,
    ALLOWLIST_URL: `http://127.0.0.1:${ports.helper}`,
    ALLOWLIST_SECRET: HELPER_SECRET,
    ...script.env,
  });
  config.port = 0;
  const feed = new AllowlistFeed(config.allowlist);
  const gateway = new Gateway(config, { cert: fs.readFileSync(cert.cert), key: fs.readFileSync(cert.key) }, feed);
  await gateway.Listen();
  const port = gateway.Server.address().port;

  const results = [];
  for (const step of script.requests) {
    const start = lines.length;
    const data = await exchange(port, step);
    await sleep(100);
    const result = parse(step.label, data);
    result.log = lines.slice(start)
      .filter((line) => line.msg === "request" || line.msg === "websocket closed")
      .map(({ t, ms, ...rest }) => rest);
    results.push(result);
  }
  await feed.Idle();
  results.push({ request: "allowlist feed", status: 0, headers: {}, body: [...allowed].sort(), log: [] });

  await gateway.Close(500);
  for (const server of [metagame, content, ws, helper]) {
    server.closeAllConnections?.();
    server.close();
  }
  fs.rmSync(dir, { recursive: true, force: true });
  SetLogSink(undefined);
  fs.writeFileSync(path.join(here, `${name}.expected.json`), JSON.stringify({ results }, null, 1) + "\n");
  console.log(`${name}: ${results.length} replies`);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
