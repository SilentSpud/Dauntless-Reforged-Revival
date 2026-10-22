// The game manifest: the exact list of Dauntless 1.4.4 files, their sizes and SHA-256 hashes, from
// Dauntless Revived's launcher 0.1.35 (src/main/manifest.ts there; the list itself is its
// UndauntedContent/data/dauntless-1.4.4.json, copied here). The launcher only ever writes files that are
// in it, and refuses a content server whose manifest is not identical.

import { createHash } from "node:crypto";
import path from "node:path";
import raw from "./dauntless-1.4.4.json";
import { REVIVED_BUILD as EXPECTED_BUILD, REVIVED_DLL_RELATIVE_PATHS as DLL_RELATIVE_PATHS, REVIVED_EXE_RELATIVE_PATH as EXE_RELATIVE_PATH, REVIVED_EXE_SHA256 as PINNED_EXE_SHA256, REVIVED_EXE_SIZE as PINNED_EXE_SIZE } from "./constants";

export interface ManifestFile {
  path: string;
  size: number;
  sha256: string;
}

export interface GameManifest {
  build: string;
  totalBytes: number;
  files: ManifestFile[];
}

export type ManifestCheck = { ok: true; manifest: GameManifest } | { ok: false; reason: string };

const RESERVED_NAMES = /^(con|prn|aux|nul|com[0-9]|lpt[0-9]|conin\$|conout\$)(\..*)?$/i;

// A manifest path is relative, uses forward slashes, and cannot climb out of the install folder
// or name anything Windows treats specially (drive letters, streams, device names).
export function isSafeRelativePath(p: unknown): p is string {
  if (typeof p !== "string" || p.length === 0 || p.length > 240) return false;
  if (p.startsWith("/") || p.endsWith("/")) return false;
  const segments = p.split("/");
  for (const s of segments) {
    if (s.length === 0 || s === "." || s === "..") return false;
    if (!/^[A-Za-z0-9_.+() -]+$/.test(s)) return false;
    if (s.startsWith(" ") || s.endsWith(" ") || s.endsWith(".")) return false;
    if (RESERVED_NAMES.test(s)) return false;
  }
  return true;
}

// Joins a checked manifest path onto the install folder and makes sure the result stays inside it.
export function resolveInside(root: string, rel: string): string {
  if (!isSafeRelativePath(rel)) throw new Error("unsafe path");
  const base = path.resolve(root);
  const full = path.resolve(base, ...rel.split("/"));
  const back = path.relative(base, full);
  if (back === "" || back.startsWith("..") || path.isAbsolute(back)) throw new Error("path escapes install folder");
  return full;
}

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

export function validateManifest(raw: unknown, expectedBuild: string = EXPECTED_BUILD, requireGameExe = true): ManifestCheck {
  if (!isObject(raw)) return { ok: false, reason: "not an object" };
  if (raw.build !== expectedBuild) return { ok: false, reason: "wrong build" };
  if (!Array.isArray(raw.files) || raw.files.length === 0) return { ok: false, reason: "no files" };
  if (!Number.isSafeInteger(raw.totalBytes) || (raw.totalBytes as number) < 0) return { ok: false, reason: "bad totalBytes" };

  const seen = new Set<string>();
  const files: ManifestFile[] = [];
  let sum = 0;
  for (const f of raw.files) {
    if (!isObject(f)) return { ok: false, reason: "bad entry" };
    if (!isSafeRelativePath(f.path)) return { ok: false, reason: `unsafe path: ${String(f.path).slice(0, 120)}` };
    if (!Number.isSafeInteger(f.size) || (f.size as number) < 0) return { ok: false, reason: `bad size: ${f.path}` };
    if (typeof f.sha256 !== "string" || !/^[0-9a-f]{64}$/.test(f.sha256)) return { ok: false, reason: `bad sha256: ${f.path}` };
    const key = f.path.toLowerCase(); // Windows file names are case-insensitive
    if (seen.has(key)) return { ok: false, reason: `duplicate: ${f.path}` };
    seen.add(key);
    if (DLL_RELATIVE_PATHS.some((d) => d.toLowerCase() === key)) return { ok: false, reason: `pinned DLL listed: ${f.path}` };
    files.push({ path: f.path, size: f.size as number, sha256: f.sha256 });
    sum += f.size as number;
  }
  if (sum !== raw.totalBytes) return { ok: false, reason: "totalBytes does not match the files" };

  if (requireGameExe) {
    const exe = files.find((f) => f.path === EXE_RELATIVE_PATH);
    if (!exe || exe.sha256 !== PINNED_EXE_SHA256 || exe.size !== PINNED_EXE_SIZE) {
      return { ok: false, reason: "game executable is not the pinned 1.4.4 build" };
    }
  }

  files.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
  return { ok: true, manifest: { build: raw.build as string, totalBytes: sum, files } };
}

export type ManifestComparison = { equal: true } | { equal: false; difference: string };

// The server's manifest must be exactly the compiled-in one: same build, same files, same sizes,
// same hashes. Order does not matter.
export function compareManifests(expected: GameManifest, actual: unknown): ManifestComparison {
  const check = validateManifest(actual, expected.build, false);
  if (!check.ok) return { equal: false, difference: `server manifest invalid (${check.reason})` };
  const other = check.manifest;
  if (other.totalBytes !== expected.totalBytes) return { equal: false, difference: "total size differs" };
  if (other.files.length !== expected.files.length) return { equal: false, difference: "file count differs" };
  const byPath = new Map(other.files.map((f) => [f.path, f]));
  for (const f of expected.files) {
    const o = byPath.get(f.path);
    if (!o) return { equal: false, difference: `missing on server: ${f.path}` };
    if (o.size !== f.size) return { equal: false, difference: `size differs: ${f.path}` };
    if (o.sha256 !== f.sha256) return { equal: false, difference: `hash differs: ${f.path}` };
  }
  return { equal: true };
}

export function manifestFingerprint(m: GameManifest): string {
  const canonical = [...m.files]
    .sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0))
    .map((f) => `${f.path}\t${f.size}\t${f.sha256}`)
    .join("\n");
  return createHash("sha256").update(`${m.build}\n${canonical}`).digest("hex");
}

const check = validateManifest(raw);
// The compiled-in list (null only if the copy above were damaged: then nothing is installed).
export const GAME_MANIFEST: GameManifest | null = check.ok ? check.manifest : null;
