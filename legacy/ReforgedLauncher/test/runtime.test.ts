// The server's game runtime: its manifest, the trust check, the install with hash verification,
// and the local runtime a host can use instead.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { parseRuntimeManifest, fetchRuntimeManifest, type RuntimeManifest } from "../src/main/hostapi";
import { checkRuntime, installLocalRuntime, installRuntime, isCompleteRuntimeDir, replaceFile, RuntimeError, runtimeSetId, sweepAside, type ReplaceOps } from "../src/main/runtime-files";
import { promises as fsp } from "node:fs";
import type { Endpoint } from "../src/main/http";
import { FakeServer, makeRuntime, runtimeManifestFor, sha256 } from "./fakes";

const KEY = "RRK_" + "a".repeat(48);
const runtime = makeRuntime(1);
let server: FakeServer;
let ep: Endpoint;
const dirs: string[] = [];

function tempDir(): string {
  const d = mkdtempSync(path.join(tmpdir(), "rr-runtime-"));
  dirs.push(d);
  return d;
}

function manifest(files = runtime): RuntimeManifest {
  const m = parseRuntimeManifest(runtimeManifestFor(files));
  assert.ok(m);
  return m;
}

before(async () => {
  server = new FakeServer({ existingUsers: new Map([["aurora", KEY]]), runtime });
  ep = { host: "127.0.0.1", port: await server.start(), pin: null };
});

after(async () => {
  await server.stop();
  for (const d of dirs) rmSync(d, { recursive: true, force: true });
});

test("manifest: only the two known files, each once, with a plain SHA-256 and a sane size", () => {
  const good = runtimeManifestFor(runtime);
  assert.ok(parseRuntimeManifest(good));
  assert.equal(parseRuntimeManifest({ ...good, files: good.files.slice(0, 1) }), null, "both files are needed");
  assert.equal(parseRuntimeManifest({ ...good, files: [...good.files, good.files[0]] }), null, "no duplicates");
  assert.equal(parseRuntimeManifest({ ...good, files: [{ ...good.files[0], name: "..\\evil.dll" }, good.files[1]] }), null);
  assert.equal(parseRuntimeManifest({ ...good, files: [{ ...good.files[0], name: "dxgi.dll" }, good.files[1]] }), null);
  assert.equal(parseRuntimeManifest({ ...good, files: [{ ...good.files[0], sha256: "ABC" }, good.files[1]] }), null);
  assert.equal(parseRuntimeManifest({ ...good, files: [{ ...good.files[0], size: 0 }, good.files[1]] }), null);
  assert.equal(parseRuntimeManifest({ ...good, files: [{ ...good.files[0], size: 1e12 }, good.files[1]] }), null);
  assert.equal(parseRuntimeManifest({ ...good, version: "" }), null);
  assert.equal(parseRuntimeManifest([]), null);
  const m = parseRuntimeManifest({ ...good, files: [...good.files].reverse() });
  assert.deepEqual(m?.files.map((f) => f.name), ["MysticParadox.dll", "winmm.dll"]);
});

test("runtimeSetId names the exact set of files, in any order", () => {
  const m = manifest();
  assert.equal(runtimeSetId(m.files), runtimeSetId([...m.files].reverse()));
  assert.notEqual(runtimeSetId(m.files), runtimeSetId(manifest(makeRuntime(2)).files));
  assert.match(runtimeSetId(m.files), /^[0-9a-f]{64}$/);
});

test("fetchRuntimeManifest: ok, none when the host has not published one, bad_answer for junk", async () => {
  const ok = await fetchRuntimeManifest(ep);
  assert.equal(ok.kind, "ok");
  const empty = new FakeServer({ runtime: null });
  const junk = new FakeServer({ runtimeOverride: { version: "x", files: [{ name: "evil.exe", size: 1, sha256: "0".repeat(64) }] } });
  try {
    assert.deepEqual(await fetchRuntimeManifest({ host: "127.0.0.1", port: await empty.start(), pin: null }), { kind: "none" });
    assert.deepEqual(await fetchRuntimeManifest({ host: "127.0.0.1", port: await junk.start(), pin: null }), { kind: "bad_answer" });
  } finally {
    await empty.stop();
    await junk.stop();
  }
});

test("checkRuntime: untrusted, missing, outdated, ok, unavailable and local", async () => {
  const win64 = tempDir();
  const m = manifest();
  assert.deepEqual(await checkRuntime(win64, m, false, null), { state: "untrusted", playable: false });
  assert.deepEqual(await checkRuntime(win64, m, true, null), { state: "missing", playable: false });
  assert.deepEqual(await checkRuntime(win64, null, false, null), { state: "unavailable", playable: false });
  for (const f of runtime) writeFileSync(path.join(win64, f.name), f.data);
  assert.deepEqual(await checkRuntime(win64, m, false, null), { state: "ok", playable: true }, "the exact files are already there");
  assert.deepEqual(await checkRuntime(win64, null, false, null), { state: "ok", playable: true }, "one runtime build serves every server");
  const newer = manifest(makeRuntime(3));
  assert.deepEqual(await checkRuntime(win64, newer, false, null), { state: "untrusted", playable: false }, "a changed runtime asks again");
  assert.deepEqual(await checkRuntime(win64, newer, true, null), { state: "outdated", playable: false });
  const local = tempDir();
  assert.deepEqual(await checkRuntime(win64, m, true, local), { state: "local", playable: false });
  for (const f of makeRuntime(4)) writeFileSync(path.join(local, f.name), f.data);
  assert.deepEqual(await checkRuntime(win64, m, true, local), { state: "local", playable: true });
});

test("installRuntime: downloads with the key, checks every file, writes the marker, skips files already right", async () => {
  const win64 = tempDir();
  const progress: number[] = [];
  server.seen.length = 0;
  await installRuntime({ endpoint: ep, key: KEY, manifest: manifest(), win64Dir: win64, onProgress: (p) => progress.push(p.filesDone) });
  for (const f of runtime) assert.equal(sha256(readFileSync(path.join(win64, f.name))), sha256(f.data));
  assert.ok(existsSync(path.join(win64, ".reforged-runtime.json")));
  assert.equal(readdirSync(win64).filter((n) => n.endsWith(".rr-download")).length, 0);
  assert.equal(progress[progress.length - 1], 2);
  const downloads = server.seen.filter((s) => s.path.startsWith("/content/v1/runtime/files/"));
  assert.equal(downloads.length, 2);
  assert.ok(downloads.every((d) => d.key === KEY));
  server.seen.length = 0;
  await installRuntime({ endpoint: ep, key: KEY, manifest: manifest(), win64Dir: win64 });
  assert.equal(server.seen.length, 0, "nothing downloaded when the files already match");
});

test("installRuntime: a file that does not match the list never replaces the old one", async () => {
  const win64 = tempDir();
  writeFileSync(path.join(win64, "MysticParadox.dll"), "old build");
  const bad = new FakeServer({ existingUsers: new Map([["aurora", KEY]]), runtime, corruptRuntime: true });
  try {
    const badEp = { host: "127.0.0.1", port: await bad.start(), pin: null };
    await assert.rejects(installRuntime({ endpoint: badEp, key: KEY, manifest: manifest(), win64Dir: win64 }), (e: unknown) => e instanceof RuntimeError && e.code === "different");
    assert.equal(readFileSync(path.join(win64, "MysticParadox.dll"), "utf8"), "old build");
    assert.equal(readdirSync(win64).filter((n) => n.endsWith(".rr-download")).length, 0);
  } finally {
    await bad.stop();
  }
});

test("installRuntime: an unknown key is refused (key_rejected)", async () => {
  const win64 = tempDir();
  await assert.rejects(installRuntime({ endpoint: ep, key: "RRK_" + "9".repeat(48), manifest: manifest(), win64Dir: win64 }), (e: unknown) => e instanceof RuntimeError && e.code === "key_rejected");
});

test("installLocalRuntime copies a host's own build next to the game", async () => {
  const local = tempDir();
  const win64 = tempDir();
  assert.equal(await isCompleteRuntimeDir(local), false);
  const files = makeRuntime(7);
  for (const f of files) writeFileSync(path.join(local, f.name), f.data);
  assert.equal(await isCompleteRuntimeDir(local), true);
  mkdirSync(win64, { recursive: true });
  await installLocalRuntime(local, win64);
  for (const f of files) assert.ok(readFileSync(path.join(win64, f.name)).equals(f.data));
});

// Windows: a DLL a running process has loaded can be renamed but not overwritten. These ops act like
// that for the files in `loaded`.
function windowsLikeOps(loaded: Set<string>): ReplaceOps {
  return {
    rename: async (from, to) => {
      if (loaded.has(to)) throw Object.assign(new Error(`EPERM: operation not permitted, rename '${from}' -> '${to}'`), { code: "EPERM" });
      await fsp.rename(from, to);
      if (loaded.has(from)) {
        loaded.delete(from);
        loaded.add(to);
      }
    },
    rm: async (file, opts) => {
      if (loaded.has(file)) throw Object.assign(new Error(`EPERM: operation not permitted, unlink '${file}'`), { code: "EPERM" });
      await fsp.rm(file, opts);
    },
  };
}

test("replaceFile: a loaded runtime file is moved aside and the new one takes its place", async () => {
  const dir = tempDir();
  const target = path.join(dir, "MysticParadox.dll");
  writeFileSync(target, "old build");
  writeFileSync(`${target}.rr-download`, "new build");
  const loaded = new Set([target]);
  assert.equal(await replaceFile(`${target}.rr-download`, target, windowsLikeOps(loaded)), "in_use");
  assert.equal(readFileSync(target, "utf8"), "new build");
  const aside = readdirSync(dir).filter((n) => n.startsWith("MysticParadox.dll.rr-old-"));
  assert.equal(aside.length, 1, "the old file stays while it is loaded");
  assert.equal(readFileSync(path.join(dir, aside[0]), "utf8"), "old build");
  await sweepAside(dir);
  assert.deepEqual(readdirSync(dir), ["MysticParadox.dll"], "swept once nothing holds it");
});

test("replaceFile: a free file is replaced in place; a file that can't be moved aside is kept", async () => {
  const dir = tempDir();
  const target = path.join(dir, "winmm.dll");
  writeFileSync(target, "old");
  writeFileSync(`${target}.rr-download`, "new");
  assert.equal(await replaceFile(`${target}.rr-download`, target), "replaced");
  assert.equal(readFileSync(target, "utf8"), "new");
  writeFileSync(`${target}.rr-download`, "newer");
  const locked: ReplaceOps = {
    rename: async () => {
      throw Object.assign(new Error("EBUSY"), { code: "EBUSY" });
    },
    rm: (f, o) => fsp.rm(f, o),
  };
  await assert.rejects(replaceFile(`${target}.rr-download`, target, locked), /EBUSY/);
  assert.equal(readFileSync(target, "utf8"), "new");
});

test("installRuntime clears old runtime files left aside by an earlier install", async () => {
  const win64 = tempDir();
  writeFileSync(path.join(win64, "MysticParadox.dll.rr-old-123"), "old");
  writeFileSync(path.join(win64, "notes.txt.rr-old-1"), "not ours");
  const r = await installRuntime({ endpoint: ep, key: KEY, manifest: manifest(), win64Dir: win64 });
  assert.deepEqual(r.replacedInUse, []);
  assert.equal(existsSync(path.join(win64, "MysticParadox.dll.rr-old-123")), false);
  assert.equal(existsSync(path.join(win64, "notes.txt.rr-old-1")), true);
});
