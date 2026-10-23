// `npm run start`: every process of the server, in order, each with its own env file and log, and
// restarted when it stops unexpectedly. Ctrl+C stops them all, the game servers included.
//
//   mongod (if managed)  127.0.0.1:27017
//   backend              127.0.0.1:61000   ParadoxBackend   (API, chat; HTTP only)
//   director             127.0.0.1:61001   ParadoxDirector  (starts Ramsgate, the Training Grounds, hunts on UDP 8770-8777)
//   content              127.0.0.1:61002   ReforgedContent  (the runtime, news, art)
//   allowlist (public)   127.0.0.1:61005   legacy/ReforgedGateway/allowlist (Windows Firewall, administrator)
//   gateway              0.0.0.0:443 (public, TLS) or <Tailscale IP>:61000 (private, HTTP)

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import path from "node:path";
import { readEnvFile } from "./hostconfig";

export interface ServiceSpec {
    name: string;
    cwd: string;
    command: string;
    args: string[];
    envFile: string | null;
    extraEnv?: NodeJS.ProcessEnv;
    // Considered started once this answers (an HTTP GET, or for "tcp" just a connection), or as
    // soon as the process is up when not given.
    ready?: { port: number; path?: string; host?: string; tcp?: boolean };
    // The Director's game servers are its children: always stop the whole tree.
    killTree?: boolean;
}

// What the services get from the shell: the system's own variables only. Our env file decides the
// rest, and nothing left over in the shell (an old MYSTICPARADOX_ or REFORGED_ setting) leaks in.
export function serviceEnvironment(base: NodeJS.ProcessEnv, fromFile: Record<string, string>, extra: NodeJS.ProcessEnv = {}): NodeJS.ProcessEnv {
    const env: NodeJS.ProcessEnv = {};
    for (const [k, v] of Object.entries(base)) {
        if (v === undefined) continue;
        if (/^(REFORGED_|MYSTICPARADOX_|MYSTPAX_|GATEWAY_|ALLOWLIST_|CONTENT_|MONGODB_|NODE_ENV$|PORT$|BIND_HOST$|AUTH_|API_KEY_HASH_SECRET$|GAMESERVER_|METAGAME_|MY_IP$|LOG_LEVEL$|QOS_|REALTIME_|REGISTRATION_MODE$|DEPLOYSERVER_URL$|MATCHMAKING_MODE$|BACKEND_URL$)/i.test(k)) continue;
        env[k] = v;
    }
    return { ...env, ...fromFile, ...extra };
}

export function killProcessTree(child: ChildProcess, platform: NodeJS.Platform = process.platform): void {
    if (child.pid === undefined || child.exitCode !== null) return;
    if (platform === "win32") {
        spawnSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { windowsHide: true, stdio: "ignore" });
        return;
    }
    try {
        // Started detached on POSIX: the child leads its own process group.
        process.kill(-child.pid, "SIGTERM");
    } catch {
        child.kill("SIGTERM");
    }
}

export function waitForHttp(port: number, urlPath: string, timeoutMs: number, host = "127.0.0.1"): Promise<boolean> {
    const until = Date.now() + timeoutMs;
    return new Promise((resolve) => {
        const attempt = () => {
            const req = http.get({ host, port, path: urlPath, timeout: 2000, agent: false }, (res) => {
                res.resume();
                resolve(true);
            });
            req.on("timeout", () => req.destroy());
            req.on("error", () => {
                if (Date.now() > until) resolve(false);
                else setTimeout(attempt, 500);
            });
        };
        attempt();
    });
}

export function waitForTcp(port: number, timeoutMs: number, host = "127.0.0.1"): Promise<boolean> {
    const until = Date.now() + timeoutMs;
    return new Promise((resolve) => {
        const attempt = () => {
            const socket = net.connect({ port, host });
            socket.once("connect", () => {
                socket.destroy();
                resolve(true);
            });
            socket.once("error", () => {
                socket.destroy();
                if (Date.now() > until) resolve(false);
                else setTimeout(attempt, 500);
            });
        };
        attempt();
    });
}

interface Running {
    spec: ServiceSpec;
    child: ChildProcess | null;
    restarts: number;
    lastStart: number;
}

export type Printer = (name: string, line: string) => void;

export class Supervisor {
    private readonly running: Running[] = [];
    private stopping = false;

    constructor(private readonly logDir: string, private readonly print: Printer, private readonly platform: NodeJS.Platform = process.platform) {}

    async start(spec: ServiceSpec, readyTimeoutMs = 60000): Promise<boolean> {
        const entry: Running = { spec, child: null, restarts: 0, lastStart: 0 };
        this.running.push(entry);
        this.launch(entry);
        if (!spec.ready) return true;
        const ok = spec.ready.tcp
            ? await waitForTcp(spec.ready.port, readyTimeoutMs, spec.ready.host)
            : await waitForHttp(spec.ready.port, spec.ready.path ?? "/", readyTimeoutMs, spec.ready.host);
        this.print(spec.name, ok ? `ready on ${spec.ready.host ?? "127.0.0.1"}:${spec.ready.port}` : `did not answer on port ${spec.ready.port} within ${Math.round(readyTimeoutMs / 1000)} s (see its log in ${this.logDir})`);
        return ok;
    }

    private launch(entry: Running): void {
        const { spec } = entry;
        fs.mkdirSync(this.logDir, { recursive: true });
        const log = fs.createWriteStream(path.join(this.logDir, `${spec.name}.log`), { flags: "a" });
        log.write(`\n===== ${new Date().toISOString()} starting ${spec.name}\n`);
        const env = serviceEnvironment(process.env, spec.envFile ? readEnvFile(spec.envFile) : {}, spec.extraEnv);
        const child = spawn(spec.command, spec.args, {
            cwd: spec.cwd,
            env,
            stdio: ["ignore", "pipe", "pipe"],
            windowsHide: true,
            detached: this.platform !== "win32",
        });
        entry.child = child;
        entry.lastStart = Date.now();
        let partial = "";
        const onData = (chunk: Buffer) => {
            log.write(chunk);
            const text = partial + chunk.toString("utf8");
            const lines = text.split(/\r?\n/);
            partial = lines.pop() ?? "";
            for (const line of lines) if (line.trim()) this.print(spec.name, line.slice(0, 400));
        };
        child.stdout?.on("data", onData);
        child.stderr?.on("data", onData);
        child.on("error", (e) => this.print(spec.name, `could not start: ${e.message}`));
        child.on("exit", (code, signal) => {
            log.end(`===== ${new Date().toISOString()} ${spec.name} exited (${code ?? signal})\n`);
            entry.child = null;
            if (this.stopping) return;
            // Restart with backoff; a process that keeps dying right away is given up on.
            const quick = Date.now() - entry.lastStart < 30000;
            entry.restarts = quick ? entry.restarts + 1 : 0;
            if (entry.restarts > 5) {
                this.print(spec.name, `stopped ${entry.restarts} times in a row right after starting; not restarting it. Check ${path.join(this.logDir, `${spec.name}.log`)} and run npm run doctor.`);
                return;
            }
            const delay = Math.min(30000, 2000 * 2 ** entry.restarts);
            this.print(spec.name, `exited (${code ?? signal}); restarting in ${Math.round(delay / 1000)} s`);
            setTimeout(() => {
                if (!this.stopping) this.launch(entry);
            }, delay);
        });
    }

    // Reverse order: the gateway first, so no player request reaches a service that is going away.
    async stopAll(): Promise<void> {
        this.stopping = true;
        for (const entry of [...this.running].reverse()) {
            const child = entry.child;
            if (!child) continue;
            const exited = new Promise<void>((resolve) => child.once("exit", () => resolve()));
            if (entry.spec.killTree || this.platform === "win32") killProcessTree(child, this.platform);
            else child.kill("SIGTERM");
            await Promise.race([exited, new Promise((r) => setTimeout(r, 8000))]);
            if (child.exitCode === null) killProcessTree(child, this.platform);
        }
    }
}
