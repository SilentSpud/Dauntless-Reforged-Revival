// Checks on the player's PC: Tailscale, free disk space, the Visual C++ runtime.

import { existsSync, promises as fsp } from "node:fs";
import path from "node:path";

// The game (Unreal Engine 4.26) needs the Visual C++ 2015-2022 runtime (x64). The Reforged runtime
// links its own C runtime statically, so only the game's needs count here; a missing file is a
// warning with a download link, never a reason to refuse Play.
export const VC_RUNTIME_FILES = ["VCRUNTIME140.dll", "VCRUNTIME140_1.dll", "MSVCP140.dll"];

export function tailscaleCandidates(env: NodeJS.ProcessEnv = process.env, platform: NodeJS.Platform = process.platform): string[] {
  if (platform === "linux") {
    const out = ["/usr/bin/tailscale", "/usr/local/bin/tailscale", "/snap/bin/tailscale"];
    for (const d of (env.PATH ?? "").split(path.delimiter)) if (d.trim()) out.push(path.join(d.trim(), "tailscale"));
    return [...new Set(out)];
  }
  if (platform !== "win32") return [];
  const dirs = new Set<string>();
  for (const base of [env.ProgramW6432, env.ProgramFiles, env["ProgramFiles(x86)"], "C:\\Program Files"]) {
    if (base) dirs.add(path.join(base, "Tailscale"));
  }
  if (env.LOCALAPPDATA) dirs.add(path.join(env.LOCALAPPDATA, "Tailscale"));
  const candidates = [...dirs].map((d) => path.join(d, "tailscale.exe"));
  for (const d of (env.PATH ?? env.Path ?? "").split(path.delimiter)) {
    if (d.trim()) candidates.push(path.join(d.trim().replace(/^"|"$/g, ""), "tailscale.exe"));
  }
  return candidates;
}

export function findTailscale(
  env: NodeJS.ProcessEnv = process.env,
  exists: (p: string) => boolean = existsSync,
  platform: NodeJS.Platform = process.platform,
): string | null {
  for (const c of tailscaleCandidates(env, platform)) {
    try {
      if (exists(c)) return c;
    } catch {
      /* unreadable PATH entry */
    }
  }
  return null;
}

// Free space on the drive that holds `dir` (or its nearest existing parent).
export async function freeBytes(dir: string): Promise<number | null> {
  let current = path.resolve(dir);
  for (;;) {
    try {
      const st = await fsp.statfs(current);
      return Number(st.bavail) * Number(st.bsize);
    } catch {
      const parent = path.dirname(current);
      if (parent === current) return null;
      current = parent;
    }
  }
}

export function missingVcRuntime(
  env: NodeJS.ProcessEnv = process.env,
  exists: (p: string) => boolean = existsSync,
  platform: NodeJS.Platform = process.platform,
): string[] {
  if (platform !== "win32") return [];
  const windir = env.WINDIR ?? env.SystemRoot ?? "C:\\Windows";
  return VC_RUNTIME_FILES.filter((f) => !exists(path.join(windir, "System32", f)));
}
