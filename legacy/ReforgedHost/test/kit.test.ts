import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { describe, it } from "node:test";
import { firewallScript, hostConfigFromArgs, parseArgs, watchStopInput } from "../src/cli";
import { PassThrough } from "node:stream";
import { BACKEND_GAME_DATA, gameDataState, gameDataSteps, generateGameData, SUPPLIED_SOURCES, useExamples } from "../src/gamedata";
import { DEFAULT_MONGO_URI, type HostConfig } from "../src/hostconfig";
import { createInviteCode, formatInvite, inviteFor } from "../src/invite";
import { directUri, mongodArgs } from "../src/mongo";
import { paths } from "../src/paths";
import { ensureServerFlags, installServerRuntime, publishRuntime, SERVER_RUNTIME_FLAGS, sha256File } from "../src/publish";
import { serviceEnvironment, Supervisor } from "../src/services";
import { parseInvite } from "../../ReforgedLauncher/src/shared/invite";
import { ParseRuntimeIndex } from "../../ReforgedContent/src/runtime";
import { tempDir, write } from "./helpers";

const detect = { tailscale: () => "100.101.102.103", mongod: () => "C:\\Program Files\\MongoDB\\Server\\8.0\\bin\\mongod.exe" };

describe("command line", () => {
    it("parses --key value, --key=value and flags", () => {
        assert.deepEqual(parseArgs(["setup", "--mode", "private", "--name=Friday Hunts", "--skip-install", "--uses", "3"]), { _: ["setup"], mode: "private", name: "Friday Hunts", "skip-install": true, uses: "3" });
    });

    it("builds host.json from the arguments, the old host.json and what it finds on this PC", () => {
        const c = hostConfigFromArgs(parseArgs(["--mode", "private", "--name", "Friday"]), null, detect);
        assert.equal(c.host, "100.101.102.103", "private: this PC's Tailscale address");
        assert.equal(c.gameAddress, "100.101.102.103");
        assert.equal(c.mongo.uri, DEFAULT_MONGO_URI);
        assert.equal(c.mongo.manage, true);
        assert.equal(c.mongo.mongod, detect.mongod());
        const pub = hostConfigFromArgs(parseArgs(["--mode", "public", "--host", "203.0.113.7"]), c, detect);
        assert.equal(pub.gameAddress, "203.0.113.7");
        assert.equal(pub.name, "Friday", "kept from the old host.json");
        assert.throws(() => hostConfigFromArgs(parseArgs(["--mode", "public", "--host", "hunts.example.net"]), null, detect), /game-address/);
        assert.equal(hostConfigFromArgs(parseArgs(["--mode", "public", "--host", "hunts.example.net", "--game-address", "203.0.113.7"]), null, detect).gameAddress, "203.0.113.7");
        assert.throws(() => hostConfigFromArgs(parseArgs([]), null, detect), /--mode/);
        assert.throws(() => hostConfigFromArgs(parseArgs(["--mode", "private"]), null, { ...detect, tailscale: () => null }), /Tailscale/);
        assert.equal(hostConfigFromArgs(parseArgs(["--mode", "private", "--mongo-uri", "mongodb://db.local:27017/?replicaSet=rs0"]), null, detect).mongo.manage, false);
    });

    it("takes test multipliers on a private server only, keeps them on the next setup, and --off clears them", () => {
        const c = hostConfigFromArgs(parseArgs(["--mode", "private", "--xp-multiplier", "5", "--reward-multiplier", "x3"]), null, detect);
        assert.deepEqual(c.multipliers, { xp: 5, rewards: 3 });
        assert.deepEqual(hostConfigFromArgs(parseArgs([]), c, detect).multipliers, { xp: 5, rewards: 3 });
        assert.equal(hostConfigFromArgs(parseArgs(["--off"]), c, detect).multipliers, undefined);
        assert.equal(hostConfigFromArgs(parseArgs(["--xp-multiplier", "1", "--reward-multiplier", "1"]), c, detect).multipliers, undefined);
        assert.equal(hostConfigFromArgs(parseArgs(["--mode", "public", "--host", "203.0.113.7"]), c, detect).multipliers, undefined, "not carried into a public server");
        assert.throws(() => hostConfigFromArgs(parseArgs(["--mode", "public", "--host", "203.0.113.7", "--xp-multiplier", "5"]), null, detect), /private/);
        assert.throws(() => hostConfigFromArgs(parseArgs(["--mode", "private", "--xp-multiplier", "500"]), null, detect), /1 to 100/);
    });

    it("follows a new Tailscale address on setup, and keeps the old one only when Tailscale is not running", () => {
        const old = hostConfigFromArgs(parseArgs(["--mode", "private", "--name", "Freeza"]), null, { ...detect, tailscale: () => "100.117.171.17" });
        const moved = hostConfigFromArgs(parseArgs([]), old, { ...detect, tailscale: () => "100.88.12.40" });
        assert.equal(moved.host, "100.88.12.40", "another Tailscale account, another address");
        assert.equal(moved.gameAddress, "100.88.12.40", "the game servers move with it");
        assert.equal(moved.name, "Freeza");
        const offline = hostConfigFromArgs(parseArgs([]), old, { ...detect, tailscale: () => null });
        assert.equal(offline.host, "100.117.171.17");
        assert.equal(hostConfigFromArgs(parseArgs(["--host", "100.64.0.9"]), old, detect).host, "100.64.0.9", "--host still wins");
    });

    it("wants a full path for --game", () => {
        assert.equal(hostConfigFromArgs(parseArgs(["--mode", "private", "--game", "/games/Dauntless 1.14.7/Dauntless"]), null, detect).gameDir, "/games/Dauntless 1.14.7/Dauntless");
        assert.ok(hostConfigFromArgs(parseArgs(["--mode", "private", "--game", "D:\\Games\\Dauntless 1.14.7\\Dauntless"]), null, detect).gameDir);
        assert.throws(() => hostConfigFromArgs(parseArgs(["--mode", "private", "--game", "Games\\Dauntless"]), null, detect), /full path/);
        // What npm 10 on Windows used to hand over for "D:\Games\Dauntless 1.14.7\Dauntless".
        assert.throws(() => hostConfigFromArgs(parseArgs(["--mode", "private", "--game", "^D:\\Games\\Dauntless^ 1.14.7\\Dauntless^"]), null, detect), /full path/);
    });

    // npm on Windows escapes a script's arguments for cmd.exe once, or twice when the script's first
    // command is a .cmd (npm itself is npm.cmd). Arguments meant for node.exe after
    // "npm run build && node ..." kept one level of ^, so the command scripts start with node and
    // build in their pre-script.
    it("package.json: every command script starts with node and builds in its pre-script", () => {
        const pkg = JSON.parse(fs.readFileSync(path.resolve(__dirname, "..", "..", "..", "package.json"), "utf8")) as { scripts: Record<string, string> };
        const commands = Object.entries(pkg.scripts).filter(([, s]) => s.includes("dist/cli.js"));
        assert.ok(commands.length >= 7);
        for (const [name, script] of commands) {
            assert.match(script, /^node dist\/cli\.js\b/, `${name} starts with node`);
            assert.equal(pkg.scripts[`pre${name}`], "npm run build --silent", `pre${name} builds`);
        }
    });

    it("writes firewall rules: Tailscale-only in private mode, 443 and a closed allowlist rule in public mode", () => {
        const base = hostConfigFromArgs(parseArgs(["--mode", "private", "--game", "D:\\Games\\Dauntless"]), null, detect);
        const priv = firewallScript(base, "C:\\node\\node.exe");
        assert.match(priv, /100\.64\.0\.0\/10/);
        assert.match(priv, /LocalPort|'61000'/);
        assert.doesNotMatch(priv, /'443'/);
        const pub = firewallScript({ ...base, mode: "public", host: "203.0.113.7", gameAddress: "203.0.113.7" }, "C:\\node\\node.exe");
        assert.match(pub, /'443'/);
        assert.match(pub, /ReforgedRevival-GamePorts-Allowlist/);
        assert.match(pub, /'False'/);
    });
});

describe("the launcher's control of a running server", () => {
    it("stops on a stop line or when the launcher's pipe closes, once", async () => {
        const viaLine = new PassThrough();
        let stops = 0;
        watchStopInput(viaLine, () => stops++);
        viaLine.write("hello\nSTO");
        assert.equal(stops, 0, "only a whole stop line");
        viaLine.write("P\r\n");
        viaLine.end();
        await new Promise((r) => setImmediate(r));
        assert.equal(stops, 1, "stopped once, not again when the pipe closed");

        const viaClose = new PassThrough();
        let closed = 0;
        watchStopInput(viaClose, () => closed++);
        viaClose.end();
        viaClose.resume();
        await new Promise((r) => setImmediate(r));
        assert.equal(closed, 1, "the launcher quit: the pipe closed");
    });
});

describe("invites", () => {
    const config: HostConfig = { version: 1, mode: "private", host: "100.101.102.103", gameAddress: "100.101.102.103", name: "Friday Hunts & Co", gameDir: null, registration: "INVITECODE", shareUrl: "https://login.tailscale.com/admin/invite/abcDEF123", mongo: { uri: DEFAULT_MONGO_URI, manage: true, mongod: null }, maxHuntServers: 3 };

    it("are links the launcher accepts, private and public", () => {
        const priv = parseInvite(inviteFor(config, "ABCD-EFGH-JKMN", null));
        assert.ok(priv.ok);
        if (priv.ok) assert.deepEqual(priv.invite, { game: "reforged", mode: "private", host: "100.101.102.103", port: 61000, code: "ABCD-EFGH-JKMN", name: "Friday Hunts & Co", share: config.shareUrl, fp: null });
        const fp = "ab".repeat(32);
        const pub = parseInvite(inviteFor({ ...config, mode: "public", host: "hunts.example.net", gameAddress: "203.0.113.7", shareUrl: null }, "ABCD-EFGH-JKMN", fp));
        assert.ok(pub.ok);
        if (pub.ok) assert.deepEqual(pub.invite, { game: "reforged", mode: "public", host: "hunts.example.net", port: 443, code: "ABCD-EFGH-JKMN", name: "Friday Hunts & Co", share: null, fp });
        assert.throws(() => formatInvite({ mode: "public", host: "x.example", port: 443, code: "ABCD", name: "x", fp: null, share: null }), /fingerprint/);
    });

    it("get their code from the backend's local admin route with the admin key", async () => {
        let seen: { key: unknown; body: string } | null = null;
        const server = http.createServer((req, res) => {
            let body = "";
            req.on("data", (c) => (body += c));
            req.on("end", () => {
                seen = { key: req.headers["x-reforged-admin-key"], body };
                if (req.headers["x-reforged-admin-key"] !== "k".repeat(64)) return void res.writeHead(403).end();
                res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ code: "WXYZ-2345-6789" }));
            });
        });
        await new Promise<void>((r) => server.listen(62060, "127.0.0.1", r));
        try {
            assert.equal(await createInviteCode("k".repeat(64), { uses: 2, note: "Alex" }, 62060), "WXYZ-2345-6789");
            assert.deepEqual(JSON.parse((seen as unknown as { body: string }).body), { uses: 2, infinite: false, name: "Alex" });
            await assert.rejects(createInviteCode("wrong".repeat(10), { uses: 1 }, 62060), /admin key/);
            await assert.rejects(createInviteCode("k".repeat(64), { uses: 1 }, 62069), /not running/);
        } finally {
            server.close();
        }
    });
});

describe("runtime publishing", () => {
    it("writes the files and a runtime.json the content server accepts, then installs them for the game servers", () => {
        const p = paths(tempDir("publish"));
        write(p.runtimeDll, Buffer.alloc(6000, 3));
        write(p.loaderDll, Buffer.alloc(2000, 4));
        const r = publishRuntime(p, new Date("2026-10-15T00:00:00Z"));
        assert.match(r.version, /^20261015-[0-9a-f]{8}$/);
        const index = ParseRuntimeIndex(JSON.parse(fs.readFileSync(path.join(p.runtimePublish, "runtime.json"), "utf8")));
        assert.equal(index.files.find((f) => f.name === "winmm.dll")?.sha256, sha256File(p.loaderDll));
        const win64 = tempDir("win64");
        assert.deepEqual(installServerRuntime(p, win64).installed, ["MysticParadox.dll", "winmm.dll"]);
        assert.deepEqual(installServerRuntime(p, win64).unchanged, ["MysticParadox.dll", "winmm.dll"]);
        assert.deepEqual(ensureServerFlags(win64), ["CALL_HUNTPASS_QUERY.flag"], "the hub's progression query is switched on");
        assert.deepEqual(ensureServerFlags(win64), [], "and left alone once it is there");
        assert.ok(SERVER_RUNTIME_FLAGS.every((f) => fs.existsSync(path.join(win64, f))));
    });

    it("clears old runtime files a previous install had to move aside", () => {
        const p = paths(tempDir("publish-aside"));
        write(p.runtimeDll, Buffer.alloc(6000, 5));
        write(p.loaderDll, Buffer.alloc(2000, 6));
        publishRuntime(p);
        const win64 = tempDir("win64-aside");
        fs.writeFileSync(path.join(win64, "MysticParadox.dll.rr-old-123"), "old");
        const r = installServerRuntime(p, win64);
        assert.deepEqual(r.inUse, []);
        assert.equal(fs.existsSync(path.join(win64, "MysticParadox.dll.rr-old-123")), false);
        assert.equal(fs.existsSync(path.join(win64, "MysticParadox.dll.tmp")), false);
    });

    it("refuses before anything is built", () => {
        assert.throws(() => publishRuntime(paths(tempDir("nopublish"))), /not built/);
    });
});

describe("game data", () => {
    it("tells real from example data and never overwrites a file", () => {
        const dir = tempDir("gamedata");
        assert.equal(gameDataState(dir, BACKEND_GAME_DATA).state, "missing");
        for (const f of BACKEND_GAME_DATA) write(path.join(dir, f.replace(/\.json$/, ".example.json")), `{"example":"${f}"}`);
        assert.equal(useExamples(dir, BACKEND_GAME_DATA).length, BACKEND_GAME_DATA.length);
        const synthetic = gameDataState(dir, BACKEND_GAME_DATA);
        assert.equal(synthetic.state, "synthetic");
        assert.equal(synthetic.synthetic.length, BACKEND_GAME_DATA.length);
        for (const f of BACKEND_GAME_DATA) write(path.join(dir, f), `{"real":"${f}"}`);
        assert.equal(useExamples(dir, BACKEND_GAME_DATA).length, 0);
        assert.equal(gameDataState(dir, BACKEND_GAME_DATA).state, "real");
    });

    it("skips the steps whose export or supplied source is not there", () => {
        const p = paths(tempDir("steps"));
        const steps = gameDataSteps(p, { examplesForMissing: false });
        assert.equal(steps.length, 7);
        assert.ok(steps.every((s) => s.skipIf?.() !== null), "nothing exported yet: every step is skipped with a reason");
        assert.equal(steps[0].title, "Progression config", "the progression config goes first: Ramsgate needs it");
        write(path.join(p.exports, "hunts_1_14_7", "player_hunts.jsonl"), "{}\n");
        assert.equal(steps.find((s) => s.title === "Director hunt tables")?.skipIf?.(), null, "the hunt tables run once the hunts are exported");
    });

    it("runs the other steps when one fails, and names the error, not Node's version line", async () => {
        const p = paths(tempDir("gen"));
        write(path.join(p.backendDir, "package.json"), JSON.stringify({ name: "b", private: true, scripts: { "generate:progression-config": "node fail.js", "reward-cache:generate": "node ok.js" } }));
        write(path.join(p.backendDir, "fail.js"), "throw new Error('Catalog item not found: x');\n");
        write(path.join(p.backendDir, "ok.js"), "require('fs').writeFileSync('ran.txt', 'yes');\n");
        write(path.join(p.rawGameData, SUPPLIED_SOURCES["progression_config.json"]), "{}\n");
        write(path.join(p.exports, "catalog_1_14_7.jsonl"), "{}\n");
        const lines: string[] = [];
        const r = await generateGameData(p, { examplesForMissing: false, logFile: path.join(p.logs, "gamedata.log"), print: (l) => lines.push(l) });
        assert.deepEqual(r, { ok: false, failed: ["Progression config"] });
        assert.ok(fs.existsSync(path.join(p.backendDir, "ran.txt")), "the Reward Cache step still ran");
        assert.ok(lines.some((l) => l.includes("failed: Error: Catalog item not found: x")), lines.join("\n"));
    });
});

describe("services", () => {
    it("get only the system's variables from the shell, never an old server setting", () => {
        const env = serviceEnvironment({ PATH: "/bin", SystemRoot: "C:\\Windows", REFORGED_BACKEND_URL: "http://evil", MYSTPAX_X: "1", MONGODB_URI: "mongodb://elsewhere", NODE_ENV: "development", GATEWAY_SECRET: "old" }, { MONGODB_URI: "mongodb://127.0.0.1:27017/?replicaSet=rs0", NODE_ENV: "production" });
        assert.deepEqual(env, { PATH: "/bin", SystemRoot: "C:\\Windows", MONGODB_URI: "mongodb://127.0.0.1:27017/?replicaSet=rs0", NODE_ENV: "production" });
    });

    it("start a process, wait until it answers, restart it when it dies, and stop it", async () => {
        const dir = tempDir("svc");
        const script = path.join(dir, "svc.js");
        const counter = path.join(dir, "starts.txt");
        write(script, `
            const fs = require("fs");
            fs.appendFileSync(${JSON.stringify(counter)}, "x");
            const starts = fs.readFileSync(${JSON.stringify(counter)}, "utf8").length;
            const server = require("http").createServer((q, s) => s.end(process.env.GREETING || "none")).listen(62061, "127.0.0.1", () => console.log("listening " + starts));
            if (starts === 1) setTimeout(() => process.exit(3), 300);
        `);
        const lines: string[] = [];
        const sup = new Supervisor(path.join(dir, "logs"), (name, line) => lines.push(`${name}: ${line}`));
        const ok = await sup.start({ name: "svc", cwd: dir, command: process.execPath, args: [script], envFile: null, extraEnv: { GREETING: "hello" }, ready: { port: 62061, path: "/" } }, 10000);
        assert.equal(ok, true);
        // It exits once; the supervisor starts it again after its backoff.
        const until = Date.now() + 10000;
        while (fs.readFileSync(counter, "utf8").length < 2 && Date.now() < until) await new Promise((r) => setTimeout(r, 100));
        assert.equal(fs.readFileSync(counter, "utf8").length, 2);
        await sup.stopAll();
        assert.ok(lines.some((l) => l.includes("restarting")));
        assert.ok(fs.readFileSync(path.join(dir, "logs", "svc.log"), "utf8").includes("listening 1"));
    });
});

describe("MongoDB", () => {
    it("runs as a one-member replica set on loopback, and is first reached directly", () => {
        const args = mongodArgs("D:\\rr\\.workspace\\mongodb", "D:\\rr\\.workspace\\logs\\mongod.log", 27017);
        assert.deepEqual(args.slice(0, 6), ["--replSet", "rs0", "--bind_ip", "127.0.0.1", "--port", "27017"]);
        assert.equal(directUri("mongodb://127.0.0.1:27017/?replicaSet=rs0"), "mongodb://127.0.0.1:27017/?directConnection=true");
    });
});
