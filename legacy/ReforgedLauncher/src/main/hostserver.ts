// The Host page's server: ReforgedHost's commands (setup, start, gamedata, invite), run by the launcher
// so the host never has to open a terminal. They run with the Node.js installed on this PC (the
// launcher's own runtime is Electron), straight from the host kit's build (node dist/cli.js), with the
// host kit built first when its sources changed.
//
// The server (npm run start) stays a child of the launcher. Windows has no signal a program can send
// another to stop it the way Ctrl+C does, so the host kit reads a "stop" line on its stdin
// (REFORGED_CONTROL=stdin, legacy/ReforgedHost/src/cli.ts): it stops every service and exits. Only when it
// does not within STOP_TIMEOUT_MS is its process tree ended. It also stops when the launcher quits
// (its stdin closes).

import { execFile, spawn as nodeSpawn, type ChildProcess } from "node:child_process";
import { createWriteStream, existsSync, mkdirSync, readdirSync, statSync, type WriteStream } from "node:fs";
import net from "node:net";
import path from "node:path";
import type { HostServerState, HostServerView } from "../shared/types";
import { hostKitDir } from "./hostkit";

// The host kit's contract (legacy/ReforgedHost/src/cli.ts).
export const CONTROL_ENV = "REFORGED_CONTROL";
export const SERVER_READY_LINE = "REFORGED_SERVER_READY";
// The backend's port: something answers there when a server runs on this PC.
export const BACKEND_PORT = 61000;
export const STOP_TIMEOUT_MS = 45000;
const MAX_LINES = 300;

export interface NodeTools {
  node: string;
  npmCli: string | null; // npm's own script beside node, so npm runs without a shell
}

function safeExists(exists: (p: string) => boolean, file: string): boolean {
  try {
    return exists(file);
  } catch {
    return false;
  }
}

// node on PATH, or where its Windows installer and nvm-windows put it.
export function findNode(env: NodeJS.ProcessEnv, exists: (p: string) => boolean = existsSync, platform: NodeJS.Platform = process.platform): NodeTools | null {
  const win = platform === "win32";
  const p = win ? path.win32 : path.posix;
  const dirs: string[] = [];
  for (const d of (env.PATH ?? env.Path ?? "").split(win ? ";" : ":")) {
    const dir = d.trim().replace(/^"|"$/g, "");
    if (dir) dirs.push(dir);
  }
  if (win) {
    for (const base of [env.ProgramW6432, env.ProgramFiles, "C:\\Program Files"]) if (base) dirs.push(p.join(base, "nodejs"));
    if (env.NVM_SYMLINK) dirs.push(env.NVM_SYMLINK);
  }
  for (const dir of dirs) {
    const node = p.join(dir, win ? "node.exe" : "node");
    if (!safeExists(exists, node)) continue;
    const npmCli = win ? p.join(dir, "node_modules", "npm", "bin", "npm-cli.js") : p.join(dir, "..", "lib", "node_modules", "npm", "bin", "npm-cli.js");
    return { node, npmCli: safeExists(exists, npmCli) ? npmCli : null };
  }
  return null;
}

// Terminal colours and carriage-return progress bars are not for a page.
export function cleanLine(line: string): string {
  return line.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "").replace(/\r/g, "").trimEnd().slice(0, 400);
}

// The line that says why a command failed: the host kit's "Error: ..." line, else the last one.
export function failureLine(lines: readonly string[]): string | null {
  for (let i = lines.length - 1; i >= 0; i--) {
    const m = /^Error: (.+)$/.exec(lines[i].trim());
    if (m) return m[1].slice(0, 300);
  }
  const last = [...lines].reverse().find((l) => l.trim());
  return last ? last.trim().slice(0, 300) : null;
}

function newestMtime(dir: string): number {
  let newest = 0;
  let entries: string[];
  try {
    entries = readdirSync(dir);
  } catch {
    return 0;
  }
  for (const name of entries) {
    const full = path.join(dir, name);
    try {
      const st = statSync(full);
      newest = Math.max(newest, st.isDirectory() ? newestMtime(full) : st.mtimeMs);
    } catch {
      /* gone */
    }
  }
  return newest;
}

// The host kit needs building when dist/cli.js is missing or older than a source file.
export function hostKitNeedsBuild(hostKitDir: string): boolean {
  const cli = path.join(hostKitDir, "dist", "cli.js");
  let built: number;
  try {
    built = statSync(cli).mtimeMs;
  } catch {
    return true;
  }
  return newestMtime(path.join(hostKitDir, "src")) > built;
}

export type SpawnFn = typeof nodeSpawn;

export interface HostServerOptions {
  env: NodeJS.ProcessEnv;
  platform: NodeJS.Platform;
  onChange: () => void;
  spawn?: SpawnFn;
  findNode?: () => NodeTools | null;
  // Is a server answering on this PC? (the backend's port)
  probe?: () => Promise<boolean>;
  killTree?: (pid: number) => Promise<void>;
  stopTimeoutMs?: number;
}

export class HostKitError extends Error {
  constructor(readonly code: string, readonly detail: string | null = null) {
    super(detail ? `${code}: ${detail}` : code);
  }
}

function portAnswers(port: number, host = "127.0.0.1", timeoutMs = 700): Promise<boolean> {
  return new Promise((resolve) => {
    const socket = net.connect({ port, host });
    const done = (ok: boolean) => {
      socket.destroy();
      resolve(ok);
    };
    socket.setTimeout(timeoutMs, () => done(false));
    socket.once("connect", () => done(true));
    socket.once("error", () => done(false));
  });
}

function defaultKillTree(platform: NodeJS.Platform): (pid: number) => Promise<void> {
  return (pid) =>
    new Promise((resolve) => {
      if (platform === "win32") execFile("taskkill", ["/PID", String(pid), "/T", "/F"], { windowsHide: true }, () => resolve());
      else {
        try {
          process.kill(pid, "SIGKILL");
        } catch {
          /* gone */
        }
        resolve();
      }
    });
}

export class HostServer {
  private state: HostServerState = "stopped";
  private message: string | null = null;
  private invite: HostServerView["invite"] = null;
  private lines: string[] = [];
  private child: ChildProcess | null = null;
  private stopRequested = false;
  private building: Promise<void> | null = null;
  private changeTimer: NodeJS.Timeout | null = null;
  private readonly spawn: SpawnFn;

  constructor(private readonly o: HostServerOptions) {
    this.spawn = o.spawn ?? nodeSpawn;
  }

  view(): HostServerView {
    return { state: this.state, message: this.message, lines: [...this.lines], invite: this.invite };
  }

  get running(): boolean {
    return this.child !== null;
  }

  get serverState(): HostServerState {
    return this.state;
  }

  setInvite(invite: HostServerView["invite"]): void {
    this.invite = invite;
    this.changed(true);
  }

  // Output arrives line by line; the page hears of it at most every 250 ms.
  private changed(now = false): void {
    if (now) {
      if (this.changeTimer) clearTimeout(this.changeTimer);
      this.changeTimer = null;
      this.o.onChange();
      return;
    }
    if (this.changeTimer) return;
    this.changeTimer = setTimeout(() => {
      this.changeTimer = null;
      this.o.onChange();
    }, 250);
    this.changeTimer.unref?.();
  }

  private set(state: HostServerState, message: string | null = this.message): void {
    this.state = state;
    this.message = message;
    this.changed(true);
  }

  note(line: string): void {
    const clean = cleanLine(line);
    if (!clean) return;
    this.lines.push(clean);
    if (this.lines.length > MAX_LINES) this.lines.splice(0, this.lines.length - MAX_LINES);
    this.changed();
  }

  private tools(): NodeTools {
    const tools = (this.o.findNode ?? (() => findNode(this.o.env, existsSync, this.o.platform)))();
    if (!tools) throw new HostKitError("node_missing");
    return tools;
  }

  private childEnv(tools: NodeTools, extra: Record<string, string> = {}): NodeJS.ProcessEnv {
    const sep = this.o.platform === "win32" ? ";" : ":";
    const dir = path.dirname(tools.node);
    const env: NodeJS.ProcessEnv = { ...this.o.env, NO_COLOR: "1", FORCE_COLOR: "0", ...extra };
    // The host kit starts npm and node itself: the same Node.js, without a shell.
    if (tools.npmCli) env.npm_execpath = tools.npmCli;
    const pathKey = Object.keys(env).find((k) => k.toLowerCase() === "path") ?? "PATH";
    env[pathKey] = `${dir}${sep}${env[pathKey] ?? ""}`;
    for (const k of Object.keys(env)) if (/^ELECTRON_/i.test(k)) delete env[k];
    return env;
  }

  // Runs a program, its output into the page's lines and a log file. Resolves with the exit code.
  private exec(file: string, args: string[], cwd: string, env: NodeJS.ProcessEnv, logFile: string, onLine?: (line: string) => boolean): { child: ChildProcess; done: Promise<number> } {
    let log: WriteStream | null = null;
    try {
      mkdirSync(path.dirname(logFile), { recursive: true });
      log = createWriteStream(logFile, { flags: "a" });
      log.write(`\n> ${path.basename(file)} ${args.join(" ")}  (in ${cwd}, ${new Date().toISOString()})\n`);
    } catch {
      log = null;
    }
    const child = this.spawn(file, args, { cwd, env, windowsHide: true, stdio: ["pipe", "pipe", "pipe"] });
    const reader = () => {
      let rest = "";
      return (chunk: Buffer) => {
        const text = chunk.toString("utf8");
        log?.write(text);
        rest += text;
        const parts = rest.split("\n");
        rest = parts.pop() ?? "";
        for (const line of parts) if (!onLine || onLine(line)) this.note(line);
      };
    };
    child.stdout?.on("data", reader());
    child.stderr?.on("data", reader());
    const done = new Promise<number>((resolve) => {
      child.once("error", (e) => {
        this.note(`could not start ${path.basename(file)}: ${e.message}`);
        log?.end();
        resolve(-1);
      });
      child.once("close", (code) => {
        log?.end(`\n(exit code ${code ?? "none"})\n`);
        resolve(code ?? -1);
      });
    });
    return { child, done };
  }

  // npm install (once, or again when an update changed its dependencies) and npm run build in ReforgedHost,
  // when its build is missing or older than its sources.
  async prepare(workspace: string, opts: { install?: boolean } = {}): Promise<NodeTools> {
    const tools = this.tools();
    const dir = hostKitDir(workspace);
    if (!existsSync(path.join(dir, "package.json"))) throw new HostKitError("workspace");
    if (this.building) await this.building;
    const install = opts.install === true || !existsSync(path.join(dir, "node_modules"));
    if (!install && !hostKitNeedsBuild(dir)) return tools;
    if (!tools.npmCli) throw new HostKitError("npm_missing");
    const logFile = path.join(workspace, ".workspace", "logs", "launcher-hostkit-build.log");
    const work = (async () => {
      const env = this.childEnv(tools);
      if (install) {
        this.note("Installing the host kit (npm install)...");
        const code = await this.exec(tools.node, [tools.npmCli!, "install", "--no-audit", "--no-fund"], dir, env, logFile).done;
        if (code !== 0) throw new HostKitError("hostkit_build", failureLine(this.lines));
      }
      this.note("Building the host kit (npm run build)...");
      const code = await this.exec(tools.node, [tools.npmCli!, "run", "build", "--silent"], dir, env, logFile).done;
      if (code !== 0) throw new HostKitError("hostkit_build", failureLine(this.lines));
    })();
    this.building = work.finally(() => (this.building = null));
    await work;
    return tools;
  }

  // A one-shot host kit command (setup, gamedata, invite): node dist/cli.js <args>.
  async command(workspace: string, args: string[], logName: string): Promise<{ code: number; lines: string[]; failure: string | null }> {
    const tools = await this.prepare(workspace);
    const dir = hostKitDir(workspace);
    const own: string[] = [];
    this.note(`> ${args[0]}`);
    const { done } = this.exec(tools.node, [path.join(dir, "dist", "cli.js"), ...args], dir, this.childEnv(tools, { INIT_CWD: dir }), path.join(workspace, ".workspace", "logs", `launcher-${logName}.log`), (line) => {
      own.push(cleanLine(line));
      return true;
    });
    const code = await done;
    return { code, lines: own, failure: code === 0 ? null : failureLine(own) };
  }

  // Another program (git) run the same way, its output on the page and in a log file.
  async program(file: string, args: string[], cwd: string, logFile: string): Promise<{ code: number; lines: string[] }> {
    const own: string[] = [];
    const env: NodeJS.ProcessEnv = { ...this.o.env, GIT_TERMINAL_PROMPT: "0" };
    const { done } = this.exec(file, args, cwd, env, logFile, (line) => {
      own.push(cleanLine(line));
      return true;
    });
    return { code: await done, lines: own };
  }

  // Is a server running on this PC that the launcher did not start?
  async refresh(): Promise<void> {
    if (this.child || this.state === "starting" || this.state === "stopping") return;
    const answers = await (this.o.probe ?? (() => portAnswers(BACKEND_PORT)))();
    if (answers && this.state !== "external") this.set("external", null);
    else if (!answers && this.state === "external") this.set("stopped", null);
  }

  async start(workspace: string): Promise<void> {
    if (this.child || this.state === "starting" || this.state === "stopping") return;
    if (await (this.o.probe ?? (() => portAnswers(BACKEND_PORT)))()) {
      this.set("external", null);
      return;
    }
    this.stopRequested = false;
    this.set("starting", null);
    let tools: NodeTools;
    try {
      tools = await this.prepare(workspace);
    } catch (e) {
      const code = e instanceof HostKitError ? e.code : "hostkit_build";
      this.set("failed", e instanceof HostKitError && e.detail ? e.detail : code);
      throw e;
    }
    const dir = hostKitDir(workspace);
    const own: string[] = [];
    this.note("> start");
    const { child, done } = this.exec(tools.node, [path.join(dir, "dist", "cli.js"), "start"], dir, this.childEnv(tools, { [CONTROL_ENV]: "stdin" }), path.join(workspace, ".workspace", "logs", "launcher-server.log"), (line) => {
      const clean = cleanLine(line);
      if (clean.trim() === SERVER_READY_LINE) {
        if (this.state === "starting") this.set("running", null);
        return false;
      }
      own.push(clean);
      if (own.length > MAX_LINES) own.shift();
      return true;
    });
    this.child = child;
    void done.then((code) => {
      this.child = null;
      if (this.stopRequested) this.set("stopped", null);
      else this.set("failed", failureLine(own) ?? `exit ${code}`);
    });
  }

  // Asks the server to stop; ends its process tree if it does not within the timeout.
  async stop(): Promise<void> {
    const child = this.child;
    if (!child) return;
    this.stopRequested = true;
    this.set("stopping", null);
    const exited = new Promise<boolean>((resolve) => {
      if (child.exitCode !== null) return resolve(true);
      child.once("close", () => resolve(true));
    });
    try {
      child.stdin?.write("stop\n");
      child.stdin?.end();
    } catch {
      /* already closed */
    }
    const timeout = new Promise<boolean>((resolve) => setTimeout(() => resolve(false), this.o.stopTimeoutMs ?? STOP_TIMEOUT_MS).unref?.());
    if (!(await Promise.race([exited, timeout])) && child.pid !== undefined) {
      this.note("The server did not stop in time: ending its processes.");
      await (this.o.killTree ?? defaultKillTree(this.o.platform))(child.pid);
      await Promise.race([exited, new Promise((r) => setTimeout(r, 5000).unref?.())]);
    }
  }
}
