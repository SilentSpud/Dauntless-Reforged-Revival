// Linux: the game servers are still Dauntless-Win64-Shipping.exe, a Windows program. The Director
// starts .workspace/bin/reforged-gameserver instead of the executable (GAMESERVER_BINARY_PATH), and
// that runs it through Wine (or Proton) on a virtual X display (xvfb-run): the game opens a window
// even with -nullrhi. Adapted from Dauntless Revived's Linux port (deploy/unix-common/game-runner.mjs).
//
// The Director's contract does not change: the same arguments, the same environment (the runtime
// reads MYSTICPARADOX_GAMESERVER_LAUNCH_ID and REFORGED_BACKEND_URL from it), and the readiness line
// the runtime prints on stdout, which Wine passes through.

import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

// What the Director's env file tells the launcher (written by setup, see hostconfig.ts).
export const WINE_ENV = {
    exe: "REFORGED_GAME_EXE",
    wine: "REFORGED_WINE",
    proton: "REFORGED_PROTON",
    prefix: "REFORGED_WINEPREFIX",
    protonData: "REFORGED_PROTON_DATA",
    xvfb: "REFORGED_XVFB",
    xvfbRun: "REFORGED_XVFB_RUN",
} as const;

export function isExecutable(file: string): boolean {
    try {
        fs.accessSync(file, fs.constants.X_OK);
        return fs.statSync(file).isFile();
    } catch {
        return false;
    }
}

// The first of these names found on PATH (or a path given as is), as an absolute path.
export function findOnPath(names: string[], env: NodeJS.ProcessEnv = process.env): string | null {
    for (const name of names) {
        if (name.includes("/")) {
            if (isExecutable(name)) return path.resolve(name);
            continue;
        }
        for (const dir of (env.PATH ?? "").split(path.delimiter)) {
            if (!dir) continue;
            const candidate = path.join(dir, name);
            if (isExecutable(candidate)) return candidate;
        }
    }
    return null;
}

export function findWine(env: NodeJS.ProcessEnv = process.env): string | null {
    return findOnPath(["wine", "wine64"], env);
}

// winmm=n,b: the game must load Mystic Paradox's winmm.dll (the runtime loader beside the
// executable), not Wine's own. mscoree and mshtml off: no Mono or Gecko installer prompt, which
// would wait forever on the virtual display.
export function dllOverrides(existing: string | undefined): string {
    const ours = ["winmm=n,b", "mscoree=d", "mshtml=d"];
    const names = new Set(ours.map((o) => o.split("=")[0]));
    const kept = (existing ?? "")
        .split(";")
        .map((part) => part.trim())
        .filter(Boolean)
        .filter((part) => !names.has(part.split("=")[0].toLowerCase()));
    return [...kept, ...ours].join(";");
}

export interface LaunchSpec {
    command: string;
    args: string[];
    cwd: string;
    env: NodeJS.ProcessEnv;
    runtime: string;
}

// Where Steam keeps a Proton build, for STEAM_COMPAT_CLIENT_INSTALL_PATH.
function steamRootFor(proton: string): string | null {
    let current = path.dirname(proton);
    for (let i = 0; i < 8; i++) {
        if (path.basename(current) === "compatibilitytools.d") return path.dirname(current);
        if (path.basename(current) === "common" && path.basename(path.dirname(current)) === "steamapps") return path.dirname(path.dirname(current));
        const parent = path.dirname(current);
        if (parent === current) break;
        current = parent;
    }
    return null;
}

export function buildLaunch(gameArgs: string[], env: NodeJS.ProcessEnv = process.env): LaunchSpec {
    const exe = env[WINE_ENV.exe];
    if (!exe || !path.isAbsolute(exe) || !fs.existsSync(exe)) {
        throw new Error(`${WINE_ENV.exe} must be the full path of an existing Dauntless-Win64-Shipping.exe (got ${JSON.stringify(exe ?? "")}); run npm run setup -- --game <folder>`);
    }
    const cwd = path.dirname(exe);
    const protonSetting = env[WINE_ENV.proton]?.trim();
    const wineSetting = env[WINE_ENV.wine]?.trim();
    let command: string;
    let args: string[];
    let runEnv: NodeJS.ProcessEnv;
    let runtime: string;
    if (protonSetting) {
        if (!isExecutable(protonSetting)) throw new Error(`${WINE_ENV.proton} is not an executable Proton script: ${protonSetting}`);
        const data = env[WINE_ENV.protonData]?.trim();
        if (!data || !path.isAbsolute(data)) throw new Error(`${WINE_ENV.protonData} must be a full path (Proton's compat-data folder)`);
        fs.mkdirSync(data, { recursive: true });
        const steamRoot = steamRootFor(protonSetting);
        command = protonSetting;
        args = ["run", exe, ...gameArgs];
        runEnv = {
            ...env,
            STEAM_COMPAT_DATA_PATH: data,
            ...(steamRoot ? { STEAM_COMPAT_CLIENT_INSTALL_PATH: steamRoot } : {}),
            STEAM_COMPAT_APP_ID: "0",
            PROTON_LOG: "0",
            WINEDLLOVERRIDES: dllOverrides(env.WINEDLLOVERRIDES),
        };
        runtime = `Proton (${path.basename(path.dirname(protonSetting))})`;
    } else {
        const wine = wineSetting ? (isExecutable(wineSetting) ? wineSetting : null) : findWine(env);
        if (!wine) throw new Error(wineSetting ? `${WINE_ENV.wine} is not executable: ${wineSetting}` : "Wine was not found: apt install wine (or run setup with --wine <path>)");
        const prefix = env[WINE_ENV.prefix]?.trim();
        if (!prefix || !path.isAbsolute(prefix)) throw new Error(`${WINE_ENV.prefix} must be a full path (the Wine prefix)`);
        fs.mkdirSync(prefix, { recursive: true });
        command = wine;
        args = [exe, ...gameArgs];
        runEnv = {
            ...env,
            WINEPREFIX: prefix,
            WINEDEBUG: env.WINEDEBUG ?? "-all",
            WINEDLLOVERRIDES: dllOverrides(env.WINEDLLOVERRIDES),
        };
        runtime = "Wine";
    }
    const noDisplay = !(runEnv.DISPLAY ?? "").trim();
    if (env[WINE_ENV.xvfb] !== "0" && noDisplay) {
        const xvfbSetting = env[WINE_ENV.xvfbRun]?.trim();
        const xvfb = xvfbSetting ? (isExecutable(xvfbSetting) ? xvfbSetting : null) : findOnPath(["xvfb-run"], env);
        if (!xvfb) throw new Error("there is no display and xvfb-run was not found: apt install xvfb xauth (or set a DISPLAY)");
        return { command: xvfb, args: ["-a", "-s", "-screen 0 1024x768x24", command, ...args], cwd, env: runEnv, runtime: `${runtime} on xvfb-run` };
    }
    return { command, args, cwd, env: runEnv, runtime };
}

// The launched runtime leads its own process group (xvfb-run, Xvfb, wine and the game): a signal
// for the game server goes to all of them, so no Xvfb or game process outlives it.
export function signalGroup(pid: number | undefined, signal: NodeJS.Signals, kill: (pid: number, signal: NodeJS.Signals) => void = process.kill): boolean {
    if (pid === undefined || pid <= 0) return false;
    try {
        kill(-pid, signal);
        return true;
    } catch {
        try {
            kill(pid, signal);
            return true;
        } catch {
            return false;
        }
    }
}

// Runs one game server and exits with its exit code. stdio is inherited: the Director reads the
// runtime's readiness line from it and writes it all to the game server's log.
export async function runGameServer(gameArgs: string[], env: NodeJS.ProcessEnv = process.env): Promise<number> {
    const spec = buildLaunch(gameArgs, env);
    return new Promise((resolve) => {
        const child = spawn(spec.command, spec.args, { cwd: spec.cwd, env: spec.env, stdio: "inherit", detached: true });
        let done = false;
        const handlers: [NodeJS.Signals, () => void][] = [];
        const finish = (code: number) => {
            if (done) return;
            done = true;
            for (const [signal, handler] of handlers) process.off(signal, handler);
            resolve(code);
        };
        child.once("error", (e) => {
            process.stderr.write(`reforged-gameserver: could not start ${spec.command}: ${e.message}\n`);
            finish(127);
        });
        child.once("exit", (code, signal) => {
            // Whatever is left of the group (Xvfb) goes with it.
            signalGroup(child.pid, "SIGTERM");
            if (signal) process.stderr.write(`reforged-gameserver: ${spec.runtime} ended on ${signal}\n`);
            finish(code ?? 1);
        });
        for (const signal of ["SIGTERM", "SIGINT", "SIGHUP"] as NodeJS.Signals[]) {
            const handler = () => {
                signalGroup(child.pid, signal);
            };
            handlers.push([signal, handler]);
            process.on(signal, handler);
        }
        process.stderr.write(`reforged-gameserver: game server started with ${spec.runtime} (pid ${child.pid ?? "?"})\n`);
    });
}

// A Wine prefix is ready once wineboot has made its registry.
export function winePrefixReady(prefix: string): boolean {
    return fs.existsSync(path.join(prefix, "system.reg"));
}
