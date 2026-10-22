import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import tls from "node:tls";
import { describe, it } from "node:test";
import { parseArgs, publicPortFromArgs } from "../src/cli";
import { portOwnerHint } from "../src/doctor";
import { buildEnvFiles, DEFAULT_MONGO_URI, gatewayPort, newSecrets, validateHostConfig, type HostConfig } from "../src/hostconfig";
import { certificateFingerprint, inviteFor, servedFingerprint } from "../src/invite";
import { linuxFirewallScript, systemdUnits, UNIT_SERVER } from "../src/linux";
import { paths } from "../src/paths";
import { parseInvite } from "../../ReforgedLauncher/src/shared/invite";
import { tempDir } from "./helpers";

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
};
const MOVED: HostConfig = { ...PUBLIC, publicPort: 4443 };

function get(entries: [string, string][], key: string): string | undefined {
    return entries.find(([k]) => k === key)?.[1];
}

describe("the public gateway's port (--public-port, when 443 is taken)", () => {
    it("is 443 unless moved, 61000 for a private server", () => {
        assert.equal(gatewayPort(PUBLIC), 443);
        assert.equal(gatewayPort(MOVED), 4443);
        assert.equal(gatewayPort({ mode: "private" }), 61000);
    });

    it("comes from --public-port, is kept when not given, and --public-port 443 goes back", () => {
        assert.deepEqual(publicPortFromArgs(parseArgs(["--public-port", "4443"]), "public", null), { publicPort: 4443 });
        assert.deepEqual(publicPortFromArgs(parseArgs([]), "public", MOVED), { publicPort: 4443 });
        assert.deepEqual(publicPortFromArgs(parseArgs(["--public-port", "443"]), "public", MOVED), {});
        assert.deepEqual(publicPortFromArgs(parseArgs([]), "private", MOVED), {});
        assert.throws(() => publicPortFromArgs(parseArgs(["--public-port", "4443"]), "private", null), /public servers only/);
        assert.throws(() => publicPortFromArgs(parseArgs(["--public-port", "web"]), "public", null), /port number/);
    });

    it("is checked: a real port, not one the server uses itself, public servers only", () => {
        assert.equal(validateHostConfig(MOVED).publicPort, 4443);
        for (const bad of [0, 70000, 61000, 61005, 27017, 8777, 8766, 61100]) {
            assert.throws(() => validateHostConfig({ ...PUBLIC, publicPort: bad }), /publicPort/, String(bad));
        }
        assert.throws(() => validateHostConfig({ ...PUBLIC, mode: "private", host: "100.101.102.103", gameAddress: "100.101.102.103", publicPort: 4443 }), /public servers only/);
    });

    it("reaches the gateway's env file, the invite (which the launcher reads), the firewall script and the units", () => {
        const p = paths("/home/reforged/Reforged");
        assert.equal(get(buildEnvFiles(MOVED, newSecrets(), p, { platform: "linux" }).gateway, "GATEWAY_PORT"), "4443");
        const parsed = parseInvite(inviteFor(MOVED, "ABCD-EFGH-IJKL", "a".repeat(64)));
        assert.ok(parsed.ok);
        if (parsed.ok) assert.equal(parsed.invite.port, 4443);
        assert.match(linuxFirewallScript(MOVED), /ufw allow 4443\/tcp/);
        // Above 1024: no capability at all for the server unit.
        const unit = systemdUnits(MOVED, p, { user: "reforged", node: "/usr/bin/node" })[UNIT_SERVER];
        assert.doesNotMatch(unit, /AmbientCapabilities/);
        assert.match(unit, /^CapabilityBoundingSet=$/m);
        assert.match(unit, /^Environment=REFORGED_ALLOWLIST=external$/m);
    });

    it("tells how to find the program on a port", () => {
        assert.match(portOwnerHint(443, "linux"), /ss -tlnp 'sport = :443'/);
        assert.match(portOwnerHint(443, "win32"), /netstat -ano \| findstr :443/);
    });
});

// A self-signed certificate made with openssl (skipped where there is none).
const openssl = spawnSync("openssl", ["version"], { encoding: "utf8" }).status === 0;

describe("the certificate a port serves (doctor: is it this server's gateway?)", { skip: !openssl && "needs openssl" }, () => {
    it("is the SHA-256 the invite pins, and null when nothing answers with TLS", async () => {
        const dir = tempDir("served");
        const cert = path.join(dir, "cert.pem");
        const key = path.join(dir, "key.pem");
        const made = spawnSync("openssl", ["req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes", "-days", "1", "-subj", "/CN=test", "-keyout", key, "-out", cert], { encoding: "utf8" });
        assert.equal(made.status, 0, made.stderr);
        const server = tls.createServer({ cert: fs.readFileSync(cert), key: fs.readFileSync(key) }, (s) => s.end());
        await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
        const port = (server.address() as { port: number }).port;
        try {
            assert.equal(await servedFingerprint(port), certificateFingerprint(cert));
        } finally {
            await new Promise<void>((resolve) => server.close(() => resolve()));
        }
        assert.equal(await servedFingerprint(port), null, "nothing listens any more");
    });
});
