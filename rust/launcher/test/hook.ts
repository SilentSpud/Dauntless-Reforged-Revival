// Loads the isolation hook (isolation/commands.js and hook.js) the way Tauri's isolation frame runs it.

import { readFileSync } from "node:fs";
import vm from "node:vm";

export type Spec = Record<string, { kind: string; max?: number }>;
export type Hook = (payload: unknown) => unknown;

export function loadHook(): { hook: Hook; commands: Record<string, Spec> } {
  const window: Record<string, unknown> = {};
  const context = vm.createContext({ window });
  for (const file of ["commands.js", "hook.js"]) {
    vm.runInContext(readFileSync(new URL(`../isolation/${file}`, import.meta.url), "utf8"), context);
  }
  return { hook: window.__TAURI_ISOLATION_HOOK__ as Hook, commands: window.__RR_COMMANDS__ as Record<string, Spec> };
}
