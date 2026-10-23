// SHA-256 of files on disk, streamed, and a small cache for the large game executable.

import { createHash } from "node:crypto";
import { createReadStream, promises as fsp, readFileSync } from "node:fs";
import path from "node:path";

export class AbortedError extends Error {
  constructor() {
    super("aborted");
    this.name = "AbortedError";
  }
}

export function hashFile(file: string, signal?: AbortSignal, onBytes?: (n: number) => void): Promise<string> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) return reject(new AbortedError());
    const hash = createHash("sha256");
    const stream = createReadStream(file, { highWaterMark: 1024 * 1024 });
    const onAbort = () => {
      stream.destroy();
      reject(new AbortedError());
    };
    signal?.addEventListener("abort", onAbort, { once: true });
    stream.on("data", (chunk) => {
      hash.update(chunk as Buffer);
      onBytes?.((chunk as Buffer).length);
    });
    stream.on("error", (e) => {
      signal?.removeEventListener("abort", onAbort);
      reject(e);
    });
    stream.on("end", () => {
      signal?.removeEventListener("abort", onAbort);
      resolve(hash.digest("hex"));
    });
  });
}

// Remembers the hash of a file by its path, size and modification time, so the 120 MB game
// executable is hashed once after it changes, not on every start. A file that changes (another
// build, a repair) has a new size or time and is hashed again; Play always hashes afresh.
export class HashCache {
  private entries = new Map<string, { size: number; mtimeMs: number; sha256: string }>();
  private dirty = false;

  constructor(private readonly file: string) {
    try {
      const raw = JSON.parse(readFileSync(file, "utf8"));
      if (raw && typeof raw === "object" && raw.files && typeof raw.files === "object") {
        for (const [p, v] of Object.entries(raw.files as Record<string, { size: unknown; mtimeMs: unknown; sha256: unknown }>)) {
          if (typeof v?.size === "number" && typeof v?.mtimeMs === "number" && typeof v?.sha256 === "string" && /^[0-9a-f]{64}$/.test(v.sha256)) {
            this.entries.set(p, { size: v.size, mtimeMs: v.mtimeMs, sha256: v.sha256 });
          }
        }
      }
    } catch {
      /* no cache yet */
    }
  }

  // The cached hash, or null when the file changed since (or was never hashed).
  async cached(file: string): Promise<string | null> {
    const entry = this.entries.get(file);
    if (!entry) return null;
    try {
      const st = await fsp.stat(file);
      return st.size === entry.size && st.mtimeMs === entry.mtimeMs ? entry.sha256 : null;
    } catch {
      return null;
    }
  }

  async hash(file: string, signal?: AbortSignal, onBytes?: (n: number) => void): Promise<string> {
    const st = await fsp.stat(file);
    const digest = await hashFile(file, signal, onBytes);
    const after = await fsp.stat(file);
    // Only remember a hash whose file did not change while it was being read.
    if (after.size === st.size && after.mtimeMs === st.mtimeMs) {
      this.entries.set(file, { size: st.size, mtimeMs: st.mtimeMs, sha256: digest });
      this.dirty = true;
      if (this.entries.size > 32) this.entries.delete(this.entries.keys().next().value as string);
    }
    return digest;
  }

  async save(): Promise<void> {
    if (!this.dirty) return;
    this.dirty = false;
    await fsp.mkdir(path.dirname(this.file), { recursive: true });
    await fsp.writeFile(this.file + ".tmp", JSON.stringify({ files: Object.fromEntries(this.entries) }));
    await fsp.rename(this.file + ".tmp", this.file);
  }
}
