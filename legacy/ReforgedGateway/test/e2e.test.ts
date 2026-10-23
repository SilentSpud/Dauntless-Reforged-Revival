import assert from "node:assert/strict";
import { ChildProcess, spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { after, before, describe, it } from "node:test";
import { AllowlistConfig, DefaultPowerShell } from "../src/allowlist/config";
import { BuildFirewallScript } from "../src/allowlist/firewall";
import { AllowlistHelper, AuditLog, DryRunRunner } from "../src/allowlist/helper";
import { AllowlistFeed } from "../src/feed";
import { Gateway } from "../src/gateway";
import { CaptureLog, CapturedLog, FakeUpstream, HttpsRequest, MakeCert, ROOT, StartUpstream, TempDir, TestCert, TestConfig, WaitFor } from "./helpers";

// Ports: gateway 62430, allowlist helper 62431, fake backend 62432, fake content 62433, ws 62434
// (unused); as separate processes: helper 62435, gateway 62436. The helper always runs in DRY-RUN.
const P = { gateway: 62430, helper: 62431, backend: 62432, content: 62433, ws: 62434, helperProcess: 62435, gatewayProcess: 62436 };
const HELPER_SECRET = "e2e-helper-secret-0123456789abcdef0123456789abcdef";
const GATEWAY_SECRET = "e2e-gateway-secret-0123456789abcdef0123456789abcdef";

function AuditEvents(File: string): any[] {
    if(!fs.existsSync(File)){
        return [];
    }
    return fs.readFileSync(File, "utf8").split("\n").filter((Line) => Line !== "").map((Line) => JSON.parse(Line));
}

function FakeBackend(Port: number): Promise<FakeUpstream> {
    return StartUpstream(Port, "backend", (Req, Res) => {
        if((Req.url ?? "").split("?")[0] === "/heartbeat"){
            const Ok = Req.headers["authorization"] === "Bearer good-token";
            Res.writeHead(Ok ? 200 : 401, { "content-type": "text/plain" });
            Res.end(Ok ? "20000" : "");
            return true;
        }
        return false;
    });
}

describe("gateway and allowlist helper together (in process, DRY-RUN)", () => {
    let Cert: TestCert;
    let Meta: FakeUpstream;
    let Content: FakeUpstream;
    let Helper: AllowlistHelper;
    let HelperConfig: AllowlistConfig;
    let Feed: AllowlistFeed;
    let TheGateway: Gateway;
    let Log: CapturedLog;

    before(async () => {
        Log = CaptureLog();
        Cert = MakeCert(["127.0.0.1"]);
        Meta = await FakeBackend(P.backend);
        Content = await StartUpstream(P.content, "content");
        const Dir = TempDir("e2e");
        HelperConfig = {
            bindHost: "127.0.0.1",
            port: P.helper,
            secret: HELPER_SECRET,
            dryRun: true,
            allowPrivate: true,
            ttlMs: 600_000,
            minIntervalMs: 200,
            maxEntries: 256,
            ports: "8770-8777",
            auditLog: path.join(Dir, "audit.log"),
            stateFile: path.join(Dir, "state.json"),
            powershell: DefaultPowerShell(),
            firewall: "windows",
            nft: { binary: "/usr/sbin/nft", table: "reforged_revival", iface: undefined },
        };
        const Audit = new AuditLog(HelperConfig.auditLog);
        Helper = new AllowlistHelper(HelperConfig, { audit: Audit, runner: DryRunRunner(Audit) });
        await Helper.Start();
        Feed = new AllowlistFeed({ url: `http://127.0.0.1:${P.helper}`, secret: HELPER_SECRET, refreshMs: 60_000, timeoutMs: 2000 });
        TheGateway = new Gateway(TestConfig({ gateway: P.gateway, backend: P.backend, content: P.content, ws: P.ws }), { cert: Cert.certPem, key: Cert.keyPem }, Feed);
        await TheGateway.Listen();
    });

    after(async () => {
        await TheGateway.Close(200);
        await Helper.Close();
        await Promise.all([Meta.close(), Content.close()]);
        Log.stop();
    });

    it("a logged-in heartbeat opens the game ports for exactly that address; a failed one does not", async () => {
        const Refused = await HttpsRequest(P.gateway, "/heartbeat", { method: "POST", body: "{}", localAddress: "127.0.0.42", headers: { authorization: "Bearer stolen" } });
        assert.equal(Refused.status, 401);
        const Ok = await HttpsRequest(P.gateway, "/heartbeat", { method: "POST", body: "{}", localAddress: "127.0.0.40", headers: { authorization: "Bearer good-token" } });
        assert.equal(Ok.status, 200);
        await Feed.Idle();
        await Helper.Sync.Settled();
        assert.deepEqual(Helper.State.List(), ["127.0.0.40"]);
        const DryRuns = AuditEvents(HelperConfig.auditLog).filter((Event) => Event.event === "dry_run");
        assert.equal(DryRuns[DryRuns.length - 1].script, BuildFirewallScript(["127.0.0.40"], "8770-8777"));
        assert.ok(!fs.readFileSync(HelperConfig.auditLog, "utf8").includes(HELPER_SECRET));
    });

    it("a gateway with the wrong helper secret gets 401 and says so in its log", async () => {
        const Wrong = new AllowlistFeed({ url: `http://127.0.0.1:${P.helper}`, secret: "x".repeat(40), refreshMs: 60_000, timeoutMs: 2000 });
        Wrong.Report("203.0.113.99");
        await Wrong.Idle();
        Wrong.Close();
        assert.equal(Helper.State.List().includes("203.0.113.99"), false);
        assert.ok(Log.lines.some((Line) => Line.level === "warn" && /allowlist helper did not take an address/.test(Line.entry.msg) && Line.entry.detail === "status 401"));
        assert.ok(AuditEvents(HelperConfig.auditLog).some((Event) => Event.event === "unauthorized"));
    });
});

// The compiled entry points, started the way the server starts them (node <file> with an env).
const GATEWAY_MAIN = path.join(ROOT, "build", "src", "server.js");
const HELPER_MAIN = path.join(ROOT, "build", "src", "allowlist", "server.js");

function BaseEnv(): NodeJS.ProcessEnv {
    return { SystemRoot: process.env.SystemRoot, PATH: process.env.PATH, TEMP: process.env.TEMP, TMP: process.env.TMP };
}

function Run(Main: string, Env: NodeJS.ProcessEnv, Args: string[] = []){
    return spawnSync(process.execPath, [Main, ...Args], { env: Env, encoding: "utf8", timeout: 30_000, cwd: TempDir("run") });
}

type Started = { child: ChildProcess; lines: any[]; stop: () => void };

function Launch(Main: string, Env: NodeJS.ProcessEnv, Ready: RegExp): Promise<Started> {
    return new Promise((resolve, reject) => {
        const Child = spawn(process.execPath, [Main], { env: Env, cwd: TempDir("launch"), stdio: ["ignore", "pipe", "pipe"] });
        const Lines: any[] = [];
        let Buffered = "";
        const OnData = (Chunk: Buffer) => {
            Buffered += Chunk.toString("utf8");
            let Newline: number;
            while((Newline = Buffered.indexOf("\n")) !== -1){
                const Line = Buffered.slice(0, Newline);
                Buffered = Buffered.slice(Newline + 1);
                try{
                    const Entry = JSON.parse(Line);
                    Lines.push(Entry);
                    if(Ready.test(Entry.msg)){
                        resolve({ child: Child, lines: Lines, stop: () => Child.kill() });
                    }
                }
                catch{
                    Lines.push({ raw: Line });
                }
            }
        };
        Child.stdout.on("data", OnData);
        Child.stderr.on("data", OnData);
        Child.once("exit", (Code) => reject(new Error(`exited with ${Code}: ${JSON.stringify(Lines).slice(0, 1000)}`)));
        setTimeout(() => reject(new Error("did not become ready")), 20_000).unref();
    });
}

describe("the entry points as processes", () => {
    let Cert: TestCert;
    let OtherCert: TestCert;
    let Meta: FakeUpstream;
    const Children: Started[] = [];

    before(async () => {
        Cert = MakeCert(["127.0.0.1"]);
        OtherCert = MakeCert(["127.0.0.1"]);
        Meta = await FakeBackend(P.backend);
    });

    after(async () => {
        for(const Child of Children){
            Child.stop();
        }
        await Meta.close();
    });

    it("the helper refuses to start without an explicit ALLOWLIST_DRY_RUN", () => {
        const Result = Run(HELPER_MAIN, { ...BaseEnv(), ALLOWLIST_SECRET: HELPER_SECRET, ALLOWLIST_PORT: String(P.helperProcess) });
        assert.equal(Result.status, 1);
        assert.match(Result.stderr, /ALLOWLIST_DRY_RUN must be set/);
    });

    it("the helper's --close-ports disables the rule once and exits (for stop scripts)", () => {
        const Dir = TempDir("close-ports");
        const Audit = path.join(Dir, "audit.log");
        const Result = Run(HELPER_MAIN, { ...BaseEnv(), ALLOWLIST_DRY_RUN: "1", ALLOWLIST_SECRET: HELPER_SECRET, ALLOWLIST_AUDIT_LOG: Audit }, ["--close-ports"]);
        assert.equal(Result.status, 0, Result.stderr);
        const Events = AuditEvents(Audit);
        assert.deepEqual(Events.map((Event) => Event.event), ["dry_run", "closed_ports"]);
        assert.equal(Events[0].script, BuildFirewallScript([], "8770-8777"));
    });

    it("the gateway refuses to start with bad configuration or a key that is not the certificate's", () => {
        const Missing = Run(GATEWAY_MAIN, { ...BaseEnv() });
        assert.equal(Missing.status, 1);
        assert.match(Missing.stderr, /Configuration: GATEWAY_CERT and GATEWAY_KEY/);

        const Mismatch = Run(GATEWAY_MAIN, {
            ...BaseEnv(),
            GATEWAY_CERT: Cert.cert,
            GATEWAY_KEY: OtherCert.key,
            GATEWAY_SECRET,
            ALLOWLIST_SECRET: HELPER_SECRET,
            GATEWAY_BIND: "127.0.0.1",
            GATEWAY_PORT: String(P.gatewayProcess),
        });
        assert.equal(Mismatch.status, 1);
        assert.match(Mismatch.stderr, /Certificate .* key/);
    });

    it("helper (DRY-RUN) and gateway run as their own processes and open the ports for a logged-in player", async () => {
        const Dir = TempDir("processes");
        const Audit = path.join(Dir, "audit.log");
        const HelperProcess = await Launch(HELPER_MAIN, {
            ...BaseEnv(),
            ALLOWLIST_DRY_RUN: "1",
            ALLOWLIST_ALLOW_PRIVATE: "1",
            ALLOWLIST_SECRET: HELPER_SECRET,
            ALLOWLIST_PORT: String(P.helperProcess),
            ALLOWLIST_MIN_INTERVAL_MS: "200",
            ALLOWLIST_AUDIT_LOG: Audit,
            ALLOWLIST_STATE_FILE: path.join(Dir, "state.json"),
        }, /^allowlist helper ready$/);
        Children.push(HelperProcess);
        assert.ok(HelperProcess.lines.some((Line) => /^DRY RUN/.test(Line.msg ?? "")));

        const GatewayProcess = await Launch(GATEWAY_MAIN, {
            ...BaseEnv(),
            GATEWAY_CERT: Cert.cert,
            GATEWAY_KEY: Cert.key,
            GATEWAY_SECRET,
            GATEWAY_BIND: "127.0.0.1",
            GATEWAY_PORT: String(P.gatewayProcess),
            GATEWAY_BACKEND_URL: `http://127.0.0.1:${P.backend}`,
            GATEWAY_CONTENT_URL: `http://127.0.0.1:${P.content}`,
            GATEWAY_WS_URL: `http://127.0.0.1:${P.ws}`,
            ALLOWLIST_URL: `http://127.0.0.1:${P.helperProcess}`,
            ALLOWLIST_SECRET: HELPER_SECRET,
        }, /^gateway ready$/);
        Children.push(GatewayProcess);
        const Ready = GatewayProcess.lines.find((Line) => Line.msg === "gateway ready");
        assert.equal(Ready.fingerprint, Cert.fingerprint);
        assert.equal(Ready.listen, `127.0.0.1:${P.gatewayProcess}`);
        assert.ok(!JSON.stringify(GatewayProcess.lines).includes(GATEWAY_SECRET));
        assert.ok(!JSON.stringify(GatewayProcess.lines).includes(HELPER_SECRET));

        const Before = Meta.seen.length;
        const Reply = await HttpsRequest(P.gatewayProcess, "/heartbeat", { method: "POST", body: "{}", headers: { authorization: "Bearer good-token" }, pin: Cert.fingerprint });
        assert.equal(Reply.status, 200);
        assert.equal(Meta.seen[Before].headers["x-reforged-gateway"], GATEWAY_SECRET);
        assert.equal(Meta.seen[Before].headers["x-forwarded-for"], "127.0.0.1");

        await WaitFor(() => AuditEvents(Audit).some((Event) => Event.event === "dry_run" && Event.script === BuildFirewallScript(["127.0.0.1"], "8770-8777")), 5000, "the dry-run rule change");
        const Status = await new Promise<any>((resolve, reject) => {
            const Req = http.request({ host: "127.0.0.1", port: P.helperProcess, path: "/status", headers: { "x-allowlist-secret": HELPER_SECRET }, agent: false }, (Res) => {
                let Text = "";
                Res.on("data", (Chunk) => Text += Chunk);
                Res.on("end", () => resolve(JSON.parse(Text)));
            });
            Req.on("error", reject);
            Req.end();
        });
        assert.equal(Status.dryRun, true);
        assert.deepEqual(Status.entries.map((Entry: any) => Entry.ip), ["127.0.0.1"]);

        // An admin route through the real process: refused.
        const Admin = await HttpsRequest(P.gatewayProcess, "/reforged/api/CreateInvite", { method: "POST", body: "{}", pin: Cert.fingerprint });
        assert.equal(Admin.status, 403);
    });
});
