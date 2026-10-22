import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { describe, it } from "node:test";
import { hostConfigFromArgs, parseArgs, performanceFromArgs, performanceSummary } from "../src/cli";
import { huntsThatFit, performanceChecks, runtimeCpuPatches } from "../src/doctor";
import { buildEnvFiles, DEFAULT_MONGO_URI, DEFAULT_PERFORMANCE, gamePorts, newSecrets, QUIET_GAME_LOG_CMDS, validateHostConfig, type HostConfig } from "../src/hostconfig";
import { linuxFirewallScript } from "../src/linux";
import { MONGO_CACHE_GB, mongodArgs } from "../src/mongo";
import { paths } from "../src/paths";
import { tempDir, write } from "./helpers";

const PUBLIC: HostConfig = {
    version: 1,
    mode: "public",
    host: "203.0.113.7",
    gameAddress: "203.0.113.7",
    name: "Friday Hunts",
    gameDir: "/home/reforged/games/Dauntless",
    registration: "INVITECODE",
    shareUrl: null,
    mongo: { uri: DEFAULT_MONGO_URI, manage: true, mongod: "/usr/bin/mongod" },
    maxHuntServers: 3,
};

const detect = { tailscale: () => null, mongod: () => "/usr/bin/mongod", platform: "linux" as NodeJS.Platform };
const repo = path.resolve(__dirname, "..", "..", "..", "..");

function get(entries: [string, string][], key: string): string | undefined {
    return entries.find(([k]) => k === key)?.[1];
}

describe("performance: director.env", () => {
    const s = newSecrets();
    const p = paths("/home/reforged/Reforged");

    it("caps the game servers at 30 fps (10 when nobody is on them), starts the Training Grounds on demand and keeps the game logs quiet, on Linux and Windows", () => {
        for (const platform of ["linux", "win32"] as const) {
            const f = buildEnvFiles(PUBLIC, s, p, { platform });
            assert.equal(get(f.director, "REFORGED_SERVER_MAX_FPS"), "30");
            assert.equal(get(f.director, "REFORGED_SERVER_IDLE_FPS"), "10");
            assert.equal(get(f.director, "TRAINING_ON_DEMAND"), "1");
            assert.equal(get(f.director, "GAMESERVER_LOG_CMDS"), QUIET_GAME_LOG_CMDS);
            assert.equal(get(f.director, "GAMESERVER_MEMORY_GUARD"), "1");
            assert.equal(get(f.director, "TRAINING_IDLE_STOP_MINUTES"), "5", "on demand, and stopped again after 5 min empty");
            assert.equal(get(f.director, "GAMESERVER_RUNTIME_LOG_DIR"), path.dirname(path.join(PUBLIC.gameDir!, "Archon", "Binaries", "Win64", "Dauntless-Win64-Shipping.exe")));
            assert.equal(get(f.director, "GAMESERVER_MIN_FREE_MB"), "1024");
            assert.equal(get(f.director, "GAMESERVER_HUNT_MEMORY_MB"), "1536");
        }
    });

    it("keeps UDP 8770-8777 up to 6 hunts and reaches one port further down per hunt above that", () => {
        assert.deepEqual(gamePorts({ maxHuntServers: 3 }), { first: 8770, last: 8777 });
        assert.deepEqual(gamePorts({ maxHuntServers: 6 }), { first: 8770, last: 8777 });
        assert.deepEqual(gamePorts({ maxHuntServers: 8 }), { first: 8768, last: 8777 });
        assert.deepEqual(gamePorts({ maxHuntServers: 10 }), { first: 8766, last: 8777 });
        const eight = { ...PUBLIC, maxHuntServers: 8 };
        const f = buildEnvFiles(eight, s, p, { platform: "linux" });
        assert.equal(get(f.director, "PORT_RANGE_BEGIN"), "8768", "Ramsgate 8777, Training 8776, eight hunts 8768-8775");
        assert.equal(get(f.director, "PORT_RANGE_END"), "8777");
        assert.equal(get(f.director, "MAX_CONCURRENT_HUNT_SERVERS"), "8");
        assert.equal(get(f.allowlist ?? [], "ALLOWLIST_PORTS"), "8768-8777");
        assert.match(linuxFirewallScript(eight), /ufw allow 8768:8777\/udp/);
        assert.match(linuxFirewallScript(eight), /UDP 8768-8777 in your provider's firewall/);
        assert.equal(get(buildEnvFiles(PUBLIC, s, p, { platform: "linux" }).allowlist ?? [], "ALLOWLIST_PORTS"), "8770-8777");
    });

    it("follows host.json: no cap, Training Grounds at boot, upstream's verbose logs", () => {
        const f = buildEnvFiles({ ...PUBLIC, performance: { ...DEFAULT_PERFORMANCE, serverFps: 0, idleFps: 0, trainingOnDemand: false, verboseGameLogs: true, memoryGuard: false } }, s, p, { platform: "linux" });
        assert.equal(get(f.director, "REFORGED_SERVER_MAX_FPS"), "0");
        assert.equal(get(f.director, "REFORGED_SERVER_IDLE_FPS"), "0");
        assert.equal(get(f.director, "TRAINING_ON_DEMAND"), "0");
        assert.equal(get(f.director, "GAMESERVER_LOG_CMDS"), undefined, "the Director keeps its own default");
        assert.equal(get(f.director, "GAMESERVER_MEMORY_GUARD"), "0");
        assert.equal(get(f.director, "TRAINING_IDLE_STOP_MINUTES"), "0", "at boot: never stopped");
        assert.equal(get(buildEnvFiles({ ...PUBLIC, gameDir: null }, s, p, { platform: "linux" }).director, "GAMESERVER_RUNTIME_LOG_DIR"), undefined, "no game folder yet");
    });

    it("writes only values the runtime and the Director accept", () => {
        // The Director's check for GAMESERVER_LOG_CMDS (patch 0021).
        assert.match(QUIET_GAME_LOG_CMDS, /^[A-Za-z0-9_, ]{1,512}$/);
        const runtime = fs.readFileSync(path.join(repo, "upstream", "patches", "0020-runtime-server-frame-cap.patch"), "utf8");
        assert.ok(runtime.includes('"REFORGED_SERVER_MAX_FPS", 30, 10, 240'));
        assert.ok(runtime.includes('"REFORGED_SERVER_IDLE_FPS", 0, 1, 240'));
        const director = fs.readFileSync(path.join(repo, "upstream", "patches", "0021-director-training-on-demand-and-log-levels.patch"), "utf8");
        assert.ok(director.includes("process.env.TRAINING_ON_DEMAND"));
        assert.ok(director.includes("process.env.GAMESERVER_LOG_CMDS"));
        const idle = fs.readFileSync(path.join(repo, "upstream", "patches", "0033-director-training-stops-when-idle.patch"), "utf8");
        for (const name of ["process.env.TRAINING_IDLE_STOP_MINUTES", "process.env.GAMESERVER_RUNTIME_LOG_DIR", "mysticparadox_dll_port${TRAINING_DOJO_PORT}.log"]) assert.ok(idle.includes(name), name);
        const memory = fs.readFileSync(path.join(repo, "upstream", "patches", "0022-director-hunt-memory-guard.patch"), "utf8");
        for (const name of ["env.GAMESERVER_MEMORY_GUARD", '"GAMESERVER_MIN_FREE_MB"', '"GAMESERVER_HUNT_MEMORY_MB"']) assert.ok(memory.includes(name), name);
    });

    it("refuses frame rates and switches the runtime would not take", () => {
        const bad: [Partial<NonNullable<HostConfig["performance"]>>, RegExp][] = [
            [{ serverFps: 5 }, /serverFps/],
            [{ serverFps: 241 }, /serverFps/],
            [{ serverFps: 29.5 }, /serverFps/],
            [{ idleFps: -1 }, /idleFps/],
            [{ idleFps: 300 }, /idleFps/],
        ];
        for (const [perf, error] of bad) {
            assert.throws(() => validateHostConfig({ ...PUBLIC, performance: { ...DEFAULT_PERFORMANCE, ...perf } }), error);
        }
        assert.throws(() => validateHostConfig({ ...PUBLIC, performance: { ...DEFAULT_PERFORMANCE, trainingOnDemand: "yes" as unknown as boolean } }), /trainingOnDemand/);
        assert.throws(() => validateHostConfig({ ...PUBLIC, performance: { ...DEFAULT_PERFORMANCE, huntMemoryMb: 100 } }), /huntMemoryMb/);
        assert.throws(() => validateHostConfig({ ...PUBLIC, performance: { ...DEFAULT_PERFORMANCE, minFreeMemoryMb: -1 } }), /minFreeMemoryMb/);
        assert.throws(() => validateHostConfig({ ...PUBLIC, performance: { ...DEFAULT_PERFORMANCE, trainingIdleMinutes: 1441 } }), /trainingIdleMinutes/);
        // A host.json written before the memory guard existed: the defaults fill the new fields.
        const older = { ...PUBLIC, performance: { serverFps: 30, idleFps: 10, trainingOnDemand: true, verboseGameLogs: false } as unknown as HostConfig["performance"] };
        assert.equal(get(buildEnvFiles(validateHostConfig(older), s, p, { platform: "linux" }).director, "GAMESERVER_MEMORY_GUARD"), "1");
        assert.equal(validateHostConfig({ ...PUBLIC, performance: { ...DEFAULT_PERFORMANCE, idleFps: 1 } }).performance?.idleFps, 1);
    });
});

describe("performance: setup options", () => {
    it("defaults, keeps what host.json had, and takes each option", () => {
        assert.deepEqual(performanceFromArgs(parseArgs([]), undefined), DEFAULT_PERFORMANCE);
        const set = performanceFromArgs(parseArgs(["--server-fps", "60", "--idle-fps", "0", "--training-at-boot", "--verbose-game-logs", "--no-memory-guard", "--min-free-ram", "512", "--hunt-ram", "2048", "--training-idle-minutes", "15"]), undefined);
        assert.deepEqual(set, { serverFps: 60, idleFps: 0, trainingOnDemand: false, trainingIdleMinutes: 15, verboseGameLogs: true, memoryGuard: false, minFreeMemoryMb: 512, huntMemoryMb: 2048 });
        assert.deepEqual(performanceFromArgs(parseArgs([]), set), set, "kept on the next setup");
        assert.deepEqual(performanceFromArgs(parseArgs(["--training-on-demand", "--quiet-game-logs"]), set), { ...set, trainingOnDemand: true, verboseGameLogs: false });
        assert.throws(() => performanceFromArgs(parseArgs(["--server-fps", "5"]), undefined), /--server-fps/);
        assert.throws(() => performanceFromArgs(parseArgs(["--idle-fps", "fast"]), undefined), /--idle-fps/);
        assert.throws(() => performanceFromArgs(parseArgs(["--hunt-ram", "100"]), undefined), /--hunt-ram/);
        assert.throws(() => performanceFromArgs(parseArgs(["--training-idle-minutes", "soon"]), undefined), /--training-idle-minutes/);
        assert.equal(performanceFromArgs(parseArgs(["--training-idle-minutes", "0"]), undefined).trainingIdleMinutes, 0);
        assert.equal(performanceFromArgs(parseArgs(["--memory-guard"]), set).memoryGuard, true);
    });

    it("goes into host.json with setup, and an older host.json without it gets the defaults", () => {
        const c = hostConfigFromArgs(parseArgs(["--mode", "public", "--host", "203.0.113.7", "--server-fps", "45"]), null, detect);
        assert.equal(c.performance?.serverFps, 45);
        assert.equal(validateHostConfig(c).performance?.serverFps, 45);
        const again = hostConfigFromArgs(parseArgs([]), c, detect);
        assert.equal(again.performance?.serverFps, 45);
        assert.deepEqual(hostConfigFromArgs(parseArgs([]), PUBLIC, detect).performance, DEFAULT_PERFORMANCE);
        assert.match(performanceSummary(PUBLIC), /30 fps, 10 fps after 15 s without players; Training Grounds on demand, stopped after 5 min without players; quiet game logs; at most 3 hunts at once \(UDP 8770-8777\), a new hunt only with 2560 MB free/);
        assert.equal(hostConfigFromArgs(parseArgs(["--max-hunts", "8"]), PUBLIC, detect).maxHuntServers, 8);
    });
});

describe("performance: doctor", () => {
    it("tells whether the published runtime has the game servers' CPU patches", () => {
        const dir = tempDir("runtime-cpu");
        const both = path.join(dir, "both.dll");
        write(both, Buffer.concat([Buffer.from("MZ junk "), Buffer.from("LEGACY_EXPECTED_PLAYER_WALK.flag", "utf16le"), Buffer.from(" REFORGED_SERVER_MAX_FPS\0")]));
        const old = path.join(dir, "old.dll");
        write(old, "MZ an older runtime");
        assert.deepEqual(runtimeCpuPatches(both), { cpuFixes: true, frameCap: true });
        assert.deepEqual(runtimeCpuPatches(old), { cpuFixes: false, frameCap: false });
        assert.equal(runtimeCpuPatches(path.join(dir, "none.dll")), null);
        const warn = performanceChecks(PUBLIC, old, 16384);
        assert.equal(warn[0].level, "info");
        assert.equal(warn[1].level, "warn");
        assert.match(warn[1].text, /0019 \(CPU fixes\) and 0020 \(frame cap\)/);
        assert.equal(performanceChecks(PUBLIC, both, 16384)[1].level, "ok");
        assert.equal(performanceChecks(PUBLIC, null, 16384).length, 1, "nothing published: only the settings");
    });

    it("says how many hunts fit in this machine's memory", () => {
        assert.equal(huntsThatFit(16 * 1024, DEFAULT_PERFORMANCE), 7, "16 GB: 7 hunts of 1.5 GB next to the hubs, the services and 1 GB free");
        assert.equal(huntsThatFit(15 * 1024, DEFAULT_PERFORMANCE), 6, "a \"16 GB\" VPS often reports less");
        const eight = { ...PUBLIC, maxHuntServers: 8 };
        const guarded = performanceChecks(eight, null, 16 * 1024);
        assert.equal(guarded[1].level, "info");
        assert.match(guarded[1].text, /8 hunts at once may need more than this machine's 16.0 GB: about 7 fit; the memory guard refuses/);
        const unguarded = performanceChecks({ ...eight, performance: { ...DEFAULT_PERFORMANCE, memoryGuard: false } }, null, 16 * 1024);
        assert.equal(unguarded[1].level, "warn");
        assert.equal(performanceChecks(eight, null, 32 * 1024).length, 1, "32 GB: they fit, nothing to say");
    });
});

describe("performance: MongoDB", () => {
    it("caps WiredTiger's cache instead of letting it take half of the RAM", () => {
        const args = mongodArgs("/home/reforged/Reforged/.workspace/mongodb", "/home/reforged/Reforged/.workspace/logs/mongod.log", 27017);
        const at = args.indexOf("--wiredTigerCacheSizeGB");
        assert.ok(at > 0);
        assert.equal(args[at + 1], String(MONGO_CACHE_GB));
    });
});
