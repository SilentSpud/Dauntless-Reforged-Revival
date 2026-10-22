// The Host page's Requirements tab: what hosting a server and building its runtime need on this PC,
// each one detected here, with what is wrong when something is installed but will not work
// (an old Node.js, the MongoDB Windows service holding the host kit's port, Visual Studio without
// its C++ workload). The tab's install guides are in the page's text (src/shared/i18n.ts).

import { existsSync, readdirSync } from "node:fs";
import path from "node:path";
import { execText, type ExecFn, type Toolchain } from "./hostkit";
import { findNode } from "./hostserver";
import { missingVcRuntime } from "./system";
import type { Requirement, RequirementId } from "../shared/types";

export const NODE_MINIMUM = [20, 19] as const;

export interface RequirementProbes {
  env: NodeJS.ProcessEnv;
  platform: NodeJS.Platform;
  toolchain: Toolchain;
  game: { ok: boolean; detail: string | null };
  dumperDll: string | null;
  findTailscale: () => string | null;
  exec?: ExecFn;
  exists?: (p: string) => boolean;
}

export function nodeVersionOk(version: string): boolean {
  const m = /^v?(\d+)\.(\d+)/.exec(version.trim());
  if (!m) return false;
  const [major, minor] = [Number(m[1]), Number(m[2])];
  return major > NODE_MINIMUM[0] || (major === NODE_MINIMUM[0] && minor >= NODE_MINIMUM[1]);
}

function pathDirs(env: NodeJS.ProcessEnv): string[] {
  return (env.PATH ?? env.Path ?? "")
    .split(";")
    .map((d) => d.trim().replace(/^"|"$/g, ""))
    .filter(Boolean);
}

function first(candidates: string[], exists: (p: string) => boolean): string | null {
  for (const c of candidates) {
    try {
      if (exists(c)) return c;
    } catch {
      /* unreadable */
    }
  }
  return null;
}

export function findGit(env: NodeJS.ProcessEnv, exists: (p: string) => boolean = existsSync): string | null {
  const bases = [env.ProgramW6432, env.ProgramFiles, "C:\\Program Files", env.LOCALAPPDATA ? path.win32.join(env.LOCALAPPDATA, "Programs") : undefined].filter((b): b is string => !!b);
  return first([...pathDirs(env).map((d) => path.win32.join(d, "git.exe")), ...bases.map((b) => path.win32.join(b, "Git", "cmd", "git.exe"))], exists);
}

// As ReforgedHost's findMongod: on PATH, or the newest version in the installer's folder.
export function findMongod(env: NodeJS.ProcessEnv, exists: (p: string) => boolean = existsSync, listDir?: (dir: string) => string[]): string | null {
  const onPath = first(pathDirs(env).map((d) => path.win32.join(d, "mongod.exe")), exists);
  if (onPath) return onPath;
  const base = path.win32.join(env.ProgramFiles ?? "C:\\Program Files", "MongoDB", "Server");
  let versions: string[] = [];
  try {
    versions = (listDir ?? ((d: string) => readdirSync(d)))(base);
  } catch {
    return null;
  }
  versions.sort((a, b) => b.localeCompare(a, undefined, { numeric: true }));
  return first(versions.map((v) => path.win32.join(base, v, "bin", "mongod.exe")), exists);
}

// Visual Studio of any kind, with or without the C++ workload.
async function anyVisualStudio(env: NodeJS.ProcessEnv, exec: ExecFn, exists: (p: string) => boolean): Promise<string | null> {
  const vswhere = path.win32.join(env["ProgramFiles(x86)"] ?? "C:\\Program Files (x86)", "Microsoft Visual Studio", "Installer", "vswhere.exe");
  if (!exists(vswhere)) return null;
  try {
    const list = JSON.parse((await exec(vswhere, ["-latest", "-products", "*", "-format", "json", "-utf8"])) ?? "[]") as { displayName?: unknown }[];
    return typeof list[0]?.displayName === "string" ? list[0].displayName.slice(0, 80) : null;
  } catch {
    return null;
  }
}

export async function detectRequirements(o: RequirementProbes): Promise<Requirement[]> {
  const exec = o.exec ?? execText;
  const exists = o.exists ?? existsSync;
  const win = o.platform === "win32";
  const req = (id: RequirementId, state: Requirement["state"], detail: string | null = null, problem: string | null = null): Requirement => ({ id, state, detail, problem });
  const out: Requirement[] = [];

  out.push(req("dauntless", o.game.ok ? "ok" : "missing", o.game.detail));

  const git = win ? findGit(o.env, exists) : null;
  out.push(git ? req("git", "ok", (await exec(git, ["--version"]))?.trim().replace(/^git version /, "").slice(0, 40) ?? null) : req("git", "missing"));

  const node = findNode(o.env, exists, o.platform);
  if (!node) out.push(req("node", "missing"));
  else {
    const version = (await exec(node.node, ["--version"]))?.trim() ?? "";
    out.push(version && !nodeVersionOk(version) ? req("node", "problem", version, "too_old") : req("node", "ok", version || null));
  }

  const mongod = win ? findMongod(o.env, exists) : null;
  if (!mongod) out.push(req("mongodb", "missing"));
  else {
    // The installer's default Windows service holds port 27017 without a replica set.
    const service = await exec("sc", ["query", "MongoDB"]);
    const version = /MongoDB[\\/]Server[\\/]([^\\/]+)[\\/]/i.exec(mongod)?.[1] ?? null;
    out.push(service && /RUNNING/i.test(service) ? req("mongodb", "problem", version, "service_running") : req("mongodb", "ok", version));
  }

  const tailscale = o.findTailscale();
  out.push(req("tailscale", tailscale ? "ok" : "missing"));

  if (o.toolchain.visualStudio) out.push(req("visual_studio", "ok", o.toolchain.visualStudio));
  else {
    const vs = win ? await anyVisualStudio(o.env, exec, exists) : null;
    out.push(vs ? req("visual_studio", "problem", vs, "no_cpp") : req("visual_studio", "missing"));
  }

  out.push(req("rust", o.toolchain.cargo ? "ok" : "missing"));
  out.push(o.dumperDll && exists(o.dumperDll) ? req("dumper7", "ok", path.win32.basename(o.dumperDll)) : req("dumper7", "missing"));
  out.push(req("vc_redist", missingVcRuntime(o.env, exists, o.platform).length === 0 ? "ok" : "missing"));
  return out;
}
