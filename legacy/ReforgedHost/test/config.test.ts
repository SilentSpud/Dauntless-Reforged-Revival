import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { describe, it } from "node:test";
import { formatEnv, parseEnv } from "../src/envfile";
import { buildEnvFiles, DEFAULT_MONGO_URI, loadOrCreateSecrets, newSecrets, readHostConfig, validateHostConfig, writeEnvFiles, writeHostConfig, type HostConfig } from "../src/hostconfig";
import { paths } from "../src/paths";
import { tempDir } from "./helpers";

const PRIVATE: HostConfig = {
    version: 1,
    mode: "private",
    host: "100.101.102.103",
    gameAddress: "100.101.102.103",
    name: "Friday Hunts",
    gameDir: "D:\\Games\\Dauntless",
    registration: "INVITECODE",
    shareUrl: null,
    mongo: { uri: DEFAULT_MONGO_URI, manage: true, mongod: null },
    maxHuntServers: 3,
};

const PUBLIC: HostConfig = { ...PRIVATE, mode: "public", host: "hunts.example.net", gameAddress: "203.0.113.7" };

function get(entries: [string, string][], key: string): string | undefined {
    return entries.find(([k]) => k === key)?.[1];
}

describe("env files", () => {
    it("round-trip plain, quoted and base64 values", () => {
        const text = formatEnv(["a comment"], [["A", "plain"], ["B", "has space \"and\" quotes"], ["C", "abc+/=="], ["D", ""], ["E", "C:\\Games\\Dauntless"]]);
        assert.deepEqual(parseEnv(text), { A: "plain", B: 'has space "and" quotes', C: "abc+/==", D: "", E: "C:\\Games\\Dauntless" });
        assert.deepEqual(parseEnv("# x\n\nexport K=v\nBAD LINE\n=x\nQ='single'\n"), { K: "v", Q: "single" });
        assert.throws(() => formatEnv([], [["X", "a\nb"]]));
    });
});

describe("host.json", () => {
    it("accepts a private server on a Tailscale address and a public one on a public address or name", () => {
        assert.equal(validateHostConfig(PRIVATE).host, "100.101.102.103");
        assert.equal(validateHostConfig({ ...PRIVATE, host: "hostpc.tail1234.ts.net" }).host, "hostpc.tail1234.ts.net");
        assert.equal(validateHostConfig(PUBLIC).host, "hunts.example.net");
        assert.equal(validateHostConfig({ ...PUBLIC, host: "203.0.113.7" }).mode, "public");
    });

    it("refuses addresses that cannot work for the mode, and other bad values", () => {
        assert.throws(() => validateHostConfig({ ...PRIVATE, host: "192.168.1.20" }), /Tailscale/);
        assert.throws(() => validateHostConfig({ ...PRIVATE, gameAddress: "203.0.113.7" }), /Tailscale/);
        assert.throws(() => validateHostConfig({ ...PUBLIC, host: "192.168.1.20" }), /public/);
        assert.throws(() => validateHostConfig({ ...PUBLIC, host: "100.101.102.103" }), /public/);
        assert.throws(() => validateHostConfig({ ...PUBLIC, gameAddress: "hunts.example.net" }), /IPv4/);
        assert.throws(() => validateHostConfig({ ...PRIVATE, name: "  " }), /name/);
        assert.throws(() => validateHostConfig({ ...PRIVATE, maxHuntServers: 11 }), /maxHuntServers/);
        assert.throws(() => validateHostConfig({ ...PRIVATE, maxHuntServers: 0 }), /maxHuntServers/);
        assert.equal(validateHostConfig({ ...PRIVATE, maxHuntServers: 10 }).maxHuntServers, 10);
        assert.throws(() => validateHostConfig({ ...PRIVATE, shareUrl: "https://evil.example/" }), /shareUrl/);
        assert.throws(() => validateHostConfig({ ...PRIVATE, mongo: { ...PRIVATE.mongo, uri: "http://x" } }), /mongo/);
    });

    it("is written and read back", () => {
        const file = path.join(tempDir("hostjson"), "host.json");
        writeHostConfig(file, { ...PRIVATE, name: "  Friday\u0007 Hunts " });
        assert.equal(readHostConfig(file)?.name, "Friday Hunts");
        assert.equal(readHostConfig(path.join(path.dirname(file), "none.json")), null);
    });
});

describe("secrets", () => {
    it("are made once and kept; a missing one is added without touching the others", () => {
        const dir = tempDir("secrets");
        let made = 0;
        const make = () => {
            made++;
            return newSecretsFast();
        };
        const first = loadOrCreateSecrets(dir, make);
        assert.equal(first.created, true);
        const again = loadOrCreateSecrets(dir, make);
        assert.equal(again.created, false);
        assert.deepEqual(again.secrets, first.secrets);
        const file = path.join(dir, "secrets.env");
        fs.writeFileSync(file, fs.readFileSync(file, "utf8").replace(/^REFORGED_ADMIN_KEY=.*$/m, ""));
        const third = loadOrCreateSecrets(dir, make);
        assert.equal(third.created, true);
        assert.notEqual(third.secrets.adminKey, first.secrets.adminKey);
        assert.equal(third.secrets.gatewaySecret, first.secrets.gatewaySecret);
        assert.equal(third.secrets.jwtPrivateKeyB64, first.secrets.jwtPrivateKeyB64);
        assert.equal(made, 2);
    });

    it("include a 4096-bit RSA pair for the backend's tokens, base64 PEM", () => {
        const s = newSecrets();
        assert.match(Buffer.from(s.jwtPrivateKeyB64, "base64").toString(), /BEGIN PRIVATE KEY/);
        assert.match(Buffer.from(s.jwtPublicKeyB64, "base64").toString(), /BEGIN PUBLIC KEY/);
        assert.match(s.adminKey, /^[0-9a-f]{64}$/);
    });
});

function newSecretsFast() {
    const hex = () => require("node:crypto").randomBytes(32).toString("hex") as string;
    return { gatewaySecret: hex(), allowlistSecret: hex(), adminKey: hex(), gameserverKey: hex(), apiKeyHashSecret: hex(), metricsToken: hex(), jwtPrivateKeyB64: Buffer.from(hex()).toString("base64"), jwtPublicKeyB64: Buffer.from(hex()).toString("base64") };
}

describe("the service env files", () => {
    const p = paths("C:\\rr");
    const s = newSecretsFast();

    it("private mode: the backend behind the gateway, the gateway on the Tailscale address in plain HTTP, no allowlist", () => {
        const f = buildEnvFiles(PRIVATE, s, p, { sourceCommit: "abc123", platform: "win32" });
        assert.equal(get(f.backend, "AUTH_MODE"), "APIKEY");
        assert.equal(get(f.backend, "NODE_ENV"), "production");
        assert.equal(get(f.backend, "MYSTICPARADOX_SERVICE_ROLE"), "api");
        assert.equal(get(f.backend, "BIND_HOST"), "127.0.0.1");
        assert.equal(get(f.backend, "REFORGED_HTTP_ONLY"), "true");
        assert.equal(get(f.backend, "QOS_TARGET_URL"), "http://127.0.0.1:61100/QoS");
        assert.equal(get(f.backend, "REALTIME_XMPP_DEV_CITY_MUC"), "false");
        assert.equal(get(f.backend, "HUNT_GO_NOW_MIN_DELAY_MS"), "1", "no 10-20 s wait (with the build locked) after the hunt server is ready");
        assert.equal(get(f.backend, "HUNT_GO_NOW_MAX_DELAY_MS"), "1000");
        assert.equal(get(f.backend, "REALTIME_XMPP_ALLOWED_HOSTS"), "100.101.102.103");
        assert.equal(get(f.backend, "GAMESERVER_API_KEYS"), s.gameserverKey);
        assert.equal(get(f.director, "METAGAME_API_KEY"), s.gameserverKey, "the Director's game servers use the backend's game-server key");
        assert.equal(get(f.director, "MY_IP"), "100.101.102.103");
        assert.equal(get(f.director, "REFORGED_BACKEND_URL"), "http://127.0.0.1:61000");
        assert.equal(get(f.director, "PORT_RANGE_BEGIN"), "8770");
        assert.equal(get(f.director, "PORT_RANGE_END"), "8777");
        assert.match(get(f.director, "GAMESERVER_BINARY_PATH") ?? "", /Dauntless-Win64-Shipping\.exe$/);
        assert.equal(get(f.gateway, "GATEWAY_TLS"), "0");
        assert.equal(get(f.gateway, "GATEWAY_BIND"), "100.101.102.103");
        assert.equal(get(f.gateway, "GATEWAY_PORT"), "61000");
        assert.equal(get(f.gateway, "GATEWAY_ALLOWLIST"), "0");
        assert.equal(f.allowlist, null);
        assert.equal(get(f.content, "CONTENT_RUNTIME_DIR"), p.runtimePublish);
    });

    it("public mode: TLS on 443 with the certificate, the allowlist helper for real on Windows and Linux", () => {
        const f = buildEnvFiles(PUBLIC, s, p, { platform: "win32" });
        assert.equal(get(f.gateway, "GATEWAY_TLS"), "1");
        assert.equal(get(f.gateway, "GATEWAY_BIND"), "0.0.0.0");
        assert.equal(get(f.gateway, "GATEWAY_PORT"), "443");
        assert.equal(get(f.gateway, "GATEWAY_CERT"), p.certFile);
        assert.equal(get(f.gateway, "ALLOWLIST_SECRET"), s.allowlistSecret);
        assert.equal(get(f.allowlist ?? [], "ALLOWLIST_SECRET"), s.allowlistSecret);
        assert.equal(get(f.allowlist ?? [], "ALLOWLIST_DRY_RUN"), "0");
        assert.equal(get(f.allowlist ?? [], "ALLOWLIST_FIREWALL"), "windows");
        const linux = buildEnvFiles(PUBLIC, s, p, { platform: "linux" }).allowlist ?? [];
        assert.equal(get(linux, "ALLOWLIST_DRY_RUN"), "0");
        assert.equal(get(linux, "ALLOWLIST_FIREWALL"), "nftables");
        assert.equal(get(buildEnvFiles(PUBLIC, s, p, { platform: "darwin" }).allowlist ?? [], "ALLOWLIST_DRY_RUN"), "1");
        assert.equal(get(f.director, "MY_IP"), "203.0.113.7");
        assert.equal(get(f.backend, "REALTIME_XMPP_ALLOWED_HOSTS"), "hunts.example.net");
    });

    it("test multipliers: only a private server's backend gets them, always with its mode and host", () => {
        const on = buildEnvFiles({ ...PRIVATE, multipliers: { xp: 5, rewards: 3 } }, s, p, { platform: "win32" });
        assert.equal(get(on.backend, "REFORGED_SERVER_MODE"), "private");
        assert.equal(get(on.backend, "REFORGED_SERVER_HOST"), "100.101.102.103");
        assert.equal(get(on.backend, "REFORGED_XP_MULTIPLIER"), "5");
        assert.equal(get(on.backend, "REFORGED_REWARD_MULTIPLIER"), "3");
        const off = buildEnvFiles(PRIVATE, s, p, { platform: "win32" });
        assert.equal(get(off.backend, "REFORGED_XP_MULTIPLIER"), undefined);
        const pub = buildEnvFiles(PUBLIC, s, p, { platform: "win32" });
        assert.equal(get(pub.backend, "REFORGED_SERVER_MODE"), "public");
        assert.equal(get(pub.backend, "REFORGED_XP_MULTIPLIER"), undefined);
        assert.throws(() => validateHostConfig({ ...PUBLIC, multipliers: { xp: 5, rewards: 1 } }), /private/);
        assert.throws(() => validateHostConfig({ ...PRIVATE, multipliers: { xp: 101, rewards: 1 } }), /1 to 100/);
        assert.throws(() => validateHostConfig({ ...PRIVATE, multipliers: { xp: 0.5, rewards: 1 } }), /1 to 100/);
        assert.equal(validateHostConfig({ ...PUBLIC, multipliers: { xp: 1, rewards: 1 } }).mode, "public");
    });

    it("are written owner-only, and the allowlist file goes away in private mode", () => {
        const root = tempDir("envfiles");
        const lp = paths(root);
        writeEnvFiles(lp, buildEnvFiles(PUBLIC, s, lp, { platform: "linux" }));
        assert.ok(fs.existsSync(path.join(lp.config, "allowlist.env")));
        writeEnvFiles(lp, buildEnvFiles(PRIVATE, s, lp, { platform: "linux" }));
        assert.ok(!fs.existsSync(path.join(lp.config, "allowlist.env")));
        const backend = parseEnv(fs.readFileSync(path.join(lp.config, "backend.env"), "utf8"));
        assert.equal(backend.REFORGED_ADMIN_KEY, s.adminKey);
        if (process.platform !== "win32") assert.equal(fs.statSync(path.join(lp.config, "backend.env")).mode & 0o077, 0);
    });
});
