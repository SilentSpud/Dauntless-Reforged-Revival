// Linux (Debian 13 and the like): what a Linux server needs beyond the Windows host kit.
//   - the launcher the Director starts for each game server (wine.ts), as .workspace/bin/reforged-gameserver
//   - privileges: a public server listens on TCP 443 and its allowlist changes nftables
//   - systemd units that give each part only the privilege it needs, and run the server at boot
//   - firewall.sh: lets a host firewall (ufw) pass the server's traffic
// Adapted from Dauntless Revived's Linux port (deploy/linux-server/).

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { gamePortsText, gatewayPort, type HostConfig, type LinuxOptions } from "./hostconfig";
import { PORTS, type Paths } from "./paths";
import { run } from "./run";
import { dllOverrides, findOnPath, findWine, winePrefixReady } from "./wine";

export const CAP_NET_BIND_SERVICE = 10;
export const CAP_NET_ADMIN = 12;

export const DEFAULT_LINUX: LinuxOptions = { wine: null, proton: null, xvfb: true, firewallInterface: null };

export function linuxOptions(c: HostConfig): LinuxOptions {
    return c.linux ?? DEFAULT_LINUX;
}

// ------------------------------------------------------------------ privileges

// CapEff from /proc/<pid>/status: the capabilities this process has now.
export function effectiveCapabilities(statusText: string): bigint {
    const m = /^CapEff:\s*([0-9a-f]+)\s*$/im.exec(statusText);
    return m ? BigInt(`0x${m[1]}`) : 0n;
}

function readText(file: string): string | null {
    try {
        return fs.readFileSync(file, "utf8");
    } catch {
        return null;
    }
}

export function hasCapability(bit: number, statusText: string | null = readText("/proc/self/status")): boolean {
    if (statusText === null) return false;
    return (effectiveCapabilities(statusText) & (1n << BigInt(bit))) !== 0n;
}

export function isRoot(): boolean {
    return typeof process.getuid === "function" && process.getuid() === 0;
}

// Can this process listen on a port below 1024: root, CAP_NET_BIND_SERVICE, or a system that lets
// everyone (net.ipv4.ip_unprivileged_port_start).
export function canBindPort(port: number, opts: { root?: boolean; status?: string | null; unprivilegedStart?: string | null } = {}): boolean {
    if (port >= 1024) return true;
    if (opts.root ?? isRoot()) return true;
    const start = Number((opts.unprivilegedStart !== undefined ? opts.unprivilegedStart : readText("/proc/sys/net/ipv4/ip_unprivileged_port_start"))?.trim() ?? "1024");
    if (Number.isInteger(start) && start <= port) return true;
    return hasCapability(CAP_NET_BIND_SERVICE, opts.status !== undefined ? opts.status : readText("/proc/self/status"));
}

export function canChangeFirewall(opts: { root?: boolean; status?: string | null } = {}): boolean {
    if (opts.root ?? isRoot()) return true;
    return hasCapability(CAP_NET_ADMIN, opts.status !== undefined ? opts.status : readText("/proc/self/status"));
}

// ------------------------------------------------------------------ the game-server launcher

// A POSIX shell word: single-quoted, with ' written as '\''.
export function shQuote(s: string): string {
    return `'${s.replace(/'/g, `'\\''`)}'`;
}

export function gameserverWrapperScript(node: string, hostKit: string): string {
    return [
        "#!/bin/sh",
        "# Dauntless Reforged Revival: the Director starts this for each game server. Written by `npm run setup`.",
        "# It runs Dauntless-Win64-Shipping.exe through Wine or Proton (ReforgedHost/src/wine.ts).",
        `exec ${shQuote(node)} ${shQuote(path.join(hostKit, "dist", "gameserver.js"))} "$@"`,
        "",
    ].join("\n");
}

export function writeGameserverWrapper(p: Paths, node: string = process.execPath): string {
    fs.mkdirSync(p.bin, { recursive: true });
    const tmp = `${p.gameserverWrapper}.tmp`;
    fs.writeFileSync(tmp, gameserverWrapperScript(node, p.hostKit), { mode: 0o755 });
    fs.chmodSync(tmp, 0o755);
    fs.renameSync(tmp, p.gameserverWrapper);
    return p.gameserverWrapper;
}

// What runs the game servers, as setup and doctor describe it. Null when nothing was found.
export function gameRuntime(lx: LinuxOptions, env: NodeJS.ProcessEnv = process.env): { kind: "wine" | "proton"; path: string } | null {
    if (lx.proton) return { kind: "proton", path: lx.proton };
    const wine = lx.wine ?? findWine(env);
    return wine ? { kind: "wine", path: wine } : null;
}

// The first start of Wine makes its prefix (a minute or more): done once here, so it does not eat
// into the first game server's readiness timeout. Proton makes its own on its first run.
export async function ensureWinePrefix(p: Paths, lx: LinuxOptions, logFile: string): Promise<{ ok: boolean; created: boolean; detail?: string }> {
    if (lx.proton) return { ok: true, created: false };
    if (winePrefixReady(p.winePrefix)) return { ok: true, created: false };
    const wine = lx.wine ?? findWine();
    if (!wine) return { ok: false, created: false, detail: "Wine was not found (apt install wine)" };
    fs.mkdirSync(p.winePrefix, { recursive: true });
    const env: NodeJS.ProcessEnv = { ...process.env, WINEPREFIX: p.winePrefix, WINEDEBUG: "-all", WINEDLLOVERRIDES: dllOverrides(process.env.WINEDLLOVERRIDES) };
    const xvfb = lx.xvfb && !(process.env.DISPLAY ?? "").trim() ? findOnPath(["xvfb-run"]) : null;
    const [command, args] = xvfb ? [xvfb, ["-a", wine, "wineboot", "--init"]] : [wine, ["wineboot", "--init"]];
    const r = await run(command, args, { cwd: p.workspace, env, logFile, quiet: true });
    // wineboot returns before wineserver has written everything: wait for it.
    const server = findOnPath(["wineserver"]);
    if (server) await run(server, ["-w"], { cwd: p.workspace, env, logFile, quiet: true });
    if (!winePrefixReady(p.winePrefix)) return { ok: false, created: false, detail: `wineboot did not make the prefix (exit ${r.code}${r.lastLine ? `: ${r.lastLine}` : ""}; see ${logFile})` };
    return { ok: true, created: true };
}

// ------------------------------------------------------------------ systemd

// One word of an ExecStart= line: double-quoted, with systemd's specifiers (%) and variables ($) escaped.
export function unitWord(s: string): string {
    if (/[\n\r]/.test(s)) throw new Error("a path in a systemd unit cannot hold a line break");
    return `"${s.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/%/g, "%%").replace(/\$/g, "$$$$")}"`;
}

// A path setting (WorkingDirectory=): taken as is, only specifiers (%) expanded.
export function unitPath(s: string): string {
    if (/[\n\r]/.test(s)) throw new Error("a path in a systemd unit cannot hold a line break");
    return s.replace(/%/g, "%%");
}

export interface UnitContext {
    user: string;
    node: string;
}

export function unitContext(): UnitContext {
    return { user: os.userInfo().username, node: process.execPath };
}

export const UNIT_SERVER = "reforged.service";
export const UNIT_ALLOWLIST = "reforged-allowlist.service";

// The units for this server. Both run as the user who ran setup, never as root:
//   reforged-allowlist (public): the allowlist helper, with CAP_NET_ADMIN only (nftables)
//   reforged: `npm run start`'s supervisor (MongoDB, backend, Director and its game servers,
//             content server, gateway), with CAP_NET_BIND_SERVICE only in public mode (port 443)
export function systemdUnits(c: HostConfig, p: Paths, ctx: UnitContext): Record<string, string> {
    if (ctx.user === "root") throw new Error("run setup as the user the server should run as, not as root (the units run as that user)");
    const pub = c.mode === "public";
    const files: Record<string, string> = {};
    const allowlistEnv = path.join(p.config, "allowlist.env");
    const allowlistMain = path.join(p.gatewayDir, "dist", "allowlist", "server.js");
    if (pub) {
        files[UNIT_ALLOWLIST] = [
            "# Dauntless Reforged Revival: the game-port allowlist (nftables). Written by `npm run setup`.",
            "[Unit]",
            "Description=Dauntless Reforged Revival game-port allowlist (nftables)",
            "After=network-online.target",
            "Wants=network-online.target",
            `Before=${UNIT_SERVER}`,
            "",
            "[Service]",
            "Type=simple",
            `User=${ctx.user}`,
            `WorkingDirectory=${unitPath(p.gatewayDir)}`,
            `ExecStart=${unitWord(ctx.node)} ${unitWord(`--env-file=${allowlistEnv}`)} ${unitWord(allowlistMain)}`,
            "# Whatever happened, the game ports end closed.",
            `ExecStopPost=${unitWord(ctx.node)} ${unitWord(`--env-file=${allowlistEnv}`)} ${unitWord(allowlistMain)} --close-ports`,
            "Restart=on-failure",
            "RestartSec=3",
            "TimeoutStopSec=30",
            "AmbientCapabilities=CAP_NET_ADMIN",
            "CapabilityBoundingSet=CAP_NET_ADMIN",
            "NoNewPrivileges=true",
            "",
            "[Install]",
            "WantedBy=multi-user.target",
            "",
        ].join("\n");
    }
    files[UNIT_SERVER] = [
        "# Dauntless Reforged Revival: the whole server (npm run start). Written by `npm run setup`.",
        "[Unit]",
        `Description=Dauntless Reforged Revival server`,
        `After=network-online.target${pub ? ` ${UNIT_ALLOWLIST}` : ""}`,
        "Wants=network-online.target",
        ...(pub ? [`Requires=${UNIT_ALLOWLIST}`] : []),
        "",
        "[Service]",
        "Type=simple",
        `User=${ctx.user}`,
        `WorkingDirectory=${unitPath(p.hostKit)}`,
        ...(pub ? ["# The allowlist is its own unit (above): start only waits for it.", "Environment=REFORGED_ALLOWLIST=external"] : []),
        `ExecStart=${unitWord(ctx.node)} ${unitWord(path.join(p.hostKit, "dist", "cli.js"))} start`,
        "# SIGTERM to start only: it stops the gateway first and every game server with the Director.",
        "KillMode=mixed",
        "KillSignal=SIGTERM",
        "TimeoutStopSec=120",
        "Restart=on-failure",
        "RestartSec=10",
        // A port below 1024 (443) needs CAP_NET_BIND_SERVICE; nothing else does.
        ...(pub && gatewayPort(c) < 1024 ? ["AmbientCapabilities=CAP_NET_BIND_SERVICE", "CapabilityBoundingSet=CAP_NET_BIND_SERVICE"] : ["CapabilityBoundingSet="]),
        "NoNewPrivileges=true",
        "LimitNOFILE=65536",
        "",
        "[Install]",
        "WantedBy=multi-user.target",
        "",
    ].join("\n");
    const units = pub ? [UNIT_ALLOWLIST, UNIT_SERVER] : [UNIT_SERVER];
    files["install.sh"] = [
        "#!/bin/sh",
        "# Dauntless Reforged Revival: installs this server's systemd units and starts them. Written by `npm run setup`.",
        "# Run once (and again after setup changes them):  sudo sh install.sh",
        "set -eu",
        "[ \"$(id -u)\" -eq 0 ] || { echo \"run it with sudo: sudo sh $0\" >&2; exit 1; }",
        "here=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)",
        ...(pub ? [] : [`if [ -f /etc/systemd/system/${UNIT_ALLOWLIST} ]; then systemctl disable --now ${UNIT_ALLOWLIST} || true; rm -f /etc/systemd/system/${UNIT_ALLOWLIST}; fi`]),
        `for unit in ${units.join(" ")}; do`,
        "    install -m 0644 \"$here/$unit\" \"/etc/systemd/system/$unit\"",
        "done",
        "systemctl daemon-reload",
        `systemctl enable ${units.join(" ")}`,
        `systemctl restart ${units.join(" ")}`,
        `echo "Started. Status: systemctl status ${UNIT_SERVER}   Logs: journalctl -u ${UNIT_SERVER} -f"`,
        `echo "Stop: sudo systemctl stop ${UNIT_SERVER}   Start: sudo systemctl start ${UNIT_SERVER}"`,
        "",
    ].join("\n");
    return files;
}

export function writeSystemdUnits(c: HostConfig, p: Paths, ctx: UnitContext = unitContext()): string[] {
    const files = systemdUnits(c, p, ctx);
    fs.mkdirSync(p.systemd, { recursive: true });
    for (const name of fs.readdirSync(p.systemd)) {
        if (!(name in files)) fs.rmSync(path.join(p.systemd, name), { force: true });
    }
    const written: string[] = [];
    for (const [name, text] of Object.entries(files)) {
        const file = path.join(p.systemd, name);
        fs.writeFileSync(file, text, { mode: name.endsWith(".sh") ? 0o755 : 0o644 });
        written.push(file);
    }
    return written;
}

export function systemdInstalled(unit: string = UNIT_SERVER): boolean {
    return fs.existsSync(path.join("/etc/systemd/system", unit));
}

// ------------------------------------------------------------------ host firewall

// The allowlist's own nftables table already keeps the game ports closed to everyone who has not
// logged in. This only opens the way through a host firewall (ufw), when one is active.
export function linuxFirewallScript(c: HostConfig): string {
    const game = gamePortsText(c, ":");
    const lines = [
        "#!/bin/sh",
        "# Dauntless Reforged Revival: lets a host firewall (ufw) pass this server's traffic. Written by `npm run setup`.",
        "# Run once:  sudo sh firewall.sh",
        "set -eu",
        "[ \"$(id -u)\" -eq 0 ] || { echo \"run it with sudo: sudo sh $0\" >&2; exit 1; }",
        "if command -v ufw >/dev/null 2>&1 && ufw status | grep -q 'Status: active'; then",
    ];
    if (c.mode === "public") {
        lines.push(
            `    ufw allow ${gatewayPort(c)}/tcp comment 'Dauntless Reforged Revival gateway'`,
            "    # The allowlist's nftables table (reforged_revival) still closes these to everyone who has not logged in.",
            `    ufw allow ${game}/udp comment 'Dauntless Reforged Revival game ports (allowlist)'`,
            "    echo 'ufw: added the rules'",
            "else",
            "    echo 'No active ufw: nothing to add here (the allowlist manages the game ports itself).'",
            "fi",
            `echo "Also allow inbound TCP ${gatewayPort(c)} and UDP ${gamePortsText(c)} in your provider's firewall, if it has one (Hostinger: the VPS's Firewall page in hPanel)."`,
        );
    } else {
        lines.push(
            `    ufw allow in on tailscale0 to any port ${PORTS.privateGateway} proto tcp comment 'Dauntless Reforged Revival gateway (Tailscale)'`,
            `    ufw allow in on tailscale0 to any port ${game} proto udp comment 'Dauntless Reforged Revival game ports (Tailscale)'`,
            "    echo 'ufw: added the rules'",
            "else",
            `    echo 'No active ufw. The game ports (UDP ${gamePortsText(c)}) are reachable from every network: block them outside Tailscale, for example with ufw (ufw allow OpenSSH; ufw allow in on tailscale0; ufw enable).'`,
            "fi",
        );
    }
    return lines.join("\n") + "\n";
}
