// A Dauntless 1.4.4 already on this PC: the folder Dauntless Revived's own launcher downloaded it to
// (its default, or the one in its settings), so it is not downloaded a second time.

import { promises as fsp } from "node:fs";
import path from "node:path";
import type { DetectedGame } from "../../shared/types";
import { REVIVED_EXE_RELATIVE_PATH, REVIVED_EXE_SIZE } from "./constants";

// Dauntless Revived's launcher downloads to %LOCALAPPDATA%\DauntlessRevived\Game unless told otherwise.
export function revivedDefaultDir(env: NodeJS.ProcessEnv): string | null {
  return env.LOCALAPPDATA ? path.join(env.LOCALAPPDATA, "DauntlessRevived", "Game") : null;
}

export async function hasRevivedExe(root: string): Promise<boolean> {
  try {
    const st = await fsp.stat(path.join(root, ...REVIVED_EXE_RELATIVE_PATH.split("/")));
    return st.isFile() && st.size === REVIVED_EXE_SIZE;
  } catch {
    return false;
  }
}

export async function detectRevivedInstalls(env: NodeJS.ProcessEnv): Promise<DetectedGame[]> {
  const candidates: string[] = [];
  if (env.APPDATA) {
    try {
      const raw = JSON.parse(await fsp.readFile(path.join(env.APPDATA, "Dauntless Revived Launcher", "settings.json"), "utf8")) as { installDir?: unknown };
      if (typeof raw.installDir === "string" && path.win32.isAbsolute(raw.installDir) && raw.installDir.length < 200) candidates.push(raw.installDir);
    } catch {
      /* no Dauntless Revived launcher here */
    }
  }
  const def = revivedDefaultDir(env);
  if (def) candidates.push(def);
  const found: DetectedGame[] = [];
  for (const root of [...new Set(candidates)]) if (await hasRevivedExe(root)) found.push({ root, source: "revived", version: "1.4.4" });
  return found;
}
