// Records what the Node content server answers to a request script (<name>.requests.json), for the
// Rust content server's parity test (tests/parity.rs; MIGRATION_PLAN.md RS2).
//
//   cd legacy/ReforgedContent && npm ci && npx tsc -p tsconfig.test.json
//   node ../../rust/services/content/tests/parity/record.cjs <name>
//
// A script builds a runtime folder ("runtime": {"files": name -> {"text"} or {"hex"} or {"pattern":
// length}, "index": runtime.json as an object or text, else one made from the files}, or null for no
// CONTENT_RUNTIME_DIR), an art pack ("branding": {"files", "json"}) and news ("news"); "keys" says what
// the fake backend answers to GetUserInfo for each key. Headers are [name, value] pairs, and "$etag"
// in a value is the last ETag a reply carried. A request {"write": name, "spec"} replaces a runtime
// file instead. Requests go out as raw HTTP, the path exactly as written. Kept in step with
// tests/parity.rs.
"use strict";
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const net = require("node:net");
const http = require("node:http");
const crypto = require("node:crypto");

const name = process.argv[2];
if (!name) throw new Error("usage: record.cjs <name>");
const here = __dirname;
const script = JSON.parse(fs.readFileSync(path.join(here, `${name}.requests.json`), "utf8"));
const content = path.resolve(here, "../../../../../legacy/ReforgedContent");
const build = path.join(content, "build", "src");

// Kept in step with bytes_of() in tests/parity.rs.
function bytesOf(spec) {
  if (spec.text !== undefined) return Buffer.from(spec.text, "utf8");
  if (spec.hex !== undefined) return Buffer.from(spec.hex, "hex");
  return Buffer.from(Array.from({ length: spec.pattern }, (_, i) => (i * 7 + 3) & 0xff));
}

// Every file's modification time, so ETags made from it are the same on both servers.
const MTIME = new Date(1700000000000);

function writeFile(full, data) {
  fs.mkdirSync(path.dirname(full), { recursive: true });
  fs.writeFileSync(full, data);
  fs.utimesSync(full, MTIME, MTIME);
}

const text = (value) => (typeof value === "string" ? value : JSON.stringify(value));

// Kept in step with fixture() in tests/parity.rs.
function fixture(dir) {
  if (script.runtime) {
    const runtime = path.join(dir, "runtime");
    fs.mkdirSync(runtime);
    const files = Object.entries(script.runtime.files ?? {}).map(([file, spec]) => [file, bytesOf(spec)]);
    for (const [file, data] of files) writeFile(path.join(runtime, file), data);
    const index = script.runtime.index ?? {
      version: "20261014-1a2b3c4d",
      build: { changelist: 647472 },
      files: files.map(([file, data]) => ({ name: file, size: data.length, sha256: crypto.createHash("sha256").update(data).digest("hex") })),
    };
    // "$sha:<file>" in an index given as text is that file's SHA-256
    const sha = Object.fromEntries(files.map(([file, data]) => [file, crypto.createHash("sha256").update(data).digest("hex")]));
    writeFile(path.join(runtime, "runtime.json"), text(index).replace(/\$sha:([\w.]+)/g, (_, file) => sha[file]));
  }
  if (script.branding) {
    const art = path.join(dir, "art");
    fs.mkdirSync(art);
    for (const [file, spec] of Object.entries(script.branding.files ?? {})) writeFile(path.join(art, file), bytesOf(spec));
    if (script.branding.json !== undefined) writeFile(path.join(art, "branding.json"), text(script.branding.json));
  }
  if (script.news !== undefined) writeFile(path.join(dir, "news.json"), text(script.news));
}

// The last ETag a reply carried: "$etag" in a request header stands for it.
let lastEtag = "";

// One raw HTTP/1.1 exchange on its own connection: the reply's status, headers and body.
function raw(port, request) {
  return new Promise((resolve, reject) => {
    const socket = net.connect(port, "127.0.0.1");
    const chunks = [];
    socket.on("data", (chunk) => chunks.push(chunk));
    socket.on("error", reject);
    socket.on("end", () => resolve(Buffer.concat(chunks)));
    const headers = (request.headers ?? []).map(([k, v]) => `${k}: ${v.replace("$etag", lastEtag)}\r\n`).join("");
    socket.write(`${request.method ?? "GET"} ${request.path} HTTP/1.1\r\nhost: content\r\n${headers}connection: close\r\n\r\n`);
  });
}

// The CORS and range headers are listed so that their absence is checked too.
const HEADERS = ["content-type", "content-length", "content-range", "etag", "accept-ranges", "cache-control", "retry-after", "allow",
  "access-control-allow-origin", "cross-origin-resource-policy", "x-content-type-options"];

function parse(reply, label) {
  const end = reply.indexOf("\r\n\r\n");
  const head = reply.subarray(0, end).toString("latin1").split("\r\n");
  const status = Number(head[0].split(" ")[1]);
  const headers = {};
  for (const line of head.slice(1)) {
    const at = line.indexOf(":");
    const key = line.slice(0, at).trim().toLowerCase();
    if (HEADERS.includes(key)) headers[key] = line.slice(at + 1).trim();
  }
  const body = reply.subarray(end + 4);
  const shown = body.toString("utf8");
  const plain = body.length <= 4096 && Buffer.from(shown, "utf8").equals(body) && !/[\x00-\x08]/.test(shown)
    ? shown
    : `<${body.length} bytes sha256 ${crypto.createHash("sha256").update(body).digest("hex")}>`;
  return { request: label, status, headers: Object.fromEntries(HEADERS.filter((h) => h in headers).map((h) => [h, headers[h]])), body: plain };
}

async function main() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "content-parity-"));
  fixture(dir);
  const backend = http.createServer((req, res) => {
    const known = req.url === "/reforged/api/GetUserInfo";
    const answer = (known && (script.keys ?? {})[req.headers["x-reforged-user-api-key"]]) || { status: 401, body: {} };
    res.writeHead(answer.status, { "content-type": "application/json" });
    res.end(text(answer.body));
  });
  await new Promise((resolve) => backend.listen(0, "127.0.0.1", resolve));
  const env = {
    BACKEND_URL: `http://127.0.0.1:${backend.address().port}`,
    ...(script.runtime ? { CONTENT_RUNTIME_DIR: path.join(dir, "runtime") } : {}),
    ...(script.branding ? { CONTENT_BRANDING_DIR: path.join(dir, "art") } : {}),
    ...(script.news !== undefined ? { CONTENT_NEWS_FILE: path.join(dir, "news.json") } : {}),
    ...script.env,
  };
  const { LoadConfig } = require(path.join(build, "config.js"));
  const { CreateContentHandler } = require(path.join(build, "app.js"));
  const { AuthCache, BackendLookup } = require(path.join(build, "auth.js"));
  const { StreamLimiter } = require(path.join(build, "limits.js"));
  const { Branding } = require(path.join(build, "branding.js"));
  const { News } = require(path.join(build, "news.js"));
  const { RuntimeStore } = require(path.join(build, "runtime.js"));
  const { SetLogSink } = require(path.join(build, "log.js"));
  SetLogSink(() => {});
  const config = LoadConfig(env);
  const handler = CreateContentHandler({
    runtime: new RuntimeStore(config.runtimeDir),
    auth: new AuthCache({ lookup: BackendLookup(config.backendUrl), ttlMs: config.authCacheSeconds * 1000 }),
    limiter: new StreamLimiter(config.maxStreamsPerAccount, config.maxStreamsTotal),
    branding: new Branding(config.brandingDir),
    news: new News(config.newsFile, Date.now, undefined),
  });
  const server = http.createServer(handler);
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const results = [];
  try {
    for (const request of script.requests) {
      if (request.write !== undefined) {
        writeFile(path.join(dir, "runtime", request.write), bytesOf(request.spec));
        continue;
      }
      const reply = await raw(server.address().port, request);
      const result = parse(reply, `${request.method ?? "GET"} ${request.path}`);
      if (result.headers.etag !== undefined) lastEtag = result.headers.etag;
      results.push(result);
    }
  } finally {
    server.close();
    backend.close();
  }
  const out = path.join(here, `${name}.expected.json`);
  fs.writeFileSync(out, JSON.stringify({ about: script.about, results }, null, 2) + "\n");
  console.log(`${path.basename(out)}: ${results.length} replies`);
  fs.rmSync(dir, { recursive: true, force: true });
  process.exit(0);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
