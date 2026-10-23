// The two client DLLs Dauntless Revived's launcher 0.1.35 puts next to the game (dxgi.dll and
// UndauntedInternalServer.dll). That launcher ships them in its own resources; this one downloads the
// same files once from that release's commit on GitHub into its own folder, and only keeps a file whose
// SHA-256 is the pinned one. They are copied next to the game the way 0.1.35 does it: checked before
// copying, after copying and before every launch.

import https from "node:https";
import { promises as fsp } from "node:fs";
import path from "node:path";
import { hashFile } from "../verify";
import { REVIVED_DLLS, type PinnedDll } from "./constants";

export class DllError extends Error {
  constructor(public readonly dll: string, message: string) {
    super(message);
    this.name = "DllError";
  }
}

export type FetchBytes = (url: string, maxBytes: number) => Promise<Buffer>;

// A plain HTTPS GET (no redirects: raw.githubusercontent.com answers directly), at most maxBytes.
export const httpsGet: FetchBytes = (url, maxBytes) =>
  new Promise((resolve, reject) => {
    const req = https.get(url, { timeout: 30000, headers: { "user-agent": "DauntlessReforgedRevivalLauncher" } }, (res) => {
      if (res.statusCode !== 200) {
        res.resume();
        return reject(new Error(`HTTP ${res.statusCode ?? "?"}`));
      }
      const chunks: Buffer[] = [];
      let n = 0;
      res.on("data", (c: Buffer) => {
        n += c.length;
        if (n > maxBytes) {
          req.destroy(new Error("too large"));
          return;
        }
        chunks.push(c);
      });
      res.on("end", () => resolve(Buffer.concat(chunks)));
      res.on("error", reject);
    });
    req.on("timeout", () => req.destroy(new Error("timed out")));
    req.on("error", reject);
  });

export function win64Dir(root: string): string {
  return path.join(root, "Archon", "Binaries", "Win64");
}

// Clears the "downloaded from the internet" mark (the Zone.Identifier stream), like Unblock-File.
async function unblock(file: string): Promise<void> {
  try {
    await fsp.unlink(`${file}:Zone.Identifier`);
  } catch {
    /* no mark, or not NTFS */
  }
}

async function hashOrNull(file: string): Promise<string | null> {
  try {
    return await hashFile(file);
  } catch {
    return null;
  }
}

// The launcher's own copies (userData\revived\dlls): downloaded once, kept while they match.
export async function ensureDllCache(cacheDir: string, fetch: FetchBytes = httpsGet, dlls: readonly PinnedDll[] = REVIVED_DLLS): Promise<void> {
  await fsp.mkdir(cacheDir, { recursive: true });
  for (const d of dlls) {
    const file = path.join(cacheDir, d.name);
    if ((await hashOrNull(file)) === d.sha256) continue;
    let data: Buffer;
    try {
      data = await fetch(d.url, d.size + 1024);
    } catch (e) {
      throw new DllError(d.name, `download failed: ${e instanceof Error ? e.message : String(e)}`);
    }
    const tmp = `${file}.part`;
    await fsp.writeFile(tmp, data);
    if ((await hashOrNull(tmp)) !== d.sha256) {
      await fsp.unlink(tmp).catch(() => undefined);
      throw new DllError(d.name, "the downloaded file is not the pinned one");
    }
    await fsp.rename(tmp, file);
  }
}

export async function dllsInstalled(root: string, dlls: readonly PinnedDll[] = REVIVED_DLLS): Promise<boolean> {
  for (const d of dlls) if ((await hashOrNull(path.join(win64Dir(root), d.name))) !== d.sha256) return false;
  return true;
}

// Copies the cached DLLs next to the game, each checked before, while and after copying.
export async function installDlls(cacheDir: string, root: string, dlls: readonly PinnedDll[] = REVIVED_DLLS): Promise<void> {
  const dir = win64Dir(root);
  await fsp.mkdir(dir, { recursive: true });
  for (const d of dlls) {
    const target = path.join(dir, d.name);
    if ((await hashOrNull(target)) === d.sha256) {
      await unblock(target);
      continue;
    }
    const source = path.join(cacheDir, d.name);
    if ((await hashOrNull(source)) !== d.sha256) throw new DllError(d.name, "the launcher's copy does not match the pinned hash");
    const tmp = `${target}.new`;
    await fsp.copyFile(source, tmp);
    if ((await hashOrNull(tmp)) !== d.sha256) {
      await fsp.unlink(tmp).catch(() => undefined);
      throw new DllError(d.name, "changed while copying (antivirus?)");
    }
    await fsp.rename(tmp, target);
    await unblock(target);
    if ((await hashOrNull(target)) !== d.sha256) throw new DllError(d.name, "changed after copying (antivirus?)");
  }
}
