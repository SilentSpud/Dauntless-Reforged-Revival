// Writes rh3.json, which the Rust host kit's setup and invite tests compare against: the Node kit's
// own cli.ts (hostConfigFromArgs, firewallScript, performanceSummary), invite.ts (inviteFor) and
// hostconfig.ts (writeHostConfig) on the same inputs.
//
// Run with run.sh next to this file (Node 22.18 or newer: it copies legacy/ReforgedHost/src with
// .ts import extensions and runs this with Node's type stripping, no npm install needed).
import fs from "node:fs";
import os from "node:os";
import pathModule from "node:path";
import { firewallScript, hostConfigFromArgs, parseArgs, performanceSummary } from "./cli.ts";
import { formatInvite, inviteFor } from "./invite.ts";
import { readHostConfig, writeHostConfig } from "./hostconfig.ts";

const [, , out] = process.argv;
const tmp = fs.mkdtempSync(pathModule.join(os.tmpdir(), "rh3-gen-"));
let n = 0;
// host.json as setup writes it, read back as setup does.
function written(config: any): { text: string; config: any } {
    const file = pathModule.join(tmp, `host-${n++}.json`);
    writeHostConfig(file, config);
    return { text: fs.readFileSync(file, "utf8"), config: readHostConfig(file) };
}
const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

const MONGOD = "C:\\Program Files\\MongoDB\\Server\\8.0\\bin\\mongod.exe";
type Detect = { tailscale: string | null; mongod: string | null; platform: "win32" | "linux" };
const TS: Detect = { tailscale: "100.101.102.103", mongod: MONGOD, platform: "win32" };

// [name, args, the host.json before (a name from this list), what this PC has]
const setups: [string, string[], string | null, Detect][] = [
    ["private", ["--mode", "private", "--name", "Friday"], null, TS],
    ["private-share", ["--mode", "private", "--name", "Friday Hunts & Co", "--share", "https://login.tailscale.com/admin/invite/abcDEF123", "--registration", "open", "--max-hunts", "6"], null, TS],
    ["public-from-private", ["--mode", "public", "--host", "203.0.113.7"], "private", TS],
    ["public-dns", ["--mode", "public", "--host", "Hunts.Example.NET", "--game-address", "203.0.113.7", "--public-port", "4443", "--name", "Freeza \"Hunts\" \u2014 P\u00fablico"], null, TS],
    ["public-dns-no-address", ["--mode", "public", "--host", "hunts.example.net"], null, TS],
    ["public-port-back", ["--public-port", "443"], "public-dns", TS],
    ["public-port-kept", ["--name", "Moved"], "public-dns", TS],
    ["public-port-bad", ["--mode", "public", "--host", "203.0.113.7", "--public-port", "web"], null, TS],
    ["public-port-reserved", ["--mode", "public", "--host", "203.0.113.7", "--public-port", "61000"], null, TS],
    ["private-public-port", ["--mode", "private", "--public-port", "4443"], null, TS],
    ["no-mode", [], null, TS],
    ["no-tailscale", ["--mode", "private"], null, { ...TS, tailscale: null }],
    ["tailscale-moved", [], "private", { ...TS, tailscale: "100.88.12.40" }],
    ["tailscale-offline", [], "private", { ...TS, tailscale: null }],
    ["host-wins", ["--host", "100.64.0.9"], "private", TS],
    ["own-mongo", ["--mode", "private", "--mongo-uri", "mongodb://db.local:27017/?replicaSet=rs0"], null, TS],
    ["no-mongod", ["--mode", "private", "--no-manage-mongo"], null, { ...TS, mongod: null }],
    ["mongod-missing", ["--mode", "private"], null, { ...TS, mongod: null }],
    ["multipliers", ["--mode", "private", "--xp-multiplier", "5", "--reward-multiplier", "x3"], null, TS],
    ["multipliers-kept", [], "multipliers", TS],
    ["multipliers-off", ["--off"], "multipliers", TS],
    ["multipliers-ones", ["--xp-multiplier", "1", "--reward-multiplier", "1"], "multipliers", TS],
    ["multipliers-not-public", ["--mode", "public", "--host", "203.0.113.7"], "multipliers", TS],
    ["multipliers-public", ["--mode", "public", "--host", "203.0.113.7", "--xp-multiplier", "5"], null, TS],
    ["multipliers-too-big", ["--mode", "private", "--xp-multiplier", "500"], null, TS],
    ["multipliers-fraction", ["--mode", "private", "--xp-multiplier", "2.5"], null, TS],
    ["game-relative", ["--mode", "private", "--game", "Games\\Dauntless"], null, TS],
    ["game-npm-escaped", ["--mode", "private", "--game", "^D:\\Games\\Dauntless^ 1.14.7\\Dauntless^"], null, TS],
    ["performance", ["--mode", "public", "--host", "203.0.113.7", "--server-fps", "60", "--idle-fps", "0", "--training-at-boot", "--verbose-game-logs", "--no-memory-guard", "--min-free-ram", "512 MB", "--hunt-ram", "2048", "--training-idle-minutes", "15"], null, TS],
    ["performance-kept", ["--training-on-demand", "--quiet-game-logs", "--max-hunts", "8"], "performance", TS],
    ["server-fps-low", ["--mode", "private", "--server-fps", "5"], null, TS],
    ["idle-fps-word", ["--mode", "private", "--idle-fps", "fast"], null, TS],
    ["hunt-ram-low", ["--mode", "private", "--hunt-ram", "100"], null, TS],
    ["training-soon", ["--mode", "private", "--training-idle-minutes", "soon"], null, TS],
    ["max-hunts-bad", ["--mode", "private", "--max-hunts", "11"], null, TS],
    ["registration-bad", ["--mode", "private", "--registration", "maybe"], null, TS],
    ["linux", ["--mode", "public", "--host", "203.0.113.7", "--wine", "/usr/bin/wine64", "--no-xvfb", "--firewall-interface", "ens3"], null, { ...TS, platform: "linux" }],
    ["linux-kept", ["--xvfb", "--no-firewall-interface", "--proton", "/opt/GE-Proton10/proton"], "linux", { ...TS, platform: "linux" }],
    ["linux-iface-bad", ["--mode", "public", "--host", "203.0.113.7", "--firewall-interface", "eth0; drop"], null, { ...TS, platform: "linux" }],
    ["linux-on-windows", ["--name", "Back on Windows"], "linux", TS],
];

const configs: Record<string, any> = {};
const setupOut = setups.map(([name, args, before, detect]) => {
    try {
        const c = hostConfigFromArgs(parseArgs(args), before ? configs[before] : null, { tailscale: () => detect.tailscale, mongod: () => detect.mongod, platform: detect.platform });
        const w = written(c);
        configs[name] = w.config;
        return { name, args, before, detect, hostJson: w.text, summary: performanceSummary(w.config) };
    } catch (e) {
        return { name, args, before, detect, error: message(e) };
    }
});

const firewall = ["private", "public-dns", "performance-kept"].map((name) => ({ name, script: firewallScript(configs[name], "C:\\node\\node.exe") }));

const FP = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const invites = [
    ["private", "ABCD-EFGH-JKMN", null],
    ["private-share", "WXYZ-2345-6789", null],
    ["public-from-private", "ABCD-EFGH-JKMN", FP],
    ["public-dns", "a b/c?d&e=f+g%h", FP],
    ["performance-kept", "\u00e9\u00e8-\u2713", FP],
].map(([name, code, fp]) => ({ name, code, fp, link: inviteFor(configs[name as string], code as string, fp) }));
let noFp = "";
try {
    formatInvite({ mode: "public", host: "x.example", port: 443, code: "ABCD", name: "x", fp: null, share: null });
} catch (e) {
    noFp = message(e);
}

fs.writeFileSync(out, JSON.stringify({ setups: setupOut, firewall, invites, noFingerprint: noFp }, null, 2) + "\n");
fs.rmSync(tmp, { recursive: true, force: true });
console.log("written", out);
