// Writes the fixtures the Rust host kit's tests compare against, with the Node kit's own code
// (legacy/ReforgedHost/src/{paths,envfile,hostconfig}.ts, imports given their .ts extension; the
// posix flavour runs them with path.posix).
import fs from "node:fs";
import pathModule from "node:path";
import { buildEnvFiles, DEFAULT_MONGO_URI, loadOrCreateSecrets, readHostConfig, writeEnvFiles, writeHostConfig } from "./hostconfig.ts";
import { paths } from "./paths.ts";

const [, , out, flavour] = process.argv;
const win = flavour === "windows";
const root = win ? "C:\\rr" : "/home/reforged/Reforged";
const gameDir = win ? "D:\\Games\\Dauntless" : "/srv/dauntless/Dauntless";
fs.rmSync(out, { recursive: true, force: true });
fs.mkdirSync(out, { recursive: true });
const at = (...p: string[]) => pathModule.join(out, ...p);
// The posix flavour's path.dirname cannot split this machine's paths: make the folders here.
for (const dir of ["hostjson", "secrets"]) fs.mkdirSync(at(dir), { recursive: true });

const PRIVATE: any = {
    version: 1, mode: "private", host: " 100.101.102.103 ", gameAddress: "100.101.102.103", name: "  Friday\u0007 Hunts ",
    gameDir, registration: "INVITECODE", shareUrl: "https://login.tailscale.com/admin/invite/abc_DEF-123",
    mongo: { uri: DEFAULT_MONGO_URI, manage: true, mongod: win ? "C:\\Program Files\\MongoDB\\Server\\8.0\\bin\\mongod.exe" : "/usr/bin/mongod" },
    maxHuntServers: 3, multipliers: undefined,
    performance: { serverFps: 30, idleFps: 10, trainingOnDemand: true, trainingIdleMinutes: 5, verboseGameLogs: false, memoryGuard: true, minFreeMemoryMb: 1024, huntMemoryMb: 1536 },
};
const PUBLIC: any = {
    version: 1, mode: "public", host: "Hunts.Example.NET", gameAddress: "203.0.113.7", name: "Freeza \"Hunts\" \u2014 P\u00fablico",
    gameDir, registration: "OPEN", shareUrl: null, mongo: { uri: "mongodb://10.0.0.5:27017/?replicaSet=rs0", manage: false, mongod: null },
    maxHuntServers: 8, multipliers: undefined, publicPort: 4443,
    performance: { serverFps: 60, idleFps: 0, trainingOnDemand: false, trainingIdleMinutes: 15, verboseGameLogs: true, memoryGuard: false, minFreeMemoryMb: 512, huntMemoryMb: 2048 },
    linux: { wine: "/usr/bin/wine64", proton: null, xvfb: false, firewallInterface: "eth0" },
};

// host.json as setup writes it.
writeHostConfig(at("hostjson", "private.json"), PRIVATE);
writeHostConfig(at("hostjson", "public.json"), PUBLIC);

// A hand-edited, older host.json: another key order, no performance fields of the memory guard, a
// key the kit does not know, numbers written as 3.0 and 1e1, another indent. Read and written back.
const older = [
    "{",
    '    "comment": "edited by hand",',
    '    "mode": "private",',
    '    "version": 1,',
    '    "name": "Old\\tServer",',
    '    "host": "HOSTPC.tail1234.TS.NET",',
    '    "gameAddress": "100.64.0.9",',
    '    "gameDir": null,',
    '    "registration": "NONE",',
    '    "shareUrl": null,',
    '    "mongo": { "manage": false, "uri": "mongodb+srv://cluster.example.net/x", "mongod": null },',
    '    "maxHuntServers": 3.0,',
    '    "performance": { "serverFps": 30, "idleFps": 1e1, "trainingOnDemand": true, "verboseGameLogs": false },',
    '    "extra": [1, 2.50, {"a": [], "2": "two", "1": "one"}, {}]',
    "}",
    "",
].join("\r\n");
fs.writeFileSync(at("hostjson", "older-input.json"), older);
const read = readHostConfig(at("hostjson", "older-input.json"))!;
writeHostConfig(at("hostjson", "older.json"), read);
// What `multipliers --xp 5 --rewards 2.5` writes, then `--off`.
writeHostConfig(at("hostjson", "multipliers.json"), { ...read, multipliers: { xp: 5, rewards: 2.5 } });
writeHostConfig(at("hostjson", "multipliers-off.json"), { ...readHostConfig(at("hostjson", "multipliers.json"))!, multipliers: undefined });

// secrets.env as setup makes it (fixed values instead of random ones).
const hex = (c: string) => c.repeat(64);
const fixed = {
    gatewaySecret: hex("a"), allowlistSecret: hex("b"), adminKey: hex("c"), gameserverKey: hex("d"), apiKeyHashSecret: hex("e"), metricsToken: hex("f"),
    jwtPrivateKeyB64: Buffer.from("-----BEGIN PRIVATE KEY-----\nMIIfake\n-----END PRIVATE KEY-----\n").toString("base64"),
    jwtPublicKeyB64: Buffer.from("-----BEGIN PUBLIC KEY-----\nMIIfake\n-----END PUBLIC KEY-----\n").toString("base64"),
};
const { secrets } = loadOrCreateSecrets(at("secrets"), () => fixed);

// The service env files, as setup writes them.
const p = paths(root);
const cases: [string, any, NodeJS.Platform][] = [
    ["private-win32", { ...readHostConfig(at("hostjson", "private.json"))!, multipliers: { xp: 5, rewards: 1.5 } }, "win32"],
    ["private-linux", readHostConfig(at("hostjson", "private.json"))!, "linux"],
    ["public-win32", readHostConfig(at("hostjson", "public.json"))!, "win32"],
    ["public-linux", readHostConfig(at("hostjson", "public.json"))!, "linux"],
    ["public-darwin", { ...readHostConfig(at("hostjson", "public.json"))!, gameDir: null, linux: undefined, performance: undefined, publicPort: undefined }, "darwin"],
];
for (const [name, config, platform] of cases) {
    fs.mkdirSync(at("env", name), { recursive: true });
    writeEnvFiles({ ...p, config: at("env", name) }, buildEnvFiles(config, secrets, p, { sourceCommit: "abc123def456", platform }));
}
console.log("fixtures written to", out);
