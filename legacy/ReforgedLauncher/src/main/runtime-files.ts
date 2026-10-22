// The game runtime: MysticParadox.dll (Mystic Paradox's runtime with the Reforged patches) and
// winmm.dll (the proxy that loads it at startup), next to the game executable.
//
// Dauntless Revived pins its two DLLs inside the launcher. For 1.14.7 the runtime is built by each
// host from their own SDK (the SDK cannot be shared), so the runtime comes from the server instead:
// its content server lists the two files with their sizes and SHA-256, and the launcher
//   - asks the player once before it installs a server's runtime (and again whenever it changes),
//   - downloads each file over the same pinned connection as everything else,
//   - checks size and SHA-256 before a file replaces the old one, and again before every launch.
// A host can also point the launcher at a local folder with a runtime they built themselves.

import { createHash } from "node:crypto";
import { promises as fsp } from "node:fs";
import path from "node:path";
import type { Endpoint } from "./http";
import { HttpError, requestToFile } from "./http";
import { keyHeaders, type RuntimeManifest } from "./hostapi";
import { hashFile } from "./verify";
import { RUNTIME_FILE_NAMES, RUNTIME_MARKER_FILE } from "./constants";
import type { RuntimeFileInfo, RuntimeState } from "../shared/types";

// One id for a set of runtime files: what the player trusts, and what the marker file records.
export function runtimeSetId(files: readonly RuntimeFileInfo[]): string {
  const lines = [...files].sort((a, b) => (a.name < b.name ? -1 : 1)).map((f) => `${f.name}:${f.size}:${f.sha256}`);
  return createHash("sha256").update(lines.join("\n")).digest("hex");
}

// Clears the "downloaded from the internet" mark (the Zone.Identifier stream), like Unblock-File.
export async function unblock(file: string): Promise<void> {
  try {
    await fsp.unlink(`${file}:Zone.Identifier`);
  } catch {
    /* no mark, or not NTFS */
  }
}

// The hashes of the runtime files in a folder, by name (null when a file is missing). Hashing two
// small DLLs is cheap, so it is done afresh every time.
export async function installedRuntime(dir: string): Promise<Map<string, string | null>> {
  const out = new Map<string, string | null>();
  for (const name of RUNTIME_FILE_NAMES) {
    try {
      const st = await fsp.stat(path.join(dir, name));
      out.set(name, st.isFile() && st.size > 0 ? await hashFile(path.join(dir, name)) : null);
    } catch {
      out.set(name, null);
    }
  }
  return out;
}

export interface RuntimeCheck {
  state: RuntimeState;
  // True when the files next to the game may be used for a launch as they are.
  playable: boolean;
}

// What the runtime next to the game looks like against the server's list.
//  manifest: the server's list, or null when the server offers none (its host has not built one)
//  trusted: whether the player accepted this exact list for this server
//  localDir: a local runtime folder in use instead (always playable when complete)
export async function checkRuntime(win64Dir: string, manifest: RuntimeManifest | null, trusted: boolean, localDir: string | null): Promise<RuntimeCheck> {
  if (localDir !== null) {
    const local = await installedRuntime(localDir);
    return { state: "local", playable: [...local.values()].every((h) => h !== null) };
  }
  const installed = await installedRuntime(win64Dir);
  const present = [...installed.values()].filter((h) => h !== null).length;
  if (manifest === null) {
    // A runtime installed earlier still works: one build serves every server (the backend address
    // comes from the launcher at run time).
    return present === RUNTIME_FILE_NAMES.length ? { state: "ok", playable: true } : { state: "unavailable", playable: false };
  }
  const matches = manifest.files.every((f) => installed.get(f.name) === f.sha256);
  if (matches) return { state: "ok", playable: true };
  if (!trusted) return { state: "untrusted", playable: false };
  return { state: present === 0 ? "missing" : "outdated", playable: false };
}

export class RuntimeError extends Error {
  constructor(
    public readonly code: "key_rejected" | "not_found" | "different" | "download_failed" | "cert_mismatch" | "disk_error" | "in_use" | "cancelled",
    public readonly file: string | null,
    message: string,
  ) {
    super(message);
    this.name = "RuntimeError";
  }
}

export interface RuntimeProgress {
  doneBytes: number;
  totalBytes: number;
  filesDone: number;
  filesTotal: number;
  currentFile: string | null;
}

// Puts a checked file in place of the old one. Windows refuses to overwrite a DLL that a running
// process has loaded, and on a host's own PC the Director's game servers (Ramsgate, the Training
// Grounds, hunts) run the same Dauntless executable from the same folder, so they hold both runtime
// files for as long as the server is up. Windows does let a loaded file be renamed, though: when the
// plain replace fails, the old file is moved aside, the new one takes its name, and the old one is
// deleted once nothing has it loaded any more (here, or by sweepAside on a later install). A process
// that already had the old file keeps using it until it restarts; a new one loads the new file.
//  returns "in_use" when the old file had to be moved aside (it was loaded somewhere)
export interface ReplaceOps {
  rename(from: string, to: string): Promise<void>;
  rm(file: string, opts: { force: boolean }): Promise<void>;
}

export const ASIDE_MARK = ".rr-old-";

export async function replaceFile(tmp: string, target: string, ops: ReplaceOps = fsp): Promise<"replaced" | "in_use"> {
  try {
    await ops.rename(tmp, target);
    return "replaced";
  } catch (first) {
    const aside = `${target}${ASIDE_MARK}${Date.now()}`;
    try {
      await ops.rename(target, aside);
    } catch {
      throw first;
    }
    try {
      await ops.rename(tmp, target);
    } catch (e) {
      // Put the old file back so the game still has a runtime.
      await ops.rename(aside, target).catch(() => undefined);
      throw e;
    }
    await ops.rm(aside, { force: true }).catch(() => undefined);
    return "in_use";
  }
}

// Deletes old runtime files moved aside by an earlier install, the ones no process holds any more.
export async function sweepAside(dir: string): Promise<void> {
  let names: string[];
  try {
    names = await fsp.readdir(dir);
  } catch {
    return;
  }
  for (const name of names) {
    if (!RUNTIME_FILE_NAMES.some((r) => name.startsWith(`${r}${ASIDE_MARK}`))) continue;
    await fsp.rm(path.join(dir, name), { force: true }).catch(() => undefined);
  }
}

export interface RuntimeInstallResult {
  // Files whose old copy was loaded by a running process (on a host's PC: its game servers). The
  // new file is in place; those processes use it once they restart.
  replacedInUse: string[];
}

// Downloads and installs the server's runtime into the game's Win64 folder. Each file is written
// next to its target first, checked, and only then renamed over the old one.
export async function installRuntime(opts: {
  endpoint: Endpoint;
  key: string;
  manifest: RuntimeManifest;
  win64Dir: string;
  signal?: AbortSignal;
  onProgress?: (p: RuntimeProgress) => void;
}): Promise<RuntimeInstallResult> {
  const { manifest, win64Dir } = opts;
  const totalBytes = manifest.files.reduce((s, f) => s + f.size, 0);
  let doneBytes = 0;
  let filesDone = 0;
  const replacedInUse: string[] = [];
  await fsp.mkdir(win64Dir, { recursive: true });
  await sweepAside(win64Dir);
  const report = (current: string | null) => opts.onProgress?.({ doneBytes, totalBytes, filesDone, filesTotal: manifest.files.length, currentFile: current });
  for (const f of manifest.files) {
    const target = path.join(win64Dir, f.name);
    try {
      if ((await hashFile(target)) === f.sha256) {
        doneBytes += f.size;
        filesDone++;
        report(f.name);
        continue;
      }
    } catch {
      /* missing: download it */
    }
    const tmp = `${target}.rr-download`;
    await fsp.rm(tmp, { force: true });
    let status: number;
    const before = doneBytes;
    try {
      status = await requestToFile(opts.endpoint, `/content/v1/runtime/files/${encodeURIComponent(f.name)}`, tmp, {
        headers: keyHeaders(opts.endpoint, opts.key),
        timeoutMs: 30000,
        maxBytes: f.size,
        signal: opts.signal,
        onBytes: (n) => {
          doneBytes += n;
          report(f.name);
        },
      });
    } catch (e) {
      await fsp.rm(tmp, { force: true });
      if (e instanceof HttpError && e.kind === "aborted") throw new RuntimeError("cancelled", f.name, "cancelled");
      if (e instanceof HttpError && e.kind === "pin") throw new RuntimeError("cert_mismatch", f.name, e.message);
      if (e instanceof HttpError && e.kind === "too_large") throw new RuntimeError("different", f.name, "larger than the server's own list says");
      if ((e as NodeJS.ErrnoException)?.code === "ENOSPC" || (e as NodeJS.ErrnoException)?.code === "EACCES" || (e as NodeJS.ErrnoException)?.code === "EPERM") {
        throw new RuntimeError("disk_error", f.name, (e as Error).message);
      }
      throw new RuntimeError("download_failed", f.name, e instanceof Error ? e.message : String(e));
    }
    if (status !== 200) {
      await fsp.rm(tmp, { force: true });
      if (status === 401 || status === 403) throw new RuntimeError("key_rejected", f.name, `HTTP ${status}`);
      if (status === 404) throw new RuntimeError("not_found", f.name, "HTTP 404");
      throw new RuntimeError("download_failed", f.name, `HTTP ${status}`);
    }
    const st = await fsp.stat(tmp);
    const digest = await hashFile(tmp);
    if (st.size !== f.size || digest !== f.sha256) {
      await fsp.rm(tmp, { force: true });
      throw new RuntimeError("different", f.name, "the file does not match the server's list");
    }
    try {
      if ((await replaceFile(tmp, target)) === "in_use") replacedInUse.push(f.name);
    } catch (e) {
      await fsp.rm(tmp, { force: true });
      // Neither replaced nor moved aside: something holds the file without sharing it.
      throw new RuntimeError("in_use", f.name, e instanceof Error ? e.message : String(e));
    }
    await unblock(target);
    if ((await hashFile(target)) !== f.sha256) throw new RuntimeError("different", f.name, "changed after copying (antivirus?)");
    doneBytes = before + f.size;
    filesDone++;
    report(f.name);
  }
  await writeMarker(win64Dir, manifest);
  return { replacedInUse };
}

// Copies a local runtime (a folder with both files, built by the host) next to the game.
export async function installLocalRuntime(localDir: string, win64Dir: string): Promise<RuntimeInstallResult> {
  const replacedInUse: string[] = [];
  await sweepAside(win64Dir);
  for (const name of RUNTIME_FILE_NAMES) {
    const source = path.join(localDir, name);
    const target = path.join(win64Dir, name);
    const sourceHash = await hashFile(source);
    try {
      if ((await hashFile(target)) === sourceHash) continue;
    } catch {
      /* missing */
    }
    const tmp = `${target}.rr-download`;
    await fsp.copyFile(source, tmp);
    if ((await hashFile(tmp)) !== sourceHash) {
      await fsp.rm(tmp, { force: true });
      throw new RuntimeError("different", name, "changed while copying (antivirus?)");
    }
    try {
      if ((await replaceFile(tmp, target)) === "in_use") replacedInUse.push(name);
    } catch (e) {
      await fsp.rm(tmp, { force: true });
      throw new RuntimeError("in_use", name, e instanceof Error ? e.message : String(e));
    }
    await unblock(target);
  }
  return { replacedInUse };
}

async function writeMarker(win64Dir: string, manifest: RuntimeManifest): Promise<void> {
  const marker = {
    note: "Written by the Dauntless Reforged Revival launcher: the game runtime installed next to the game.",
    version: manifest.version,
    files: manifest.files,
    installedAt: new Date().toISOString(),
  };
  try {
    await fsp.writeFile(path.join(win64Dir, RUNTIME_MARKER_FILE), JSON.stringify(marker, null, 2));
  } catch {
    /* informational only */
  }
}

// Whether a folder holds a complete runtime (both files, not empty).
export async function isCompleteRuntimeDir(dir: string): Promise<boolean> {
  const found = await installedRuntime(dir);
  return [...found.values()].every((h) => h !== null);
}
