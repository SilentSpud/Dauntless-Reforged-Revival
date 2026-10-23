// Starting the game. The command line is the one Mystic Paradox's launcher uses for 1.14.7, with the
// account key as the "exchange code": a Reforged backend runs AUTH_MODE=APIKEY and trades the key
// for the game's session token at /account/api/oauth/token, the way Dauntless Revived passes its
// key as -AUTH_PASSWORD. The key is never logged (see maskArgs).
//
// The runtime learns where the backend is from the environment: REFORGED_BACKEND_URL names the
// launcher's local relay. MYSTICPARADOX_RUNTIME_PROFILE picks the runtime's log profile.

import { spawn, execFile, type ChildProcess } from "node:child_process";
import path from "node:path";
import { isPlausibleAccountKey } from "../shared/username";
import { EXE_NAME } from "./constants";
import type { GameLanguage, RuntimeProfile } from "../shared/types";

export interface LaunchSettings {
  key: string;
  userId: string;
  username: string;
  windowed: boolean;
  gameLocale?: string; // -epiclocale; English when left out
}

const GAME_LOCALES = ["en-US", "de-DE", "es-ES", "fr-FR", "it-IT", "ja-JP", "pt-BR", "ru-RU"];

// The -epiclocale for the Game language setting. "auto" takes the operating system's language when
// the game has text for it (es-419 -> es-ES, pt-PT -> pt-BR), and English otherwise.
export function gameLocale(setting: GameLanguage, systemLocale: string): string {
  if (setting !== "auto") return setting;
  const code = systemLocale.toLowerCase().slice(0, 2);
  return GAME_LOCALES.find((l) => l.toLowerCase().startsWith(`${code}-`)) ?? "en-US";
}

// The identity values go on the command line, so only plain characters pass.
const PLAIN = /^[A-Za-z0-9_-]{1,128}$/;

export function buildLaunchArgs(s: LaunchSettings): string[] {
  if (!isPlausibleAccountKey(s.key)) throw new Error("invalid key");
  if (!PLAIN.test(s.userId)) throw new Error("invalid user id");
  if (!/^[A-Za-z0-9_]{1,32}$/.test(s.username)) throw new Error("invalid username");
  const locale = s.gameLocale && GAME_LOCALES.includes(s.gameLocale) ? s.gameLocale : "en-US";
  const args = [
    "-DisableEOS",
    "-EpicPortal",
    "-NoEAC",
    "-AUTH_TYPE=exchangecode",
    `-AUTH_LOGIN=${s.userId}`,
    `-AUTH_PASSWORD=${s.key}`,
    "-epicapp=Archon",
    "-epicenv=Prod",
    `-epicusername=${s.username}`,
    `-epicuserid=${s.userId}`,
    `-epicaccountid=${s.userId}`,
    "-epicsandboxid=jackal",
    "-epicdeploymentid=53565ba467df4edbb6f5a3d939a8b4f2",
    `-epiclocale=${locale}`,
  ];
  if (s.windowed) args.push("-windowed", "-ResX=1280", "-ResY=720");
  return args;
}

export function maskArgs(args: string[]): string[] {
  return args.map((a) => (/^-AUTH_PASSWORD=/i.test(a) ? "-AUTH_PASSWORD=<hidden>" : a));
}

export function describeLaunch(exe: string, args: string[]): string {
  return `${exe} ${maskArgs(args).join(" ")}`;
}

export function launchEnvironment(base: NodeJS.ProcessEnv, backendUrl: string, profile: RuntimeProfile): NodeJS.ProcessEnv {
  if (!/^http:\/\/127\.0\.0\.1:[1-9][0-9]{0,4}$/.test(backendUrl)) throw new Error("the backend URL must be the local relay");
  const env: NodeJS.ProcessEnv = { ...base };
  // A value left in the environment from elsewhere must never redirect the game.
  for (const k of Object.keys(env)) if (/^(REFORGED_|MYSTICPARADOX_)/i.test(k)) delete env[k];
  env.REFORGED_BACKEND_URL = backendUrl;
  env.MYSTICPARADOX_RUNTIME_PROFILE = profile;
  return env;
}

export type SpawnFn = typeof spawn;

// Tracks the one game process this launcher started.
export class GameProcess {
  private child: ChildProcess | null = null;
  private listeners: ((running: boolean, code: number | null) => void)[] = [];

  constructor(private readonly spawnFn: SpawnFn = spawn) {}

  get running(): boolean {
    return this.child !== null;
  }

  get pid(): number | null {
    return this.child?.pid ?? null;
  }

  onChange(fn: (running: boolean, code: number | null) => void): void {
    this.listeners.push(fn);
  }

  start(win64Dir: string, args: string[], env: NodeJS.ProcessEnv): Promise<void> {
    if (this.child) return Promise.reject(new Error("already running"));
    const exe = path.join(win64Dir, EXE_NAME);
    return new Promise((resolve, reject) => {
      const child = this.spawnFn(exe, args, {
        cwd: win64Dir,
        stdio: "ignore",
        windowsHide: false,
        detached: false,
        env,
      });
      let started = false;
      child.once("spawn", () => {
        started = true;
        this.child = child;
        this.listeners.forEach((l) => l(true, null));
        resolve();
      });
      child.once("error", (e) => {
        if (!started) reject(e);
      });
      child.once("exit", (code) => {
        if (this.child === child) this.child = null;
        if (started) this.listeners.forEach((l) => l(false, code));
      });
    });
  }
}

// Lists running Dauntless clients. Game servers (the same exe started with -server, as on the
// host's own PC) do not count. Only process ids and a yes/no are read back: other processes'
// command lines contain their keys and never leave PowerShell.
export function findRunningClients(platform: NodeJS.Platform = process.platform): Promise<number[]> {
  if (platform !== "win32") return Promise.resolve([]);
  return new Promise((resolve) => {
    execFile("tasklist.exe", ["/FI", `IMAGENAME eq ${EXE_NAME}`, "/FO", "CSV", "/NH"], { windowsHide: true, timeout: 10000 }, (err, stdout) => {
      if (err || !stdout.toLowerCase().includes(EXE_NAME.toLowerCase())) return resolve([]);
      const script =
        `Get-CimInstance Win32_Process -Filter "Name='${EXE_NAME}'" | ForEach-Object { ` +
        `if ($_.CommandLine -notmatch '(^|\\s)-server(\\s|$)') { $_.ProcessId } }`;
      execFile(
        "powershell.exe",
        ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script],
        { windowsHide: true, timeout: 15000 },
        (err2, out2) => {
          if (err2) return resolve([]);
          resolve(
            out2
              .split(/\r?\n/)
              .map((l) => l.trim())
              .filter((l) => /^\d+$/.test(l))
              .map(Number),
          );
        },
      );
    });
  });
}

// The game's console was closed or sent Ctrl+C (Windows ends the process with this status): the
// player closed it, not a failure.
export const STATUS_CONTROL_C_EXIT = 0xc000013a;

// The exit codes that have an explanation the player can act on.
export function gameExitDetail(code: number): string {
  const hex = `0x${(code >>> 0).toString(16).toUpperCase().padStart(8, "0")}`;
  return hex;
}
