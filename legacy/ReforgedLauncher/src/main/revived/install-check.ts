// Checking a Dauntless 1.4.4 folder against the manifest, file by file, as Dauntless Revived's launcher
// 0.1.35 does (src/main/verify.ts there).

import { promises as fsp, readFileSync } from "node:fs";
import path from "node:path";
import { AbortedError, hashFile } from "../verify";
import { resolveInside, type ManifestFile } from "./manifest";

export interface VerifyProgress {
  totalBytes: number;
  doneBytes: number;
  filesTotal: number;
  filesDone: number;
  currentFile: string | null;
}

export interface VerifyOptions {
  fullHash: boolean;
  // Files this launcher already hashed (same size and modification time) can skip the hash.
  trusted?: (f: ManifestFile, size: number, mtimeMs: number) => boolean;
  signal?: AbortSignal;
  onProgress?: (p: VerifyProgress) => void;
  concurrency?: number;
}

export type FileProblem = "missing" | "size" | "hash";

export interface VerifyReport {
  bad: { file: ManifestFile; problem: FileProblem }[];
  checkedFiles: number;
}

export async function checkFile(
  dir: string,
  f: ManifestFile,
  fullHash: boolean,
  signal?: AbortSignal,
  onBytes?: (n: number) => void,
  trusted?: VerifyOptions["trusted"],
): Promise<FileProblem | null> {
  const full = resolveInside(dir, f.path);
  let size: number;
  let mtimeMs: number;
  try {
    const st = await fsp.stat(full);
    if (!st.isFile()) return "missing";
    size = st.size;
    mtimeMs = st.mtimeMs;
  } catch {
    return "missing";
  }
  if (size !== f.size) return "size";
  if (!fullHash || trusted?.(f, size, mtimeMs)) return null;
  const digest = await hashFile(full, signal, onBytes);
  return digest === f.sha256 ? null : "hash";
}

// Checks every file. With fullHash=false only presence and size are checked (a quick look before
// playing); Repair uses fullHash=true.
export async function verifyInstall(dir: string, files: ManifestFile[], opts: VerifyOptions): Promise<VerifyReport> {
  const totalBytes = opts.fullHash ? files.reduce((s, f) => s + f.size, 0) : files.length;
  let doneBytes = 0;
  let filesDone = 0;
  const bad: VerifyReport["bad"] = [];
  const queue = [...files];
  let lastReport = 0;
  const report = (current: string | null, force = false) => {
    const now = Date.now();
    if (!force && now - lastReport < 200) return;
    lastReport = now;
    opts.onProgress?.({ totalBytes, doneBytes, filesTotal: files.length, filesDone, currentFile: current });
  };
  const worker = async () => {
    for (;;) {
      if (opts.signal?.aborted) throw new AbortedError();
      const f = queue.shift();
      if (!f) return;
      let counted = 0;
      const problem = await checkFile(
        dir,
        f,
        opts.fullHash,
        opts.signal,
        (n) => {
          counted += n;
          doneBytes += n;
          report(f.path);
        },
        opts.trusted,
      );
      if (opts.fullHash) doneBytes += f.size - counted; // files skipped by the size check
      else doneBytes += 1;
      filesDone++;
      if (problem) bad.push({ file: f, problem });
      report(f.path);
    }
  };
  const n = Math.max(1, Math.min(opts.concurrency ?? 2, 8));
  await Promise.all(Array.from({ length: n }, worker));
  report(null, true);
  bad.sort((a, b) => (a.file.path < b.file.path ? -1 : 1));
  return { bad, checkedFiles: filesDone };
}

// Removes leftover .part files for the given manifest files (after a finished install or repair).
export async function removePartFiles(dir: string, files: ManifestFile[]): Promise<void> {
  for (const f of files) {
    try {
      await fsp.unlink(resolveInside(dir, f.path) + ".part");
    } catch {
      /* not there */
    }
  }
}

// Remembers which files were hashed (by size and modification time), so an interrupted install
// does not re-hash gigabytes it already checked. Repair ignores it and hashes everything.
export class VerifiedCache {
  private dir: string | null = null;
  private files = new Map<string, { size: number; mtimeMs: number }>();
  private dirty = false;

  constructor(private readonly file: string) {
    try {
      const raw = JSON.parse(readFileSync(file, "utf8"));
      if (raw && typeof raw.dir === "string" && raw.files && typeof raw.files === "object") {
        this.dir = raw.dir;
        for (const [p, v] of Object.entries(raw.files as Record<string, { size: unknown; mtimeMs: unknown }>)) {
          if (typeof v?.size === "number" && typeof v?.mtimeMs === "number") this.files.set(p, { size: v.size, mtimeMs: v.mtimeMs });
        }
      }
    } catch {
      /* no cache yet */
    }
  }

  private use(dir: string): void {
    if (this.dir !== dir) {
      this.dir = dir;
      this.files.clear();
      this.dirty = true;
    }
  }

  isTrusted(dir: string, f: ManifestFile, size: number, mtimeMs: number): boolean {
    if (this.dir !== dir) return false;
    const v = this.files.get(f.path);
    return v !== undefined && v.size === size && v.mtimeMs === mtimeMs && size === f.size;
  }

  async record(dir: string, f: ManifestFile): Promise<void> {
    this.use(dir);
    try {
      const st = await fsp.stat(resolveInside(dir, f.path));
      this.files.set(f.path, { size: st.size, mtimeMs: st.mtimeMs });
      this.dirty = true;
    } catch {
      /* gone again */
    }
  }

  forget(dir: string, f: ManifestFile): void {
    if (this.dir === dir && this.files.delete(f.path)) this.dirty = true;
  }

  async save(): Promise<void> {
    if (!this.dirty) return;
    this.dirty = false;
    const data = { dir: this.dir, files: Object.fromEntries(this.files) };
    await fsp.mkdir(path.dirname(this.file), { recursive: true });
    await fsp.writeFile(this.file + ".tmp", JSON.stringify(data));
    await fsp.rename(this.file + ".tmp", this.file);
  }
}
