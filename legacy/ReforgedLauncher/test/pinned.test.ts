// Public mode: ServerStatus, Register, GetUserInfo, the runtime manifest and the runtime downloads
// all go to the gateway over TLS pinned to the invite's certificate fingerprint, and nothing at all
// is sent to a server whose certificate does not match.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { CertificatePinError, pinnedConnect } from "../src/main/pinned";
import { request, HttpError, type Endpoint } from "../src/main/http";
import { fetchRuntimeManifest, fetchServerStatus, fetchUserInfo, registerAccount } from "../src/main/hostapi";
import { installRuntime, RuntimeError } from "../src/main/runtime-files";
import { FakeServer, makeRuntime } from "./fakes";
import { makeTestCert, type TestCert } from "./certs";

const KEY = "RRK_" + "e".repeat(48);

let cert: TestCert;
let other: TestCert;
let server: FakeServer;
let port = 0;
const runtime = makeRuntime();

const ep = (pin?: string): Endpoint => ({ host: "127.0.0.1", port, pin: pin ?? cert.fingerprint });

before(async () => {
  cert = makeTestCert();
  other = makeTestCert();
  server = new FakeServer({ validCodes: new Set(["GOOD-CODE"]), existingUsers: new Map([["aurora", KEY]]), runtime });
  port = await server.start(0, { cert: cert.certPem, key: cert.keyPem });
});

after(async () => {
  await server.stop();
  cert.cleanup();
  other.cleanup();
});

test("pinnedConnect accepts the pinned certificate and refuses any other", async () => {
  const sock = await pinnedConnect({ host: "127.0.0.1", port, fingerprint: cert.fingerprint });
  assert.equal(sock.encrypted, true);
  sock.destroy();
  await assert.rejects(
    pinnedConnect({ host: "127.0.0.1", port, fingerprint: other.fingerprint }),
    (e: unknown) => e instanceof CertificatePinError && e.actual === cert.fingerprint,
  );
});

test("requests over the pin; a mismatch is an HttpError of kind pin", async () => {
  const res = await request(ep(), "/dauntless-status", { timeoutMs: 5000, maxBytes: 65536 });
  assert.equal(res.status, 200);
  await assert.rejects(request(ep(other.fingerprint), "/dauntless-status", { timeoutMs: 5000, maxBytes: 65536 }), (e: unknown) => e instanceof HttpError && e.kind === "pin");
  // Plain HTTP to a TLS port is not mistaken for a pin failure.
  await assert.rejects(request({ ...ep(), pin: null }, "/dauntless-status", { timeoutMs: 3000, maxBytes: 65536 }), (e: unknown) => e instanceof HttpError && e.kind !== "pin");
});

test("ServerStatus: the full list with the key, the limited view without, cert_mismatch with another certificate", async () => {
  const anonymous = await fetchServerStatus(ep());
  assert.ok(anonymous.kind === "ok" && anonymous.status.limited && anonymous.status.players.length === 0);
  const full = await fetchServerStatus(ep(), { key: KEY });
  assert.ok(full.kind === "ok");
  if (full.kind === "ok") {
    assert.equal(full.status.limited, false);
    assert.deepEqual(full.status.players.map((p) => p.name), ["Aurora"]);
    assert.deepEqual(full.status.game, { build: "rel-1.14.7-Archon", changelist: 647472 });
    assert.equal(full.status.gameData, "real");
  }
  const seenBefore = server.seen.length;
  assert.deepEqual(await fetchServerStatus(ep(other.fingerprint), { key: KEY }), { kind: "cert_mismatch" });
  assert.equal(server.seen.length, seenBefore, "a mismatching server never sees a request");
  // A public address over plain HTTP is refused before connecting.
  assert.deepEqual(await fetchServerStatus({ host: "203.0.113.10", port, pin: null }, { key: KEY, timeoutMs: 2000 }), { kind: "unreachable" });
});

test("ServerStatus: an older server (404) is recognised through /dauntless-status, which never gets the key", async () => {
  const seen: { url: string; key: unknown }[] = [];
  const legacy = http.createServer((req, res) => {
    seen.push({ url: req.url ?? "", key: req.headers["x-reforged-user-api-key"] });
    if (req.url === "/dauntless-status") {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ "show-status": true }));
      return;
    }
    res.writeHead(404).end();
  });
  await new Promise<void>((resolve) => legacy.listen(0, "127.0.0.1", resolve));
  const legacyPort = (legacy.address() as { port: number }).port;
  try {
    assert.deepEqual(await fetchServerStatus({ host: "127.0.0.1", port: legacyPort, pin: null }, { key: KEY }), { kind: "unsupported" });
    assert.deepEqual(seen, [
      { url: "/reforged/api/ServerStatus", key: KEY },
      { url: "/dauntless-status", key: undefined },
    ]);
  } finally {
    legacy.closeAllConnections();
    await new Promise<void>((resolve) => legacy.close(() => resolve()));
  }
});

test("Register and GetUserInfo over the pin; nothing is sent to a mismatching server", async () => {
  const before = server.registrations.length;
  assert.deepEqual(await registerAccount(ep(other.fingerprint), "Borealis", "GOOD-CODE"), { ok: false, error: "cert_mismatch" });
  assert.equal(server.registrations.length, before);
  const r = await registerAccount(ep(), "Borealis", "GOOD-CODE");
  assert.equal(r.ok, true);
  if (!r.ok) return;
  assert.match(r.key, /^RRK_[0-9a-f]{48}$/);
  const info = await fetchUserInfo(ep(), r.key);
  assert.ok(info.ok && info.info.username === "Borealis");
  assert.deepEqual(await fetchUserInfo(ep(other.fingerprint), r.key), { ok: false, error: "cert_mismatch" });
  // The same code twice: refused (used up).
  assert.deepEqual(await registerAccount(ep(), "Cirrus", "GOOD-CODE"), { ok: false, error: "invite_invalid" });
  assert.deepEqual(await registerAccount(ep(), "borealis", "X"), { ok: false, error: "username_taken" });
});

test("runtime manifest and download over the pin; refused before any request with another certificate", async () => {
  const m = await fetchRuntimeManifest(ep());
  assert.equal(m.kind, "ok");
  if (m.kind !== "ok") return;
  assert.equal(m.manifest.changelist, 647472);
  assert.deepEqual(await fetchRuntimeManifest(ep(other.fingerprint)), { kind: "cert_mismatch" });
  const dir = mkdtempSync(path.join(tmpdir(), "rr-rt-tls-"));
  try {
    const seenBefore = server.seen.length;
    await assert.rejects(installRuntime({ endpoint: ep(other.fingerprint), key: KEY, manifest: m.manifest, win64Dir: dir }), (e: unknown) => e instanceof RuntimeError && e.code === "cert_mismatch");
    assert.equal(server.seen.length, seenBefore, "the key never reached the mismatching server");
    await installRuntime({ endpoint: ep(), key: KEY, manifest: m.manifest, win64Dir: dir });
    for (const f of runtime) assert.ok(readFileSync(path.join(dir, f.name)).equals(f.data), f.name);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
