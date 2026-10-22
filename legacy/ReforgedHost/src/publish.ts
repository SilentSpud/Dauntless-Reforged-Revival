// Publishing the runtime: the built MysticParadox.dll and winmm.dll go to .workspace/runtime/ with
// runtime.json (what the content server lists and the launcher checks), and next to the server's
// own game executable (the Director's game servers load the same runtime).

import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { GAME_CHANGELIST, RUNTIME_FILE_NAMES, type Paths } from "./paths";

export interface PublishedRuntime {
    version: string;
    build: { changelist: number };
    files: { name: string; size: number; sha256: string }[];
}

export function sha256File(file: string): string {
    return createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function copyAtomically(from: string, to: string): void {
    fs.mkdirSync(path.dirname(to), { recursive: true });
    fs.copyFileSync(from, `${to}.tmp`);
    replaceLoadedFile(`${to}.tmp`, to);
}

// Windows refuses to overwrite a DLL a running game server has loaded, but lets it be renamed: the
// old file is moved aside, the new one takes its name, and the old one is deleted when it is free
// (now, or on the next publish). Running game servers keep the old runtime until they restart.
export const ASIDE_MARK = ".rr-old-";

export function replaceLoadedFile(tmp: string, target: string): "replaced" | "in_use" {
    try {
        fs.renameSync(tmp, target);
        return "replaced";
    } catch (first) {
        const aside = `${target}${ASIDE_MARK}${Date.now()}`;
        try {
            fs.renameSync(target, aside);
        } catch {
            fs.rmSync(tmp, { force: true });
            throw first;
        }
        try {
            fs.renameSync(tmp, target);
        } catch (e) {
            try {
                fs.renameSync(aside, target);
            } catch {
                /* the old file stays aside */
            }
            fs.rmSync(tmp, { force: true });
            throw e;
        }
        try {
            fs.rmSync(aside, { force: true });
        } catch {
            /* still loaded: swept later */
        }
        return "in_use";
    }
}

function sweepAside(dir: string): void {
    let names: string[];
    try {
        names = fs.readdirSync(dir);
    } catch {
        return;
    }
    for (const name of names) {
        if (!RUNTIME_FILE_NAMES.some((r) => name.startsWith(`${r}${ASIDE_MARK}`))) continue;
        try {
            fs.rmSync(path.join(dir, name), { force: true });
        } catch {
            /* still loaded */
        }
    }
}

export function publishRuntime(p: Paths, now: Date = new Date(), sources: Record<string, string> = { "MysticParadox.dll": p.runtimeDll, "winmm.dll": p.loaderDll }): PublishedRuntime {
    for (const name of RUNTIME_FILE_NAMES) {
        if (!fs.existsSync(sources[name])) throw new Error(`${name} is not built yet (${sources[name]})`);
    }
    fs.mkdirSync(p.runtimePublish, { recursive: true });
    const files: PublishedRuntime["files"] = [];
    for (const name of RUNTIME_FILE_NAMES) {
        const target = path.join(p.runtimePublish, name);
        copyAtomically(sources[name], target);
        files.push({ name, size: fs.statSync(target).size, sha256: sha256File(target) });
    }
    const id = createHash("sha256").update(files.map((f) => f.sha256).join(":")).digest("hex").slice(0, 8);
    const runtime: PublishedRuntime = { version: `${now.toISOString().slice(0, 10).replace(/-/g, "")}-${id}`, build: { changelist: GAME_CHANGELIST }, files };
    // runtime.json last: the content server never sees a list whose files are not in place yet.
    fs.writeFileSync(path.join(p.runtimePublish, "runtime.json.tmp"), JSON.stringify(runtime, null, 2) + "\n");
    fs.renameSync(path.join(p.runtimePublish, "runtime.json.tmp"), path.join(p.runtimePublish, "runtime.json"));
    return runtime;
}

// Copies the published runtime next to the server's game executable. A file a running game server
// has loaded is moved aside and still replaced (listed in inUse: those servers use it once they
// restart); a file that can't be replaced at all is reported in failed.
export function installServerRuntime(p: Paths, win64Dir: string): { installed: string[]; unchanged: string[]; inUse: string[]; failed: { name: string; error: string }[] } {
    const out = { installed: [] as string[], unchanged: [] as string[], inUse: [] as string[], failed: [] as { name: string; error: string }[] };
    sweepAside(win64Dir);
    for (const name of RUNTIME_FILE_NAMES) {
        const from = path.join(p.runtimePublish, name);
        const to = path.join(win64Dir, name);
        try {
            if (fs.existsSync(to) && sha256File(to) === sha256File(from)) {
                out.unchanged.push(name);
                continue;
            }
            fs.mkdirSync(win64Dir, { recursive: true });
            fs.copyFileSync(from, `${to}.tmp`);
            if (replaceLoadedFile(`${to}.tmp`, to) === "in_use") out.inUse.push(name);
            out.installed.push(name);
        } catch (e) {
            out.failed.push({ name, error: e instanceof Error ? e.message : String(e) });
        }
    }
    return out;
}

// Runtime switches the game servers need, as empty files beside the game executable (the runtime checks
// for them there). Mystic Paradox's runtime ships them off. They act on the server role only, so the host's
// own game client, which runs from the same folder, is not affected.
//   CALL_HUNTPASS_QUERY.flag: a 1.14.7 hub never queries the progression configuration on its own, so each
//   player's progression component waits forever and the 60-second player-data timeout drops them. With it
//   the hub queries the current Hunt Pass season, which fetches the configuration (GET /progression/config).
export const SERVER_RUNTIME_FLAGS = ["CALL_HUNTPASS_QUERY.flag"];

// Creates the missing switches; returns the ones it created.
export function ensureServerFlags(win64Dir: string): string[] {
    const created: string[] = [];
    for (const name of SERVER_RUNTIME_FLAGS) {
        const file = path.join(win64Dir, name);
        if (fs.existsSync(file)) continue;
        fs.writeFileSync(file, "");
        created.push(name);
    }
    return created;
}
