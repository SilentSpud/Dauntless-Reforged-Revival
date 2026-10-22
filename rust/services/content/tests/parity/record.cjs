// Records what the Node content server answers to a request script (<name>.requests.json), for the
// Rust content server's parity test (tests/parity.rs; MIGRATION_PLAN.md S1).
//
//   cd legacy/UndauntedContent && npm ci && npx tsc -p tsconfig.test.json
//   node ../../rust/services/content/tests/parity/record.cjs <name>
//
// A script builds a game folder ("files": path -> {"text"} or {"pattern": length}), a manifest of it,
// then removes or changes some files ("gone", "resized"), adds files the manifest doesn't list
// ("extra"), an art pack ("branding": {"files", "json"}) and news ("news"); "keys" says what the fake
// metagame answers to GetUserInfo for each key. Requests go out as raw HTTP, the path exactly as
// written. Kept in step with tests/parity.rs.
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
const content = path.resolve(here, "../../../../../legacy/UndauntedContent");
const build = path.join(content, "build", "src");

// Kept in step with fill() in tests/parity.rs.
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

function fixture(dir) {
  const game = path.join(dir, "game");
  fs.mkdirSync(game);
  const files = Object.entries(script.files).map(([rel, spec]) => [rel, bytesOf(spec)]);
  for (const [rel, data] of files) writeFile(path.join(game, ...rel.split("/")), data);
  files.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  const manifest = {
    build: script.build ?? "dauntless_rel-1.4.4_Shipping_2020-10-28_20-11-15_239827",
    totalBytes: files.reduce((sum, [, data]) => sum + data.length, 0),
    files: files.map(([rel, data]) => ({ path: rel, size: data.length, sha256: crypto.createHash("sha256").update(data).digest("hex") })),
  };
  fs.writeFileSync(path.join(dir, "manifest.json"), JSON.stringify(manifest));
  for (const rel of script.gone ?? []) fs.rmSync(path.join(game, ...rel.split("/")));
  for (const [rel, spec] of Object.entries(script.resized ?? {})) writeFile(path.join(game, ...rel.split("/")), bytesOf(spec));
  for (const [rel, spec] of Object.entries(script.extra ?? {})) writeFile(path.join(game, ...rel.split("/")), bytesOf(spec));
  if (script.branding) {
    const art = path.join(dir, "art");
    fs.mkdirSync(art);
    for (const [file, spec] of Object.entries(script.branding.files ?? {})) writeFile(path.join(art, file), bytesOf(spec));
    if (script.branding.json !== undefined) writeFile(path.join(art, "branding.json"), typeof script.branding.json === "string" ? script.branding.json : JSON.stringify(script.branding.json));
  }
  if (script.news !== undefined) writeFile(path.join(dir, "news.json"), typeof script.news === "string" ? script.news : JSON.stringify(script.news));
}

// One raw HTTP/1.1 exchange on its own connection: the reply's status, headers and body.
function raw(port, request) {
  return new Promise((resolve, reject) => {
    const socket = net.connect(port, "127.0.0.1");
    const chunks = [];
    socket.on("data", (chunk) => chunks.push(chunk));
    socket.on("error", reject);
    socket.on("end", () => resolve(Buffer.concat(chunks)));
    const headers = Object.entries(request.headers ?? {}).map(([k, v]) => `${k}: ${v}\r\n`).join("");
    socket.write(`${request.method ?? "GET"} ${request.path} HTTP/1.1\r\nhost: content\r\n${headers}connection: close\r\n\r\n`);
  });
}

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
  const text = body.toString("utf8");
  const shown = body.length <= 4096 && Buffer.from(text, "utf8").equals(body) && !/[\x00-\x08]/.test(text)
    ? text
    : `<${body.length} bytes sha256 ${crypto.createHash("sha256").update(body).digest("hex")}>`;
  return { request: label, status, headers: Object.fromEntries(HEADERS.filter((h) => h in headers).map((h) => [h, headers[h]])), body: shown };
}

async function main() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "content-parity-"));
  fixture(dir);
  const metagame = http.createServer((req, res) => {
    const answer = (script.keys ?? {})[req.headers["x-undaunted-user-api-key"]] ?? { status: 401, body: {} };
    res.writeHead(answer.status, { "content-type": "application/json" });
    res.end(typeof answer.body === "string" ? answer.body : JSON.stringify(answer.body));
  });
  await new Promise((resolve) => metagame.listen(0, "127.0.0.1", resolve));
  const env = {
    CONTENT_GAME_DIR: path.join(dir, "game"),
    CONTENT_MANIFEST: path.join(dir, "manifest.json"),
    METAGAME_URL: `http://127.0.0.1:${metagame.address().port}`,
    ...(script.branding ? { CONTENT_BRANDING_DIR: path.join(dir, "art") } : {}),
    ...(script.news !== undefined ? { CONTENT_NEWS_FILE: path.join(dir, "news.json") } : {}),
    ...script.env,
  };
  const { LoadConfig } = require(path.join(build, "config.js"));
  const { LoadManifest, IndexManifest } = require(path.join(build, "manifest.js"));
  const { CheckGameDir } = require(path.join(build, "gamedir.js"));
  const { CreateContentHandler } = require(path.join(build, "app.js"));
  const { AuthCache, MetagameLookup } = require(path.join(build, "auth.js"));
  const { StreamLimiter } = require(path.join(build, "limits.js"));
  const { Branding } = require(path.join(build, "branding.js"));
  const { News } = require(path.join(build, "news.js"));
  const { SetLogSink } = require(path.join(build, "log.js"));
  SetLogSink(() => {});
  const config = LoadConfig(env);
  const manifest = LoadManifest(config.manifestPath);
  const report = CheckGameDir(config.gameDir, manifest);
  const handler = CreateContentHandler({
    downloadBaseUrl: config.downloadBaseUrl,
    manifest,
    index: IndexManifest(manifest),
    gameDir: config.gameDir,
    states: report.states,
    auth: new AuthCache({ lookup: MetagameLookup(config.metagameUrl), ttlMs: config.authCacheSeconds * 1000 }),
    limiter: new StreamLimiter(config.maxStreamsPerAccount, config.maxStreamsTotal),
    branding: new Branding(config.brandingDir),
    news: new News(config.newsFile, Date.now, undefined),
  });
  const server = http.createServer(handler);
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const results = [];
  try {
    for (const request of script.requests) {
      if (request.touch !== undefined) {
        const file = path.join(dir, "game", ...request.touch.split("/"));
        fs.utimesSync(file, new Date(1700000001000), new Date(1700000001000));
        continue;
      }
      const reply = await raw(server.address().port, request);
      results.push(parse(reply, `${request.method ?? "GET"} ${request.path}`));
    }
  } finally {
    server.close();
    metagame.close();
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
