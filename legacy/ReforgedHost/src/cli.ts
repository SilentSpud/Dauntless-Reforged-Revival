// ReforgedHost: the host's command line.
//
//   npm run setup -- --mode private [--host <Tailscale IP>] --name "Friday Hunts" --game "D:\Games\Dauntless"
//   npm run setup -- --mode public --host 203.0.113.7 --name "Friday Hunts" --game "D:\Games\Dauntless"
//   npm run setup -- --mode public --host 203.0.113.7 --name "Friday Hunts" --game /srv/dauntless/Dauntless   (Linux; docs/hosting-linux.md)
//   npm run start
//   npm run invite -- [--uses 1] [--for "Alex"] [--unlimited] [--list] [--json]
//   npm run doctor
//   npm run gamedata [-- --examples | --example-sources | --progression | --export [file] | --import <file>]
//   npm run publish-runtime [-- --from <folder with MysticParadox.dll and winmm.dll>]
//   npm run preset [-- --create [--generate] [--name "<name>"] | --apply [--force]]
//   npm run multipliers [-- --xp 5 --rewards 5 | --off]   (private test servers only)
//   npm run upstream   (only downloads and patches Mystic Paradox: what importing the SDK needs)
//   npm run update [-- --check] [--json] [--no-setup]   (fast-forwards this checkout to GitHub, then runs setup again)

import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { portInUse, portOwnerHint, printChecks, runDoctor, tailscaleIPv4 } from "./doctor";
import { BACKEND_GAME_DATA, DIRECTOR_GAME_DATA, gameDataState, generateGameData, useExamples } from "./gamedata";
import { defaultPackFile, exportGameDataPack, importGameDataPack } from "./gamepack";
import { applyPreset, createPreset, readPresetManifest } from "./preset";
import {
    DEFAULT_MONGO_URI,
    buildEnvFiles,
    gamePortsText,
    gatewayPort,
    isPublicIPv4,
    isTailscaleIPv4,
    loadOrCreateSecrets,
    MAX_MULTIPLIER,
    multipliersOn,
    readEnvFile,
    readHostConfig,
    writeEnvFiles,
    writeHostConfig,
    LINUX_INTERFACE,
    performanceOptions,
    validFps,
    validMegabytes,
    validMinutes,
    type HostConfig,
    type LinuxOptions,
    type PerformanceOptions,
} from "./hostconfig";
import { certificateFingerprint, createInviteCode, inviteFor, listInviteCodes } from "./invite";
import { ensureReplicaSet, findMongod, mongodArgs } from "./mongo";
import { findRepoRoot, GAME_EXE_RELATIVE, paths, PORTS, type Paths } from "./paths";
import { ensureServerFlags, installServerRuntime, publishRuntime } from "./publish";
import { capture, run } from "./run";
import { Supervisor, type ServiceSpec } from "./services";
import { applyUpdate, checkForUpdate, UpdateError, type UpdateCheck } from "./selfupdate";
import { ensureUpstream, gitAvailable, readPin } from "./upstream";
import {
    canBindPort,
    canChangeFirewall,
    isRoot,
    ensureWinePrefix,
    gameRuntime,
    linuxFirewallScript,
    linuxOptions,
    UNIT_ALLOWLIST,
    UNIT_SERVER,
    writeGameserverWrapper,
    writeSystemdUnits,
} from "./linux";
import { findOnPath } from "./wine";
import { waitForHttp } from "./services";

type Args = { _: string[]; [k: string]: string | boolean | string[] };

export function parseArgs(argv: string[]): Args {
    const out: Args = { _: [] };
    for (let i = 0; i < argv.length; i++) {
        const a = argv[i];
        if (!a.startsWith("--")) {
            out._.push(a);
            continue;
        }
        const eq = a.indexOf("=");
        if (eq > 0) {
            out[a.slice(2, eq)] = a.slice(eq + 1);
            continue;
        }
        const next = argv[i + 1];
        if (next !== undefined && !next.startsWith("--")) {
            out[a.slice(2)] = next;
            i++;
        } else out[a.slice(2)] = true;
    }
    return out;
}

function str(args: Args, name: string): string | undefined {
    const v = args[name];
    return typeof v === "string" && v.trim() !== "" ? v.trim() : undefined;
}

function say(line = ""): void {
    process.stdout.write(line + "\n");
}

function fail(message: string): never {
    process.stderr.write(`\nError: ${message}\n`);
    process.exit(1);
}

const onLinux = process.platform === "linux";

function gitHelp(): string {
    return onLinux ? "install it with: sudo apt install git" : "install Git for Windows from https://git-scm.com";
}

function isWindowsAdmin(): boolean {
    if (process.platform !== "win32") return true;
    return spawnSync("net", ["session"], { windowsHide: true, stdio: "ignore" }).status === 0;
}

// ------------------------------------------------------------------ setup

// Linux: the server runs as an ordinary user. The game servers face the internet, and Wine refuses a
// prefix another user owns; the systemd units give the gateway and the allowlist the one privilege
// each needs.
const NOT_AS_ROOT = "do not run the server as root. Make a user for it (sudo adduser reforged), sign in as that user (sudo -iu reforged) and run this there; docs/hosting-linux.md shows every step";

// --xp-multiplier / --reward-multiplier on setup (or --xp / --rewards on `multipliers`): a number from 1 to 100.
function multiplierArg(args: Args, names: string[], fallback: number): number {
    const raw = names.map((n) => str(args, n)).find((v) => v !== undefined);
    if (raw === undefined) return fallback;
    const value = Number(raw.replace(/^x/i, ""));
    if (!Number.isFinite(value) || value < 1 || value > MAX_MULTIPLIER) throw new Error(`--${names[0]} must be a number from 1 to ${MAX_MULTIPLIER} (got ${raw})`);
    return value;
}

export function multipliersFromArgs(args: Args, mode: HostConfig["mode"], existing: HostConfig["multipliers"], names = { xp: ["xp-multiplier"], rewards: ["reward-multiplier"] }): HostConfig["multipliers"] {
    if (args.off === true || args["no-multipliers"] === true) return undefined;
    const xp = multiplierArg(args, names.xp, existing?.xp ?? 1);
    const rewards = multiplierArg(args, names.rewards, existing?.rewards ?? 1);
    if (xp === 1 && rewards === 1) return undefined;
    if (mode !== "private") throw new Error("XP and reward multipliers are for private (Tailscale) test servers only");
    return { xp, rewards };
}

// Linux: --wine <path or name>, --proton <script> (--no-proton), --no-xvfb (--xvfb), --firewall-interface <eth0>
// (--no-firewall-interface). Each one keeps what host.json had when it is not given.
export function linuxFromArgs(args: Args, existing: LinuxOptions | undefined, find: (name: string) => string | null = (n) => findOnPath([n])): LinuxOptions {
    const program = (name: string, what: string): string => {
        const found = path.posix.isAbsolute(name) ? name : find(name);
        if (!found) throw new Error(`--${what} ${name}: not found (give its full path)`);
        return found;
    };
    const wineArg = str(args, "wine");
    const protonArg = str(args, "proton");
    const iface = str(args, "firewall-interface");
    if (iface !== undefined && !LINUX_INTERFACE.test(iface)) throw new Error(`--firewall-interface must be a network interface name like eth0 (got ${iface})`);
    return {
        wine: wineArg !== undefined ? program(wineArg, "wine") : existing?.wine ?? null,
        proton: args["no-proton"] === true ? null : protonArg !== undefined ? program(protonArg, "proton") : existing?.proton ?? null,
        xvfb: args["no-xvfb"] === true ? false : args.xvfb === true ? true : existing?.xvfb ?? true,
        firewallInterface: args["no-firewall-interface"] === true ? null : iface ?? existing?.firewallInterface ?? null,
    };
}

// --public-port <port>: the gateway's port when 443 is taken (a hosting panel); --public-port 443 goes back.
export function publicPortFromArgs(args: Args, mode: HostConfig["mode"], existing: HostConfig | null): { publicPort?: number } {
    if (mode !== "public") {
        if (str(args, "public-port") !== undefined) throw new Error("--public-port is for public servers only");
        return {};
    }
    const raw = str(args, "public-port");
    const port = raw === undefined ? existing?.mode === "public" ? existing.publicPort : undefined : Number(raw);
    if (port === undefined || port === PORTS.publicGateway) return {};
    if (!Number.isInteger(port)) throw new Error(`--public-port must be a TCP port number (got ${raw})`);
    return { publicPort: port };
}

// --server-fps <0|10-240>, --idle-fps <0|1-240>, --training-on-demand (--training-at-boot),
// --verbose-game-logs (--quiet-game-logs), --memory-guard (--no-memory-guard), --min-free-ram <MB>,
// --hunt-ram <MB>, --training-idle-minutes <0-1440>. Each one keeps what host.json had when it is not given.
export function performanceFromArgs(args: Args, existing: HostConfig["performance"]): PerformanceOptions {
    const current = performanceOptions({ performance: existing });
    const fps = (name: string, min: number, fallback: number, meaning: string): number => {
        const raw = str(args, name);
        if (raw === undefined) return fallback;
        const value = Number(raw);
        if (!validFps(value, min)) throw new Error(`--${name} must be 0 (${meaning}) or ${min} to 240 frames per second (got ${raw})`);
        return value;
    };
    const minutes = (name: string, fallback: number): number => {
        const raw = str(args, name);
        if (raw === undefined) return fallback;
        const value = Number(raw);
        if (!validMinutes(value)) throw new Error(`--${name} must be 0 (keep them running) or 1 to 1440 minutes (got ${raw})`);
        return value;
    };
    const megabytes = (name: string, min: number, fallback: number): number => {
        const raw = str(args, name);
        if (raw === undefined) return fallback;
        const value = Number(raw.replace(/\s*mb$/i, ""));
        if (!validMegabytes(value, min)) throw new Error(`--${name} must be a whole number of megabytes, at least ${min} (got ${raw})`);
        return value;
    };
    return {
        serverFps: fps("server-fps", 10, current.serverFps, "no cap"),
        idleFps: fps("idle-fps", 1, current.idleFps, "the same as --server-fps"),
        trainingOnDemand: args["training-at-boot"] === true ? false : args["training-on-demand"] === true ? true : current.trainingOnDemand,
        trainingIdleMinutes: minutes("training-idle-minutes", current.trainingIdleMinutes),
        verboseGameLogs: args["quiet-game-logs"] === true ? false : args["verbose-game-logs"] === true ? true : current.verboseGameLogs,
        memoryGuard: args["no-memory-guard"] === true ? false : args["memory-guard"] === true ? true : current.memoryGuard,
        minFreeMemoryMb: megabytes("min-free-ram", 0, current.minFreeMemoryMb),
        huntMemoryMb: megabytes("hunt-ram", 256, current.huntMemoryMb),
    };
}

export function trainingText(perf: Pick<PerformanceOptions, "trainingOnDemand" | "trainingIdleMinutes">): string {
    if (!perf.trainingOnDemand) return "at boot";
    return perf.trainingIdleMinutes > 0 ? `on demand, stopped after ${perf.trainingIdleMinutes} min without players` : "on demand";
}

export function performanceSummary(c: HostConfig): string {
    const perf = performanceOptions(c);
    const fps = perf.serverFps > 0 ? `${perf.serverFps} fps` : "no frame cap";
    const idle = perf.idleFps > 0 ? `${perf.idleFps} fps after 15 s without players` : "no idle cap";
    const memory = perf.memoryGuard ? `a new hunt only with ${perf.huntMemoryMb + perf.minFreeMemoryMb} MB free` : "no memory guard";
    return `Game servers: ${fps}, ${idle}; Training Grounds ${trainingText(perf)}; ${perf.verboseGameLogs ? "verbose" : "quiet"} game logs; at most ${c.maxHuntServers} hunt${c.maxHuntServers === 1 ? "" : "s"} at once (UDP ${gamePortsText(c)}), ${memory}`;
}

export function hostConfigFromArgs(args: Args, existing: HostConfig | null, detect: { tailscale: () => string | null; mongod: () => string | null; platform?: NodeJS.Platform }): HostConfig {
    const mode = (str(args, "mode") ?? existing?.mode) as HostConfig["mode"] | undefined;
    if (mode !== "private" && mode !== "public") throw new Error("say which kind of server: --mode private (friends over Tailscale) or --mode public (a server on the internet)");
    // A private server follows this PC's Tailscale address, which changes with the Tailscale account:
    // setup without --host takes the current one, and only falls back to the old one when Tailscale is
    // not running.
    const current = mode === "private" ? detect.tailscale() : null;
    const host = str(args, "host") ?? current ?? (existing?.mode === mode ? existing.host : undefined);
    if (!host && mode === "private") {
        throw new Error("Tailscale is not running here, so this PC's Tailscale address is unknown. Start Tailscale, or pass --host <your Tailscale IP>");
    }
    if (!host) throw new Error("a public server needs --host <its public IPv4 address or DNS name>");
    let gameAddress = str(args, "game-address") ?? (existing?.mode === mode && existing.host === host ? existing.gameAddress : undefined);
    if (!gameAddress) {
        if (mode === "private") gameAddress = isTailscaleIPv4(host) ? host : detect.tailscale() ?? undefined;
        else gameAddress = isPublicIPv4(host) ? host : undefined;
    }
    if (!gameAddress) throw new Error(mode === "private" ? "pass --game-address <your Tailscale IPv4>" : "the host is a DNS name: pass --game-address <the server's public IPv4> as well (the game servers announce an address, not a name)");
    const mongoUri = str(args, "mongo-uri") ?? existing?.mongo.uri ?? DEFAULT_MONGO_URI;
    const manage = args["no-manage-mongo"] === true ? false : args["manage-mongo"] === true ? true : existing?.mongo.manage ?? mongoUri === DEFAULT_MONGO_URI;
    const registration = (str(args, "registration")?.toUpperCase() ?? existing?.registration ?? "INVITECODE") as HostConfig["registration"];
    const gameArg = str(args, "game");
    // A full path in Windows or POSIX form (tests run on Linux with Windows paths).
    if (gameArg !== undefined && !path.win32.isAbsolute(gameArg) && !path.posix.isAbsolute(gameArg)) {
        throw new Error(`--game needs the full path of the Dauntless 1.14.7 folder, like "D:\\Games\\Dauntless" (got ${gameArg})`);
    }
    const gameDir = gameArg ?? existing?.gameDir ?? null;
    return {
        version: 1,
        mode,
        host,
        gameAddress,
        name: str(args, "name") ?? existing?.name ?? "Dauntless Reforged Revival server",
        gameDir: gameDir ? path.resolve(gameDir) : null,
        registration,
        shareUrl: mode === "private" ? str(args, "share") ?? existing?.shareUrl ?? null : null,
        mongo: { uri: mongoUri, manage, mongod: str(args, "mongod") ?? existing?.mongo.mongod ?? (manage ? detect.mongod() : null) },
        maxHuntServers: Number(str(args, "max-hunts") ?? existing?.maxHuntServers ?? 3),
        multipliers: multipliersFromArgs(args, mode, existing?.mode === mode ? existing.multipliers : undefined),
        ...publicPortFromArgs(args, mode, existing),
        performance: performanceFromArgs(args, existing?.performance),
        ...((detect.platform ?? process.platform) === "linux" ? { linux: linuxFromArgs(args, existing?.linux) } : existing?.linux ? { linux: existing.linux } : {}),
    };
}

// The launcher's Host page starts the server with REFORGED_CONTROL=stdin and its stdin a pipe: a "stop"
// line, or the pipe closing (the launcher quit), stops the server as Ctrl+C does. Windows has no signal a
// program can send another for that. The launcher also reads SERVER_READY_LINE from the output.
export const CONTROL_ENV = "REFORGED_CONTROL";
export const SERVER_READY_LINE = "REFORGED_SERVER_READY";

export function watchStopInput(input: NodeJS.ReadableStream, onStop: () => void): void {
    let buffered = "";
    let stopped = false;
    const once = () => {
        if (stopped) return;
        stopped = true;
        onStop();
    };
    input.setEncoding?.("utf8");
    input.on("data", (chunk: string | Buffer) => {
        buffered += chunk.toString();
        const lines = buffered.split(/\r?\n/);
        buffered = lines.pop() ?? "";
        if (lines.some((l) => l.trim().toLowerCase() === "stop")) once();
    });
    input.on("end", once);
    input.on("close", once);
}

export function firewallScript(c: HostConfig, nodeExe: string): string {
    const exe = c.gameDir ? path.join(c.gameDir, GAME_EXE_RELATIVE) : null;
    const lines = [
        "# Dauntless Reforged Revival: Windows Firewall rules for this server. Written by `npm run setup`.",
        "# Run once in PowerShell as administrator:  powershell -ExecutionPolicy Bypass -File firewall.ps1",
        "$ErrorActionPreference = 'Stop'",
        "function Add-Rule($Name, $Display, $Protocol, $Port, $Program, $Remote, $Enabled) {",
        "    if (Get-NetFirewallRule -Name $Name -ErrorAction SilentlyContinue) { Write-Host \"$Display already exists\"; return }",
        "    $Params = @{ Name = $Name; DisplayName = $Display; Direction = 'Inbound'; Action = 'Allow'; Protocol = $Protocol; LocalPort = $Port; Profile = 'Any'; Enabled = $Enabled }",
        "    if ($Program) { $Params.Program = $Program }",
        "    if ($Remote) { $Params.RemoteAddress = $Remote }",
        "    New-NetFirewallRule @Params | Out-Null",
        "    Write-Host \"added $Display\"",
        "}",
    ];
    const q = (s: string) => `'${s.replace(/'/g, "''")}'`;
    if (c.mode === "public") {
        lines.push(
            `Add-Rule 'ReforgedRevival-Gateway' 'Dauntless Reforged Revival gateway (TCP ${gatewayPort(c)})' 'TCP' '${gatewayPort(c)}' ${q(nodeExe)} $null 'True'`,
            "# The game ports start closed: the allowlist helper opens them only for players who logged in.",
            `Add-Rule 'ReforgedRevival-GamePorts-Allowlist' 'Dauntless Reforged Revival game ports (allowlist)' 'UDP' '${gamePortsText(c)}' ${exe ? q(exe) : "$null"} $null 'False'`,
        );
    } else {
        lines.push(
            "# Only from Tailscale addresses: nobody else can reach these.",
            `Add-Rule 'ReforgedRevival-Gateway-Tailnet' 'Dauntless Reforged Revival gateway (TCP ${PORTS.privateGateway}, Tailscale)' 'TCP' '${PORTS.privateGateway}' ${q(nodeExe)} '100.64.0.0/10' 'True'`,
            `Add-Rule 'ReforgedRevival-GamePorts-Tailnet' 'Dauntless Reforged Revival game ports (Tailscale)' 'UDP' '${gamePortsText(c)}' ${exe ? q(exe) : "$null"} '100.64.0.0/10' 'True'`,
        );
    }
    return lines.join("\r\n") + "\r\n";
}

async function npmInstallAndBuild(dir: string, logFile: string, label: string): Promise<void> {
    say(`- ${label}: npm ci`);
    let r = await run("npm", ["ci", "--no-audit", "--no-fund"], { cwd: dir, logFile, quiet: true });
    if (r.code !== 0) fail(`npm ci failed in ${dir}: ${r.lastLine ?? r.code} (full output in ${logFile})`);
    say(`- ${label}: npm run build`);
    r = await run("npm", ["run", "build"], { cwd: dir, logFile, quiet: true });
    if (r.code !== 0) fail(`npm run build failed in ${dir}: ${r.lastLine ?? r.code} (full output in ${logFile})`);
}

async function setup(p: Paths, args: Args): Promise<void> {
    if (!gitAvailable()) fail(`git is needed (it downloads Mystic Paradox at the pinned commit): ${gitHelp()}`);
    // A value with spaces that was not quoted arrives in pieces (--game D:\Games\My, then Dauntless).
    if (args._.length > 1) fail(`unexpected "${args._.slice(1).join(" ")}": put a value with spaces in quotes, for example --game "D:\\Games\\Dauntless 1.14.7\\Dauntless"`);
    if (onLinux && isRoot()) fail(NOT_AS_ROOT);
    const logFile = path.join(p.logs, "setup.log");
    fs.mkdirSync(p.logs, { recursive: true });

    let existing: HostConfig | null = null;
    try {
        existing = readHostConfig(p.hostJson);
    } catch (e) {
        say(`(ignoring the old host.json: ${e instanceof Error ? e.message : String(e)})`);
    }
    let config: HostConfig;
    try {
        config = hostConfigFromArgs(args, existing, { tailscale: tailscaleIPv4, mongod: () => findMongod() });
        writeHostConfig(p.hostJson, config);
        config = readHostConfig(p.hostJson) as HostConfig;
    } catch (e) {
        fail(e instanceof Error ? e.message : String(e));
    }
    say(`\n${config.mode === "public" ? "Public" : "Private"} server "${config.name}" at ${config.host} (game servers on ${config.gameAddress})`);
    if (multipliersOn(config)) say(`Test multipliers ON: XP x${config.multipliers!.xp}, rewards x${config.multipliers!.rewards} (npm run multipliers -- --off turns them off)`);
    say(performanceSummary(config));

    say("\n1. Mystic Paradox at the pinned commit, with the Reforged patches");
    const pin = readPin(p.upstreamPin);
    try {
        const result = await ensureUpstream({ dir: p.upstream, markerFile: p.marker, pin, patchesRoot: p.patchesDir, logFile, force: args["force-upstream"] === true });
        say(`- ${result === "already_ready" ? "already in place" : result === "fetched" ? "downloaded and patched" : "updated and patched (your SDK, game data and exports were kept)"}: ${pin.commit.slice(0, 12)} in ${p.upstream}`);
    } catch (e) {
        fail(e instanceof Error ? e.message : String(e));
    }

    // The project's preset (preset/): the game data this server starts from, where none of its own is there yet.
    try {
        const applied = applyPreset(p);
        if (applied) say(`- server preset "${applied.name}": ${applied.written.length} game-data files taken from preset/${applied.kept.length ? `, ${applied.kept.length} of your own kept` : ""}`);
    } catch (e) {
        say(`! The server preset in preset/ was not applied: ${e instanceof Error ? e.message : String(e)}`);
    }

    if (args["skip-install"] !== true) {
        say("\n2. Installing and building the services (a few minutes the first time)");
        await npmInstallAndBuild(p.backendDir, logFile, "backend");
        await npmInstallAndBuild(p.directorDir, logFile, "Director");
        await npmInstallAndBuild(p.gatewayDir, logFile, "gateway");
        await npmInstallAndBuild(p.contentDir, logFile, "content server");
    } else say("\n2. Skipped installing and building (--skip-install)");

    say("\n3. Configuration");
    const { secrets, created } = loadOrCreateSecrets(p.config);
    say(`- secrets: ${created ? "made (kept from now on)" : "kept"} in ${path.join(p.config, "secrets.env")}`);
    for (const dir of [p.runtimePublish, p.branding, p.gameserverLogs, p.mongoData, p.rawGameData]) fs.mkdirSync(dir, { recursive: true });
    if (!fs.existsSync(p.newsFile)) fs.writeFileSync(p.newsFile, JSON.stringify({ items: [] }, null, 2) + "\n");
    const sourceCommit = capture("git", ["rev-parse", "--short=12", "HEAD"], p.repo) ?? "";
    const written = writeEnvFiles(p, buildEnvFiles(config, secrets, p, { sourceCommit }));
    say(`- env files: ${written.map((f) => path.basename(f)).join(", ")} in ${p.config}`);
    if (onLinux) {
        const firewall = path.join(p.config, "firewall.sh");
        fs.writeFileSync(firewall, linuxFirewallScript(config), { mode: 0o755 });
        say(`- host firewall (ufw), once: sudo sh ${firewall}`);
        say(`- game-server launcher (Wine): ${writeGameserverWrapper(p)}`);
        try {
            writeSystemdUnits(config, p);
            say(`- systemd units (run the server at boot), once: sudo sh ${path.join(p.systemd, "install.sh")}`);
        } catch (e) {
            say(`! systemd units not written: ${e instanceof Error ? e.message : String(e)}`);
        }
    } else {
        fs.writeFileSync(path.join(p.config, "firewall.ps1"), firewallScript(config, process.execPath));
        say(`- firewall rules to add once (as administrator): ${path.join(p.config, "firewall.ps1")}`);
    }

    if (config.mode === "public") {
        if (!fs.existsSync(p.certFile)) {
            const hosts = [...new Set([config.host, config.gameAddress])].flatMap((h) => ["--host", h]);
            const r = await run(process.execPath, [path.join(p.gatewayDir, "tools", "make-cert.js"), ...hosts, "--out", p.tls, "--json"], { cwd: p.gatewayDir, logFile, quiet: true });
            if (r.code !== 0) fail(`could not make the gateway certificate: ${r.lastLine}`);
            say(`- gateway certificate made in ${p.tls}`);
        } else say(`- gateway certificate kept (a new one would change every invite): ${p.certFile}`);
        say(`- certificate fingerprint (goes into every invite): ${certificateFingerprint(p.certFile)}`);
    }

    if (onLinux) await linuxSetup(p, config, logFile);

    if (config.mongo.manage && !config.mongo.mongod) say(`\n! MongoDB was not found. Install MongoDB Community Server (${onLinux ? "docs/hosting-linux.md" : "https://www.mongodb.com/try/download/community"}), then run setup again, or pass --mongod <path to ${onLinux ? "mongod" : "mongod.exe"}>.`);
    if (!config.gameDir) say("\n! No game folder yet: pass --game \"<your Dauntless 1.14.7 folder>\" (the game servers run from it).");
    else if (!fs.existsSync(path.join(config.gameDir, GAME_EXE_RELATIVE))) say(`\n! No ${GAME_EXE_RELATIVE} in ${config.gameDir}: pass --game "<the Dauntless folder that holds Archon>" (the game servers run from it).`);

    say("\nSetup done. Next:");
    if (onLinux) {
        say("  1. The game runtime and the game data come from a Windows PC with your own 1.14.7 (docs/hosting-linux.md):");
        say("     npm run publish-runtime -- --from <the folder with MysticParadox.dll and winmm.dll>");
        say("     npm run gamedata -- --import <the game data pack>");
        say(`  2. Once: sudo sh ${path.join(p.config, "firewall.sh")}`);
        say(`  3. Run it (and at every boot): sudo sh ${path.join(p.systemd, "install.sh")}   Logs: journalctl -u ${UNIT_SERVER} -f`);
        if (config.mode === "private") say("     (a private server also runs in a terminal: npm run start)");
        say("  4. npm run invite -- --for \"Alex\"");
        say("  npm run doctor shows what is still missing.");
        return;
    }
    say("  1. The SDK, the runtime and the game data, from your own game: docs/sdk-and-game-data.md");
    say("     (the launcher's Host tools page does most of it: Settings > Advanced > Host tools)");
    say("  2. npm run publish-runtime");
    say("  3. npm run start");
    say("  4. npm run invite -- --for \"Alex\"");
    say("  npm run doctor shows what is still missing.");
}

// Linux: what runs the Windows game servers, checked and prepared once.
async function linuxSetup(p: Paths, config: HostConfig, logFile: string): Promise<void> {
    const lx = linuxOptions(config);
    say("\n4. Linux: the game servers run through Wine");
    const runtime = gameRuntime(lx);
    if (!runtime) {
        say("! Wine was not found: sudo apt install wine wine64 (docs/hosting-linux.md), then run setup again. Or pass --wine <path> or --proton <script>.");
    } else {
        say(`- ${runtime.kind === "proton" ? "Proton" : "Wine"}: ${runtime.path}`);
        if (runtime.kind === "wine") {
            say("- Wine prefix: preparing it (the first time takes a minute or two)...");
            const r = await ensureWinePrefix(p, lx, logFile);
            say(r.ok ? `- Wine prefix ${r.created ? "made" : "ready"}: ${p.winePrefix}` : `! Wine prefix: ${r.detail}`);
        }
    }
    if (lx.xvfb && !findOnPath(["xvfb-run"])) say("! xvfb-run was not found (the game servers need a virtual display): sudo apt install xvfb xauth");
    if (config.mode === "public" && !fs.existsSync("/usr/sbin/nft") && !fs.existsSync("/sbin/nft")) say("! nftables was not found (the allowlist needs it): sudo apt install nftables");
}

// ------------------------------------------------------------------ start

async function start(p: Paths): Promise<void> {
    let loaded: HostConfig | null;
    try {
        loaded = readHostConfig(p.hostJson);
    } catch (e) {
        fail(e instanceof Error ? e.message : String(e));
    }
    if (!loaded) fail("not set up yet: run npm run setup first");
    const config: HostConfig = loaded;
    for (const name of ["backend", "director", "content", "gateway"]) {
        if (!fs.existsSync(path.join(p.config, `${name}.env`))) fail(`${name}.env is missing: run npm run setup`);
    }
    // Linux, public: the reforged-allowlist systemd unit runs the allowlist (with CAP_NET_ADMIN), and the
    // reforged unit gives this process CAP_NET_BIND_SERVICE for a gateway port below 1024 (443).
    const externalAllowlist = onLinux && config.mode === "public" && process.env.REFORGED_ALLOWLIST === "external";
    if (onLinux) {
        if (isRoot()) fail(NOT_AS_ROOT);
        if (!fs.existsSync(p.gameserverWrapper)) fail("the game-server launcher is missing: run npm run setup");
        const asService = `run it as a systemd service: sudo sh ${path.join(p.systemd, "install.sh")} (then journalctl -u ${UNIT_SERVER} -f shows it)`;
        if (config.mode === "public" && !canBindPort(gatewayPort(config))) fail(`a public server listens on port ${gatewayPort(config)}, which needs a privilege: ${asService}`);
        if (config.mode === "public" && !externalAllowlist && !canChangeFirewall()) fail(`a public server's allowlist changes the firewall (nftables), which needs a privilege: ${asService}`);
    } else if (config.mode === "public" && !isWindowsAdmin()) {
        fail(`a public server changes Windows Firewall rules (the allowlist helper) and listens on port ${gatewayPort(config)}: run this terminal as administrator`);
    }
    // The gateway and the game servers listen on the Tailscale address: one that changed (another
    // Tailscale account) would only fail later, with a bind error.
    if (config.mode === "private") {
        const now = tailscaleIPv4();
        if (now && now !== config.gameAddress) {
            fail(`this PC's Tailscale address changed from ${config.gameAddress} to ${now} (a new Tailscale account?). Run setup again (the launcher: Host > Server > Update server; a terminal: npm run setup), then make new invites: the old ones name the old address`);
        }
    }
    const gd = [gameDataState(p.backendGameData, BACKEND_GAME_DATA), gameDataState(p.directorGameData, DIRECTOR_GAME_DATA)];
    if (gd.some((g) => g.state === "missing")) fail("game data is missing: npm run gamedata (or npm run gamedata -- --examples for a start-up test only)");
    if (gd.some((g) => g.state === "synthetic")) say("! This server runs on example game data: it starts, but it is not playable. The launcher tells players so.");
    if (!fs.existsSync(path.join(p.runtimePublish, "runtime.json"))) say("! No runtime published yet: players can join but not play. Build it and run npm run publish-runtime.");
    const win64 = config.gameDir ? path.dirname(path.join(config.gameDir, GAME_EXE_RELATIVE)) : null;
    if (win64 && fs.existsSync(win64)) {
        const flags = ensureServerFlags(win64);
        if (flags.length) say(`Turned on for the game servers: ${flags.join(", ")} (in ${win64})`);
    }

    const width = 9;
    const supervisor = new Supervisor(p.logs, (name, line) => process.stdout.write(`${name.padEnd(width)}| ${line}\n`));
    const node = process.execPath;

    let stopping = false;
    const stop = async () => {
        if (stopping) return;
        stopping = true;
        say("\nStopping...");
        await supervisor.stopAll();
        if (config.mode === "public" && !externalAllowlist) {
            // A hard stop never lets the helper close the game ports itself (Dauntless Revived's README).
            await run(node, [path.join(p.gatewayDir, "dist", "allowlist", "server.js"), "--close-ports"], { cwd: p.gatewayDir, quiet: true, env: { ...process.env, ...readEnvForProcess(p, "allowlist") } });
        }
        say("Stopped.");
        process.exit(0);
    };
    process.on("SIGINT", () => void stop());
    process.on("SIGTERM", () => void stop());
    process.on("SIGBREAK", () => void stop());
    const controlled = process.env[CONTROL_ENV] === "stdin";
    if (controlled) watchStopInput(process.stdin, () => void stop());

    if (config.mongo.manage) {
        const mongod = config.mongo.mongod ?? findMongod();
        if (!mongod) fail("MongoDB (mongod) was not found: install MongoDB Community Server, or run setup with --mongod <path>");
        if (await portInUse(PORTS.mongo)) {
            // A mongod left running by an earlier start is a replica set and can be used. The Windows
            // service the MongoDB installer adds by default is not, and the backend needs one.
            const there = await ensureReplicaSet(p.backendDir, config.mongo.uri, { initiate: false, timeoutMs: 4000 });
            if (!there.ok) {
                fail(`port ${PORTS.mongo} is already taken${there.reason === "standalone" ? " by a MongoDB without a replica set, usually the MongoDB Windows service" : ""}. The host kit runs its own MongoDB: stop the other one first. For the Windows service, in PowerShell as administrator: Stop-Service MongoDB; Set-Service MongoDB -StartupType Disabled`);
            }
            say(`- MongoDB is already running on port ${PORTS.mongo} (replica set ${there.setName}): using it`);
        } else {
            fs.mkdirSync(p.mongoData, { recursive: true });
            await supervisor.start({ name: "mongod", cwd: p.workspace, command: mongod, args: mongodArgs(p.mongoData, path.join(p.logs, "mongod-server.log"), PORTS.mongo), envFile: null, ready: { port: PORTS.mongo, tcp: true } }, 60000);
        }
    }
    const backendEnv = readEnvForProcess(p, "backend");
    const mongo = await ensureReplicaSet(p.backendDir, backendEnv.MONGODB_URI ?? config.mongo.uri, { initiate: true, timeoutMs: 8000 });
    if (!mongo.ok) {
        await supervisor.stopAll();
        fail(`MongoDB: ${mongo.detail}`);
    }
    if (mongo.initiated) say(`mongod   | replica set ${mongo.setName} initiated`);

    // MongoDB may be running by now: stop it before giving up.
    const abort = async (message: string): Promise<never> => {
        await supervisor.stopAll();
        fail(message);
    };
    if (onLinux) {
        const lx = linuxOptions(config);
        if (gameRuntime(lx)?.kind === "wine") {
            const prefix = await ensureWinePrefix(p, lx, path.join(p.logs, "wine.log"));
            if (prefix.created) say("wine     | Wine prefix made");
            if (!prefix.ok) say(`wine     | ! ${prefix.detail}`);
        } else if (!gameRuntime(lx)) say("wine     | ! Wine was not found: the game servers cannot start (sudo apt install wine wine64)");
        if (config.mode === "public") {
            if (externalAllowlist) {
                // Its table must be in place (game ports closed) before the Director starts any game server.
                if (!(await waitForHttp(PORTS.allowlist, "/status", 30000))) await abort(`the allowlist (${UNIT_ALLOWLIST}) is not answering on port ${PORTS.allowlist}: systemctl status ${UNIT_ALLOWLIST}`);
                say("allowlist| the reforged-allowlist service is running");
            } else {
                // Linux has no firewall closing the game ports by default: close them before any game server starts.
                const closed = await run(node, [path.join(p.gatewayDir, "dist", "allowlist", "server.js"), "--close-ports"], { cwd: p.gatewayDir, quiet: true, env: { ...process.env, ...readEnvForProcess(p, "allowlist") } });
                if (closed.code !== 0) await abort(`could not close the game ports (nftables): ${closed.lastLine}`);
            }
        }
    }

    const specs: ServiceSpec[] = [
        { name: "backend", cwd: p.backendDir, command: node, args: ["build/server.js"], envFile: path.join(p.config, "backend.env"), ready: { port: PORTS.backend, path: "/dauntless-status" } },
        { name: "director", cwd: p.directorDir, command: node, args: ["dist/server.js"], envFile: path.join(p.config, "director.env"), ready: { port: PORTS.director, path: "/api/instances" }, killTree: true },
        { name: "content", cwd: p.contentDir, command: node, args: ["dist/server.js"], envFile: path.join(p.config, "content.env"), ready: { port: PORTS.content, path: "/content/v1/news" } },
    ];
    if (config.mode === "public" && !externalAllowlist) {
        specs.push({ name: "allowlist", cwd: p.gatewayDir, command: node, args: ["dist/allowlist/server.js"], envFile: path.join(p.config, "allowlist.env"), ready: { port: PORTS.allowlist, path: "/status" } });
    }
    specs.push({
        name: "gateway",
        cwd: p.gatewayDir,
        command: node,
        args: ["dist/server.js"],
        envFile: path.join(p.config, "gateway.env"),
        ready: config.mode === "public" ? { port: gatewayPort(config), tcp: true } : { port: PORTS.privateGateway, host: config.gameAddress, tcp: true },
    });
    for (const spec of specs) {
        // Readiness is "something answers on the port": a port another program already holds would pass
        // it while this service fails to listen (a hosting panel on 443 then answers players with its own
        // certificate, and every invite fails). So a taken port stops the start here.
        if (spec.ready && (await portInUse(spec.ready.port, spec.ready.host))) {
            const move = spec.name === "gateway" && config.mode === "public" ? ", or give the gateway a free port: npm run setup -- --public-port 4443 (then new invites)" : "";
            await abort(`port ${spec.ready.port} (${spec.name}) is already used by another program. ${portOwnerHint(spec.ready.port)}, then stop that program (or a server still running from before) and start again${move}`);
        }
        const ok = await supervisor.start(spec, spec.name === "backend" ? 120000 : 60000);
        if (!ok && (spec.name === "backend" || spec.name === "gateway")) {
            say(`\n${spec.name} did not come up. Its log: ${path.join(p.logs, `${spec.name}.log`)}`);
            await stop();
            return;
        }
    }
    say("");
    say(`Server "${config.name}" is up: ${config.mode === "public" ? `https://${config.host}:${PORTS.publicGateway} (pinned TLS)` : `http://${config.host}:${PORTS.privateGateway} inside your tailnet`}`);
    if (controlled) {
        say(SERVER_READY_LINE);
        return;
    }
    say("Make an invite in another terminal: npm run invite -- --for \"Alex\"");
    say("Ctrl+C stops everything.");
}

function readEnvForProcess(p: Paths, name: string): Record<string, string> {
    return readEnvFile(path.join(p.config, `${name}.env`));
}

// ------------------------------------------------------------------ test multipliers

// Changes only the multipliers in host.json and rewrites the env files; the backend reads them when it starts.
function multipliers(p: Paths, args: Args): void {
    let config: HostConfig | null;
    try {
        config = readHostConfig(p.hostJson);
    } catch (e) {
        fail(e instanceof Error ? e.message : String(e));
    }
    if (!config) fail("not set up yet: run npm run setup first");
    const asked = args.off === true || ["xp", "rewards", "xp-multiplier", "reward-multiplier"].some((n) => str(args, n) !== undefined);
    if (asked) {
        try {
            config = { ...config, multipliers: multipliersFromArgs(args, config.mode, config.multipliers, { xp: ["xp", "xp-multiplier"], rewards: ["rewards", "reward-multiplier"] }) };
            writeHostConfig(p.hostJson, config);
        } catch (e) {
            fail(e instanceof Error ? e.message : String(e));
        }
        const { secrets } = loadOrCreateSecrets(p.config);
        const sourceCommit = capture("git", ["rev-parse", "--short=12", "HEAD"], p.repo) ?? "";
        writeEnvFiles(p, buildEnvFiles(config, secrets, p, { sourceCommit }));
    }
    if (multipliersOn(config)) say(`Test multipliers ON: XP x${config.multipliers!.xp}, rewards x${config.multipliers!.rewards}.`);
    else say(`Test multipliers off.${config.mode === "public" ? " (They are for private Tailscale servers only.)" : ""}`);
    if (asked) say("Restart the server (Ctrl+C, then npm run start) for the backend to use them. Its log says [TestMultiplier] ON or off.");
}

// ------------------------------------------------------------------ Mystic Paradox only

// Setup's first step alone: the pinned Mystic Paradox, patched, in .workspace. Importing the SDK (and
// building the runtime) needs only this, so a host can do those before the server is set up.
async function upstream(p: Paths): Promise<void> {
    if (!gitAvailable()) fail(`git is needed (it downloads Mystic Paradox at the pinned commit): ${gitHelp()}`);
    const pin = readPin(p.upstreamPin);
    try {
        const result = await ensureUpstream({ dir: p.upstream, markerFile: p.marker, pin, patchesRoot: p.patchesDir, logFile: path.join(p.logs, "upstream.log") });
        say(`Mystic Paradox ${result === "already_ready" ? "already in place" : result === "fetched" ? "downloaded and patched" : "updated and patched (your SDK, game data and exports were kept)"}: ${pin.commit.slice(0, 12)} in ${p.upstream}`);
    } catch (e) {
        fail(e instanceof Error ? e.message : String(e));
    }
}

// ------------------------------------------------------------------ updating the project

const UPDATE_PROBLEMS: Record<UpdateError["code"], string> = {
    not_git: "this folder is not a git clone of the project: clone it again with git (see docs/hosting.md) and move .workspace into the new clone",
    no_upstream: "this checkout does not follow a branch on GitHub (git checkout main, then try again)",
    fetch_failed: "could not reach GitHub",
    diverged: "this checkout has commits of its own that GitHub does not: update it by hand (git pull)",
    local_changes: "files of the project were changed on this PC and the update would overwrite them (git status lists them; git checkout -- <file> undoes a change)",
    merge_failed: "git could not apply the update",
};

function describeCheck(c: UpdateCheck): string {
    return c.behind === 0 ? `Up to date: ${c.branch} at ${c.head}.` : `${c.behind} update${c.behind === 1 ? "" : "s"} on GitHub for ${c.branch} (${c.head} -> ${c.remoteHead}).`;
}

// Compares this checkout with GitHub and fast-forwards it; then setup runs again (it re-applies the
// patches and installs what changed). --check only compares. --json ends with one JSON line (the launcher).
async function update(p: Paths, args: Args): Promise<void> {
    const json = args.json === true;
    const logFile = path.join(p.logs, "update.log");
    try {
        if (!gitAvailable()) throw new Error(`git is needed to update: ${gitHelp()}`);
        if (args.check === true) {
            const c = await checkForUpdate(p.repo, { logFile });
            say(describeCheck(c));
            if (json) say(JSON.stringify(c));
            return;
        }
        say("Comparing this project with GitHub...");
        const r = await applyUpdate(p.repo, { logFile });
        if (!r.updated) say(describeCheck(r));
        else {
            say(`Updated ${r.branch}: ${r.from} -> ${r.to} (${r.files} file${r.files === 1 ? "" : "s"} changed).`);
            if (r.runtimeChanged) say("The game runtime changed: build MysticParadox.dll again and publish it (npm run publish-runtime) for the players.");
        }
        if (json) say(JSON.stringify(r));
        if (!r.updated || args["no-setup"] === true) return;
        const kit = p.hostKit;
        // node_modules is missing right after the update that moved the kit to legacy/.
        if (r.hostKitDeps || !fs.existsSync(path.join(kit, "node_modules"))) {
            say("\nThe host kit's dependencies changed: npm install");
            if ((await run("npm", ["install", "--no-audit", "--no-fund"], { cwd: kit, logFile })).code !== 0) fail(`npm install failed (see ${logFile})`);
        }
        if (!readHostConfig(p.hostJson)) return say("Now run npm run setup.");
        say("\nRunning setup again with this server's settings (npm run setup)...");
        const s = await run("npm", ["run", "setup"], { cwd: kit });
        if (s.code !== 0) fail(`setup failed: ${s.lastLine ?? `exit ${s.code}`}`);
    } catch (e) {
        if (e instanceof UpdateError) {
            if (json) say(JSON.stringify({ error: e.code, detail: e.detail }));
            fail(`${UPDATE_PROBLEMS[e.code]}${e.detail ? ` (${e.detail})` : ""}`);
        }
        fail(e instanceof Error ? e.message : String(e));
    }
}

// ------------------------------------------------------------------ invite, gamedata, runtime

async function invite(p: Paths, args: Args): Promise<void> {
    const config = readHostConfig(p.hostJson);
    if (!config) fail("not set up yet: run npm run setup first");
    const { secrets } = loadOrCreateSecrets(p.config);
    if (args.list === true) {
        say(JSON.stringify(await listInviteCodes(secrets.adminKey), null, 2));
        return;
    }
    const uses = Number(str(args, "uses") ?? "1");
    if (!Number.isInteger(uses) || uses < 1 || uses > 1000) fail("--uses must be a whole number from 1 to 1000");
    let code: string;
    try {
        code = await createInviteCode(secrets.adminKey, { uses, infinite: args.unlimited === true, note: str(args, "for") });
    } catch (e) {
        fail(e instanceof Error ? e.message : String(e));
    }
    const fp = config.mode === "public" ? certificateFingerprint(p.certFile) : null;
    const link = inviteFor(config, code, fp);
    if (args.json === true) {
        // For the launcher's Host page: one line it can read.
        say(JSON.stringify({ link, for: str(args, "for") ?? null, uses: args.unlimited === true ? null : uses, shareUrl: config.mode === "private" ? config.shareUrl : null }));
        return;
    }
    say(`\nInvite${str(args, "for") ? ` for ${str(args, "for")}` : ""} (${args.unlimited === true ? "unlimited uses" : `${uses} use${uses === 1 ? "" : "s"}`}):\n`);
    say(`  ${link}\n`);
    say("Send the whole line. They paste it into the Dauntless Reforged Revival launcher (or click it once the launcher is installed).");
    if (config.mode === "private") say(`They also need Tailscale, and access to this PC: share it from https://login.tailscale.com/admin/machines${config.shareUrl ? "" : " (or put the share link in invites with npm run setup -- --share <link>)"}.`);
}

async function gamedata(p: Paths, args: Args): Promise<void> {
    if (!fs.existsSync(p.backendDir)) fail("not set up yet: run npm run setup first");
    if (args.export !== undefined) {
        // Relative to where npm was run from (INIT_CWD), not ReforgedHost.
        const out = typeof args.export === "string" ? path.resolve(process.env.INIT_CWD ?? process.cwd(), args.export) : defaultPackFile(p);
        let r;
        try {
            r = exportGameDataPack(p, out);
        } catch (e) {
            fail(e instanceof Error ? e.message : String(e));
        }
        say(`Game data pack written: ${r.file}`);
        say(`  ${r.included.length} files${r.left.length ? `; not in it (missing, or still the example): ${r.left.join(", ")}` : ""}`);
        say("Keep it for your own servers: it is Phoenix Labs' game data from your game, not for publishing.");
        say("On a new setup: npm run gamedata -- --import <this file>");
        return;
    }
    if (args.import !== undefined) {
        if (typeof args.import !== "string") fail("--import needs the pack file: npm run gamedata -- --import <file>");
        let r;
        try {
            r = importGameDataPack(p, path.resolve(process.env.INIT_CWD ?? process.cwd(), args.import));
        } catch (e) {
            fail(e instanceof Error ? e.message : String(e));
        }
        say(`Game data pack imported: ${r.written.length} files written, ${r.unchanged.length} already the same${r.backups.length ? ` (${r.backups.length} replaced files kept as .bak)` : ""}`);
        for (const [label, dir, files] of [["backend", p.backendGameData, BACKEND_GAME_DATA], ["Director", p.directorGameData, DIRECTOR_GAME_DATA]] as const) {
            const g = gameDataState(dir, files);
            say(`${label}: ${g.state}${g.missing.length ? ` (missing ${g.missing.join(", ")})` : ""}${g.synthetic.length ? ` (example data in ${g.synthetic.join(", ")})` : ""}`);
        }
        return;
    }
    if (args.examples === true) {
        const copied = [...useExamples(p.backendGameData, BACKEND_GAME_DATA), ...useExamples(p.directorGameData, DIRECTOR_GAME_DATA)];
        say(copied.length ? `Copied the example data for ${copied.join(", ")}.` : "Nothing was missing.");
        if (copied.length) say("Example data lets the services start for a test. It is not playable: export the real data from your game (docs/sdk-and-game-data.md).");
        return;
    }
    say("Generating the game data from your export (Items_Analysis):");
    // --progression: only the progression config (from the progression export), nothing else rewritten.
    const only = args.progression === true ? ["Progression config"] : undefined;
    const r = await generateGameData(p, { examplesForMissing: args["example-sources"] === true, logFile: path.join(p.logs, "gamedata.log"), print: say, only });
    for (const [label, dir, files] of [["backend", p.backendGameData, BACKEND_GAME_DATA], ["Director", p.directorGameData, DIRECTOR_GAME_DATA]] as const) {
        const g = gameDataState(dir, files);
        say(`${label}: ${g.state}${g.missing.length ? ` (missing ${g.missing.join(", ")})` : ""}${g.synthetic.length ? ` (example data in ${g.synthetic.join(", ")})` : ""}`);
    }
    if (!r.ok) process.exit(1);
}

async function preset(p: Paths, args: Args): Promise<void> {
    if (args.create !== undefined) {
        if (!fs.existsSync(p.backendDir)) fail("not set up yet: run npm run setup first");
        if (args.generate === true) {
            say("Generating the game data from your export (Items_Analysis) first:");
            const r = await generateGameData(p, { examplesForMissing: false, logFile: path.join(p.logs, "gamedata.log"), print: say });
            if (!r.ok) say(`! Not every generator ran (${r.failed.join(", ")}); the preset takes the files that are there.`);
        }
        const config = readHostConfig(p.hostJson);
        const exe = config?.gameDir ? path.join(config.gameDir, GAME_EXE_RELATIVE) : null;
        const name = typeof args.name === "string" && args.name.trim() ? args.name.trim() : config?.name ?? "Dauntless Reforged Revival";
        let r;
        try {
            r = createPreset(p, { name, executable: exe });
        } catch (e) {
            fail(e instanceof Error ? e.message : String(e));
        }
        say(`Server preset "${name}" written to ${p.preset}: ${r.manifest.files.length} files`);
        for (const f of r.manifest.files) say(`  ${f.path}  ${f.bytes} bytes`);
        if (r.left.length) say(`  not in it (missing, or still the example): ${r.left.join(", ")}`);
        say(`  never in a preset (built from the store captures on each server): ${r.excluded.join(", ")}`);
        say(r.manifest.executableSha256 ? `  read from the game executable ${r.manifest.executableSha256}` : "  ! no game folder in host.json, so the executable's SHA-256 was not recorded");
        say("Every server set up from this checkout now starts from it (npm run setup applies it). To ship it with the project, commit preset/.");
        say("It is data read from Phoenix Labs' game: committing it to a public repository publishes it (see docs/sdk-and-game-data.md).");
        return;
    }
    if (args.apply === true) {
        if (!fs.existsSync(p.backendDir)) fail("not set up yet: run npm run setup first");
        let r;
        try {
            r = applyPreset(p, { force: args.force === true });
        } catch (e) {
            fail(e instanceof Error ? e.message : String(e));
        }
        if (!r) fail(`no server preset in ${p.preset} (make one with npm run preset -- --create)`);
        say(`Server preset "${r.name}": ${r.written.length} files written, ${r.unchanged.length} already the same${r.backups.length ? ` (${r.backups.length} replaced files kept as .bak)` : ""}`);
        if (r.kept.length) say(`  your own game data kept (npm run preset -- --apply --force replaces it): ${r.kept.join(", ")}`);
        return;
    }
    let m;
    try {
        m = readPresetManifest(p);
    } catch (e) {
        fail(e instanceof Error ? e.message : String(e));
    }
    if (!m) {
        say(`No server preset in ${p.preset} yet. Make one from your game data: npm run preset -- --create [--generate]`);
        return;
    }
    say(`Server preset "${m.name}" for Dauntless ${m.game}, made ${m.created}${m.executableSha256 ? `, from the executable ${m.executableSha256.slice(0, 16)}...` : ""}:`);
    for (const f of m.files) say(`  ${f.path}  ${f.bytes} bytes`);
    say("npm run setup applies it; npm run preset -- --apply [--force] applies it now.");
}

// --from <folder>: MysticParadox.dll and winmm.dll from that folder instead of this checkout's builds
// (a Linux server's runtime is built on a Windows PC and copied over).
function publish(p: Paths, args: Args): void {
    const from = args.from;
    if (from === true) fail("--from needs the folder that holds MysticParadox.dll and winmm.dll");
    const dir = typeof from === "string" ? path.resolve(process.env.INIT_CWD ?? process.cwd(), from) : null;
    const sources = dir ? { "MysticParadox.dll": path.join(dir, "MysticParadox.dll"), "winmm.dll": path.join(dir, "winmm.dll") } : undefined;
    let runtime;
    try {
        runtime = publishRuntime(p, new Date(), sources);
    } catch (e) {
        fail(e instanceof Error ? e.message : String(e));
    }
    say(`Runtime ${runtime.version} published in ${p.runtimePublish}:`);
    for (const f of runtime.files) say(`  ${f.name}  ${f.size} bytes  ${f.sha256}`);
    say("Players see these hashes and accept them once in the launcher.");
    const config = readHostConfig(p.hostJson);
    if (config?.gameDir) {
        const win64 = path.dirname(path.join(config.gameDir, GAME_EXE_RELATIVE));
        const r = installServerRuntime(p, win64);
        if (r.installed.length) say(`Installed for the game servers in ${win64}: ${r.installed.join(", ")}`);
        if (r.inUse.length) say(`Game servers still running keep the old ${r.inUse.join(", ")} until the server restarts: restart it so every world uses the new runtime.`);
        if (r.failed.length) say(`! Could not replace ${r.failed.map((f) => f.name).join(", ")} in ${win64} (another program holds it: stop the server and run this again): ${r.failed[0].error}`);
        ensureServerFlags(win64);
    }
}

async function main(): Promise<void> {
    const args = parseArgs(process.argv.slice(2));
    const command = args._[0] ?? "help";
    const p = paths(findRepoRoot());
    switch (command) {
        case "setup":
            return setup(p, args);
        case "start":
            return start(p);
        case "invite":
            return invite(p, args);
        case "doctor": {
            const { fails, warns } = printChecks(await runDoctor(p, { hashExe: args["no-hash"] !== true }), say);
            say(`\n${fails} problem${fails === 1 ? "" : "s"}, ${warns} warning${warns === 1 ? "" : "s"}.`);
            process.exit(fails ? 1 : 0);
        }
        case "gamedata":
            return gamedata(p, args);
        case "publish-runtime":
            return publish(p, args);
        case "preset":
            return preset(p, args);
        case "multipliers":
            return multipliers(p, args);
        case "update":
            return update(p, args);
        case "upstream":
            return upstream(p);
        default:
            say("Dauntless Reforged Revival host kit. Commands (from the ReforgedHost folder):");
            say("  npm run setup -- --mode private|public [--host <address>] --name \"<server name>\" --game \"<Dauntless 1.14.7 folder>\"");
            say("                   [--game-address <IPv4>] [--registration INVITECODE|OPEN|NONE] [--share <Tailscale share link>]");
            say("                   [--mongo-uri <uri>] [--no-manage-mongo] [--mongod <path>] [--max-hunts 1-10] [--skip-install] [--force-upstream]");
            say("                   public: [--public-port <port>] (when 443 is taken, by a hosting panel for example)");
            say("                   Linux: [--wine <path>] [--proton <script> | --no-proton] [--no-xvfb | --xvfb] [--firewall-interface <eth0> | --no-firewall-interface]");
            say("                   CPU/RAM: [--server-fps 0|10-240] [--idle-fps 0|1-240] [--training-on-demand | --training-at-boot] [--quiet-game-logs | --verbose-game-logs]");
            say("                            [--memory-guard | --no-memory-guard] [--min-free-ram <MB>] [--hunt-ram <MB>] [--training-idle-minutes 0-1440]");
            say("  npm run start");
            say("  npm run invite -- [--uses N] [--unlimited] [--for \"<who>\"] [--list] [--json]");
            say("  npm run doctor [-- --no-hash]");
            say("  npm run gamedata [-- --examples | --example-sources | --progression | --export [file] | --import <file>]");
            say("  npm run publish-runtime [-- --from <folder with MysticParadox.dll and winmm.dll>]");
            say("  npm run preset [-- --create [--generate] [--name \"<name>\"] | --apply [--force]]");
            say("  npm run multipliers [-- --xp 1-100 --rewards 1-100 | --off]   (private test servers; also setup's --xp-multiplier/--reward-multiplier)");
            say("  npm run upstream   (downloads and patches Mystic Paradox only)");
            say("  npm run update [-- --check]   (compares this project with GitHub, updates it and runs setup again)");
            say("See README.md and ../../docs/hosting.md.");
    }
}

if (require.main === module) {
    main().catch((e) => fail(e instanceof Error ? e.stack ?? e.message : String(e)));
}
