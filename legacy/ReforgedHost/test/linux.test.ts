import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { describe, it } from "node:test";
import { hostConfigFromArgs, linuxFromArgs, parseArgs } from "../src/cli";
import { buildEnvFiles, DEFAULT_MONGO_URI, newSecrets, validateHostConfig, type HostConfig } from "../src/hostconfig";
import {
    canBindPort,
    canChangeFirewall,
    effectiveCapabilities,
    gameserverWrapperScript,
    linuxFirewallScript,
    shQuote,
    systemdUnits,
    UNIT_ALLOWLIST,
    UNIT_SERVER,
    unitWord,
    writeGameserverWrapper,
} from "../src/linux";
import { GAME_EXE_RELATIVE, paths } from "../src/paths";
import { buildLaunch, dllOverrides, signalGroup, WINE_ENV } from "../src/wine";
import { tempDir, write } from "./helpers";

const OnLinux = process.platform === "linux";

const PUBLIC: HostConfig = {
    version: 1,
    mode: "public",
    host: "203.0.113.7",
    gameAddress: "203.0.113.7",
    name: "Freeza Hunts",
    gameDir: "/srv/dauntless/Dauntless",
    registration: "INVITECODE",
    shareUrl: null,
    mongo: { uri: DEFAULT_MONGO_URI, manage: true, mongod: "/usr/bin/mongod" },
    maxHuntServers: 3,
    linux: { wine: null, proton: null, xvfb: true, firewallInterface: "eth0" },
};
const PRIVATE: HostConfig = { ...PUBLIC, mode: "private", host: "100.101.102.103", gameAddress: "100.101.102.103", linux: { ...PUBLIC.linux!, firewallInterface: null } };

function get(entries: [string, string][], key: string): string | undefined {
    return entries.find(([k]) => k === key)?.[1];
}

function script(file: string, text: string): string {
    write(file, text);
    fs.chmodSync(file, 0o755);
    return file;
}

// A fake xvfb-run (drops -a and -s <args>, runs the rest) and a fake wine that prints what it got.
function fakeTools(dir: string): { wine: string; xvfb: string; exe: string } {
    const xvfb = script(path.join(dir, "bin", "xvfb-run"), [
        "#!/bin/sh",
        "while [ $# -gt 0 ]; do case \"$1\" in -a) shift ;; -s) shift 2 ;; *) break ;; esac; done",
        "echo \"xvfb-run: $1\"",
        "exec \"$@\"",
        "",
    ].join("\n"));
    const wine = script(path.join(dir, "bin", "wine"), [
        "#!/bin/sh",
        "echo \"wine exe=$1\"",
        "shift",
        "echo \"args=$*\"",
        "echo \"prefix=$WINEPREFIX overrides=$WINEDLLOVERRIDES cwd=$(pwd)\"",
        "if [ -n \"$FAKE_SLEEP_PIDFILE\" ]; then sleep 30 & echo $! > \"$FAKE_SLEEP_PIDFILE\"; wait; fi",
        "echo \"MYSTICPARADOX_GAMESERVER_READY launchId=$MYSTICPARADOX_GAMESERVER_LAUNCH_ID port=$2\"",
        "exit 3",
        "",
    ].join("\n"));
    const exe = path.join(dir, "Dauntless", GAME_EXE_RELATIVE);
    write(exe, "not really a game");
    return { wine, xvfb, exe };
}

describe("Linux: host.json options", () => {
    it("reads --wine, --proton, --no-xvfb and --firewall-interface, and keeps the old ones otherwise", () => {
        const find = (n: string) => (n === "wine64" ? "/usr/bin/wine64" : null);
        assert.deepEqual(linuxFromArgs(parseArgs([]), undefined, find), { wine: null, proton: null, xvfb: true, firewallInterface: null });
        const set = linuxFromArgs(parseArgs(["--wine", "wine64", "--no-xvfb", "--firewall-interface", "ens3"]), undefined, find);
        assert.deepEqual(set, { wine: "/usr/bin/wine64", proton: null, xvfb: false, firewallInterface: "ens3" });
        assert.deepEqual(linuxFromArgs(parseArgs([]), set, find), set, "kept");
        assert.deepEqual(linuxFromArgs(parseArgs(["--xvfb", "--no-firewall-interface", "--proton", "/opt/GE-Proton10/proton"]), set, find), { wine: "/usr/bin/wine64", proton: "/opt/GE-Proton10/proton", xvfb: true, firewallInterface: null });
        assert.throws(() => linuxFromArgs(parseArgs(["--wine", "nowine"]), undefined, find), /not found/);
        assert.throws(() => linuxFromArgs(parseArgs(["--firewall-interface", "eth0; drop"]), undefined, find), /interface/);
    });

    it("are in host.json on Linux only, and checked", () => {
        const detect = { tailscale: () => null, mongod: () => "/usr/bin/mongod" };
        const args = parseArgs(["--mode", "public", "--host", "203.0.113.7", "--game", "/srv/dauntless/Dauntless"]);
        assert.deepEqual(hostConfigFromArgs(args, null, { ...detect, platform: "linux" }).linux, { wine: null, proton: null, xvfb: true, firewallInterface: null });
        assert.equal(hostConfigFromArgs(args, null, { ...detect, platform: "win32" }).linux, undefined);
        assert.equal(validateHostConfig(PUBLIC).linux?.firewallInterface, "eth0");
        assert.throws(() => validateHostConfig({ ...PUBLIC, linux: { ...PUBLIC.linux!, wine: "wine" } }), /linux.wine/);
        assert.throws(() => validateHostConfig({ ...PUBLIC, linux: { ...PUBLIC.linux!, firewallInterface: "a b" } }), /firewallInterface/);
    });
});

describe("Linux: env files", () => {
    const s = newSecrets();
    const p = paths("/home/reforged/Reforged");

    it("the Director starts the Wine launcher, which gets the game executable, the prefix and a longer readiness timeout", () => {
        const f = buildEnvFiles(PUBLIC, s, p, { platform: "linux" });
        assert.equal(get(f.director, "GAMESERVER_BINARY_PATH"), p.gameserverWrapper);
        assert.equal(get(f.director, "REFORGED_GAME_EXE"), path.join("/srv/dauntless/Dauntless", GAME_EXE_RELATIVE));
        assert.equal(get(f.director, "REFORGED_WINEPREFIX"), p.winePrefix);
        assert.equal(get(f.director, "REFORGED_XVFB"), "1");
        assert.equal(get(f.director, "REFORGED_WINE"), undefined, "found on PATH when not set");
        assert.equal(get(f.director, "GAMESERVER_READY_TIMEOUT_MS"), "90000");
        const allowlist = f.allowlist ?? [];
        assert.equal(get(allowlist, "ALLOWLIST_DRY_RUN"), "0");
        assert.equal(get(allowlist, "ALLOWLIST_FIREWALL"), "nftables");
        assert.equal(get(allowlist, "ALLOWLIST_NFT_TABLE"), "reforged_revival");
        assert.equal(get(allowlist, "ALLOWLIST_INTERFACE"), "eth0");
        const wine = buildEnvFiles({ ...PUBLIC, linux: { ...PUBLIC.linux!, wine: "/usr/bin/wine64", xvfb: false } }, s, p, { platform: "linux" });
        assert.equal(get(wine.director, "REFORGED_WINE"), "/usr/bin/wine64");
        assert.equal(get(wine.director, "REFORGED_XVFB"), "0");
    });

    it("Windows keeps starting the executable itself", () => {
        const f = buildEnvFiles(PUBLIC, s, p, { platform: "win32" });
        assert.equal(get(f.director, "GAMESERVER_BINARY_PATH"), path.join("/srv/dauntless/Dauntless", GAME_EXE_RELATIVE));
        assert.equal(get(f.director, "REFORGED_GAME_EXE"), undefined);
        assert.equal(get(f.director, "GAMESERVER_READY_TIMEOUT_MS"), "30000");
        assert.equal(get(f.allowlist ?? [], "ALLOWLIST_INTERFACE"), undefined);
    });
});

describe("Linux: privileges", () => {
    it("reads CapEff and decides on port 443 and nftables", () => {
        const none = "Name:\tnode\nCapEff:\t0000000000000000\n";
        const bind = "CapEff:\t0000000000000400\n";
        const admin = "CapEff:\t0000000000001000\n";
        assert.equal(effectiveCapabilities(bind), 0x400n);
        assert.equal(canBindPort(443, { root: false, status: none, unprivilegedStart: "1024" }), false);
        assert.equal(canBindPort(443, { root: false, status: bind, unprivilegedStart: "1024" }), true);
        assert.equal(canBindPort(443, { root: false, status: none, unprivilegedStart: "80" }), true);
        assert.equal(canBindPort(443, { root: true, status: none, unprivilegedStart: "1024" }), true);
        assert.equal(canBindPort(61000, { root: false, status: none, unprivilegedStart: "1024" }), true);
        assert.equal(canChangeFirewall({ root: false, status: bind }), false);
        assert.equal(canChangeFirewall({ root: false, status: admin }), true);
    });
});

describe("Linux: systemd units", () => {
    const p = paths("/home/reforged/Reforged");
    const ctx = { user: "reforged", node: "/usr/bin/node" };

    it("public: the allowlist with CAP_NET_ADMIN only, the server with CAP_NET_BIND_SERVICE only, both as the user", () => {
        const u = systemdUnits(PUBLIC, p, ctx);
        assert.deepEqual(Object.keys(u).sort(), ["install.sh", UNIT_ALLOWLIST, UNIT_SERVER].sort());
        const al = u[UNIT_ALLOWLIST];
        assert.match(al, /^User=reforged$/m);
        assert.match(al, /^AmbientCapabilities=CAP_NET_ADMIN$/m);
        assert.match(al, /^CapabilityBoundingSet=CAP_NET_ADMIN$/m);
        assert.match(al, /^ExecStart="\/usr\/bin\/node" "--env-file=\/home\/reforged\/Reforged\/\.workspace\/config\/allowlist\.env" "\/home\/reforged\/Reforged\/legacy\/ReforgedGateway\/dist\/allowlist\/server\.js"$/m);
        assert.match(al, /^ExecStopPost=.* --close-ports$/m);
        const sv = u[UNIT_SERVER];
        assert.match(sv, /^User=reforged$/m);
        assert.match(sv, /^Requires=reforged-allowlist\.service$/m);
        assert.match(sv, /^Environment=REFORGED_ALLOWLIST=external$/m);
        assert.match(sv, /^AmbientCapabilities=CAP_NET_BIND_SERVICE$/m);
        assert.match(sv, /^CapabilityBoundingSet=CAP_NET_BIND_SERVICE$/m);
        assert.match(sv, /^KillMode=mixed$/m);
        assert.match(sv, /^ExecStart="\/usr\/bin\/node" "\/home\/reforged\/Reforged\/legacy\/ReforgedHost\/dist\/cli\.js" start$/m);
        assert.match(u["install.sh"], /systemctl enable reforged-allowlist\.service reforged\.service/);
    });

    it("private: one unit and no privileges at all; never as root", () => {
        const u = systemdUnits(PRIVATE, p, ctx);
        assert.deepEqual(Object.keys(u).sort(), ["install.sh", UNIT_SERVER].sort());
        assert.match(u[UNIT_SERVER], /^CapabilityBoundingSet=$/m);
        assert.doesNotMatch(u[UNIT_SERVER], /AmbientCapabilities|REFORGED_ALLOWLIST|Requires=/);
        assert.match(u["install.sh"], /disable --now reforged-allowlist\.service/);
        assert.throws(() => systemdUnits(PUBLIC, p, { ...ctx, user: "root" }), /not as root/);
    });

    it("escapes paths for ExecStart and the shell", () => {
        assert.equal(unitWord("/opt/a b/100%/$HOME/\"q\""), "\"/opt/a b/100%%/$$HOME/\\\"q\\\"\"");
        assert.throws(() => unitWord("a\nb"));
        assert.equal(shQuote("/home/it's/node"), "'/home/it'\\''s/node'");
        assert.equal(gameserverWrapperScript("/usr/bin/node", "/home/r/Reforged/ReforgedHost").split("\n")[3], "exec '/usr/bin/node' '/home/r/Reforged/ReforgedHost/dist/gameserver.js' \"$@\"");
    });
});

describe("Linux: host firewall script", () => {
    it("opens 443 and the game ports in ufw for a public server, the tailnet only for a private one", () => {
        const pub = linuxFirewallScript(PUBLIC);
        assert.match(pub, /ufw allow 443\/tcp/);
        assert.match(pub, /ufw allow 8770:8777\/udp/);
        assert.match(pub, /Hostinger: the VPS's Firewall page/);
        const priv = linuxFirewallScript(PRIVATE);
        assert.match(priv, /ufw allow in on tailscale0 to any port 61000 proto tcp/);
        assert.doesNotMatch(priv, /ufw allow 443/);
    });
});

describe("Linux: the Wine launcher", () => {
    it("keeps other DLL overrides and puts the runtime loader's first", () => {
        assert.equal(dllOverrides(undefined), "winmm=n,b;mscoree=d;mshtml=d");
        assert.equal(dllOverrides("d3d11=n;WINMM=b;mscoree=n"), "d3d11=n;winmm=n,b;mscoree=d;mshtml=d");
    });

    it("runs the executable through wine on xvfb-run, from its own folder, with the prefix", { skip: !OnLinux && "Linux only" }, () => {
        const dir = tempDir("wine-launch");
        const t = fakeTools(dir);
        const env = { PATH: path.join(dir, "bin"), [WINE_ENV.exe]: t.exe, [WINE_ENV.prefix]: path.join(dir, "prefix") };
        const spec = buildLaunch(["key", "8777", "Hub"], env);
        assert.equal(spec.command, t.xvfb);
        assert.deepEqual(spec.args, ["-a", "-s", "-screen 0 1024x768x24", t.wine, t.exe, "key", "8777", "Hub"]);
        assert.equal(spec.cwd, path.dirname(t.exe));
        assert.equal(spec.env.WINEPREFIX, path.join(dir, "prefix"));
        assert.equal(spec.env.WINEDEBUG, "-all");
        assert.ok(fs.existsSync(path.join(dir, "prefix")));
        const direct = buildLaunch(["x"], { ...env, [WINE_ENV.xvfb]: "0" });
        assert.equal(direct.command, t.wine);
        assert.deepEqual(direct.args, [t.exe, "x"]);
        assert.equal(buildLaunch(["x"], { ...env, DISPLAY: ":0" }).command, t.wine, "a real display needs no xvfb-run");
        const proton = script(path.join(dir, "steam", "compatibilitytools.d", "GE-Proton10-1", "proton"), "#!/bin/sh\n");
        const viaProton = buildLaunch(["x"], { ...env, [WINE_ENV.xvfb]: "0", [WINE_ENV.proton]: proton, [WINE_ENV.protonData]: path.join(dir, "pfx") });
        assert.equal(viaProton.command, proton);
        assert.deepEqual(viaProton.args, ["run", t.exe, "x"]);
        assert.equal(viaProton.env.STEAM_COMPAT_DATA_PATH, path.join(dir, "pfx"));
        assert.equal(viaProton.env.STEAM_COMPAT_CLIENT_INSTALL_PATH, path.join(dir, "steam"));
        assert.throws(() => buildLaunch(["x"], { ...env, [WINE_ENV.exe]: path.join(dir, "missing.exe") }), /REFORGED_GAME_EXE/);
        assert.throws(() => buildLaunch(["x"], { ...env, PATH: "/nonexistent" }), /Wine was not found/);
        assert.throws(() => buildLaunch(["x"], { ...env, PATH: path.join(dir, "none"), [WINE_ENV.wine]: t.wine }), /xvfb-run was not found/);
    });

    it("as the Director runs it: arguments and environment reach the game, its ready line comes back, and its exit code", { skip: !OnLinux && "Linux only" }, async () => {
        const dir = tempDir("wine-run");
        const t = fakeTools(dir);
        const p = paths(dir);
        // The wrapper setup writes, pointed at the test build of gameserver.js.
        const wrapper = writeGameserverWrapper({ ...p, hostKit: path.join(__dirname, "..") }, process.execPath);
        fs.writeFileSync(wrapper, fs.readFileSync(wrapper, "utf8").replace(path.join("dist", "gameserver.js"), path.join("src", "gameserver.js")));
        const env = { ...process.env, PATH: `${path.join(dir, "bin")}:${process.env.PATH}`, [WINE_ENV.exe]: t.exe, [WINE_ENV.prefix]: path.join(dir, "prefix"), MYSTICPARADOX_GAMESERVER_LAUNCH_ID: "launch-42", DISPLAY: "" };
        const child = spawn(wrapper, ["key", "8777", "/Game/Maps/Hub", "-LogCmds=\"LogLoad Off\""], { env, stdio: ["ignore", "pipe", "pipe"] });
        let out = "";
        child.stdout.on("data", (c: Buffer) => (out += c.toString()));
        child.stderr.on("data", (c: Buffer) => (out += c.toString()));
        const code = await new Promise<number | null>((resolve) => child.once("exit", (c) => resolve(c)));
        assert.equal(code, 3, out);
        assert.match(out, /xvfb-run: .*\/bin\/wine/);
        assert.match(out, new RegExp(`wine exe=${t.exe.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`));
        assert.match(out, /args=key 8777 \/Game\/Maps\/Hub -LogCmds="LogLoad Off"/);
        assert.match(out, /overrides=winmm=n,b;mscoree=d;mshtml=d/);
        assert.match(out, new RegExp(`cwd=${path.dirname(t.exe).replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`));
        assert.match(out, /MYSTICPARADOX_GAMESERVER_READY launchId=launch-42 port=8777/);
    });

    it("a stop for the game server stops the whole runtime (wine, its children, the display)", { skip: !OnLinux && "Linux only" }, async () => {
        const dir = tempDir("wine-stop");
        const t = fakeTools(dir);
        const pidFile = path.join(dir, "sleep.pid");
        const env = { ...process.env, PATH: `${path.join(dir, "bin")}:${process.env.PATH}`, [WINE_ENV.exe]: t.exe, [WINE_ENV.prefix]: path.join(dir, "prefix"), FAKE_SLEEP_PIDFILE: pidFile, DISPLAY: "" };
        const child = spawn(process.execPath, [path.join(__dirname, "..", "src", "gameserver.js"), "key", "8770"], { env, stdio: "ignore" });
        const until = Date.now() + 10000;
        while (!fs.existsSync(pidFile) || fs.readFileSync(pidFile, "utf8").trim() === "") {
            if (Date.now() > until) throw new Error("the fake game never started");
            await new Promise((r) => setTimeout(r, 50));
        }
        const sleeper = Number(fs.readFileSync(pidFile, "utf8").trim());
        const exited = new Promise<void>((resolve) => child.once("exit", () => resolve()));
        child.kill("SIGTERM");
        await exited;
        const alive = () => {
            try {
                process.kill(sleeper, 0);
                return true;
            } catch {
                return false;
            }
        };
        const gone = Date.now() + 5000;
        while (alive() && Date.now() < gone) await new Promise((r) => setTimeout(r, 50));
        assert.equal(alive(), false, "the game's child process outlived the stop");
    });

    it("signals the process group, falling back to the process", () => {
        const sent: [number, string][] = [];
        assert.equal(signalGroup(1234, "SIGTERM", (pid, sig) => void sent.push([pid, sig])), true);
        assert.deepEqual(sent, [[-1234, "SIGTERM"]]);
        const fallback: number[] = [];
        assert.equal(signalGroup(55, "SIGTERM", (pid) => {
            if (pid < 0) throw new Error("no group");
            fallback.push(pid);
        }), true);
        assert.deepEqual(fallback, [55]);
        assert.equal(signalGroup(undefined, "SIGTERM"), false);
    });
});
