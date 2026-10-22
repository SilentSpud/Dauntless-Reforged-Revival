import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { after, before, describe, it } from "node:test";
import { AllowlistConfig, DefaultPowerShell } from "../src/allowlist/config";
import { BuildFirewallScript, IsElevated, PowerShellRunner, RULE_DISPLAY_NAME, RULE_NAME } from "../src/allowlist/firewall";
import { AllowlistHelper, AuditLog, DryRunRunner } from "../src/allowlist/helper";
import { AllowlistState } from "../src/allowlist/state";
import { FirewallSync } from "../src/allowlist/sync";
import { ParseIp } from "../src/ip";
import { CaptureLog, CapturedLog, Sleep, TempDir, WaitFor } from "./helpers";

// Ports: helpers 62420, 62421, 62422. Every helper here runs in DRY-RUN: nothing in this file
// ever changes a firewall. PowerShell is only used to parse scripts and to read cmdlet metadata.
const P = { helper: 62420, helperPrivate: 62421, helperRestart: 62422 };
const SECRET = "allowlist-test-secret-0123456789abcdef0123456789";

const EMPTY_SCRIPT = [
    "$ErrorActionPreference = 'Stop'",
    "$Name = 'ReforgedRevival-GamePorts-Allowlist'",
    "$Display = 'Dauntless Reforged Revival game ports (allowlist)'",
    "Get-NetFirewallRule -DisplayName $Display -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne $Name } | Disable-NetFirewallRule",
    "$Rule = Get-NetFirewallRule -Name $Name -ErrorAction SilentlyContinue",
    "if ($null -ne $Rule) {",
    "    Set-NetFirewallRule -Name $Name -Enabled False",
    "}",
    "",
].join("\r\n");

const TWO_SCRIPT = [
    "$ErrorActionPreference = 'Stop'",
    "$Name = 'ReforgedRevival-GamePorts-Allowlist'",
    "$Display = 'Dauntless Reforged Revival game ports (allowlist)'",
    "Get-NetFirewallRule -DisplayName $Display -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne $Name } | Disable-NetFirewallRule",
    "$Rule = Get-NetFirewallRule -Name $Name -ErrorAction SilentlyContinue",
    "$Addresses = @('203.0.113.7', '2001:db8::1')",
    "if ($null -eq $Rule) {",
    "    New-NetFirewallRule -Name $Name -DisplayName $Display -Direction Inbound -Action Allow -Protocol UDP -LocalPort '8770-8777' -RemoteAddress $Addresses -Profile Any -Enabled True | Out-Null",
    "} else {",
    "    Set-NetFirewallRule -Name $Name -NewDisplayName $Display -Direction Inbound -Action Allow -Protocol UDP -LocalPort '8770-8777' -RemoteAddress $Addresses -Profile Any -Enabled True",
    "}",
    "",
].join("\r\n");

function Ip(Text: string){
    const Parsed = ParseIp(Text);
    assert.ok(Parsed, Text);
    return Parsed!;
}

describe("AllowlistState", () => {
    it("keeps an address for the TTL after its last report", () => {
        const State = new AllowlistState(600_000, 10);
        assert.equal(State.Refresh(Ip("203.0.113.7"), 0), "added");
        assert.equal(State.Refresh(Ip("203.0.113.7"), 1000), "refreshed");
        assert.deepEqual(State.Expire(600_999), []);
        assert.deepEqual(State.Expire(601_000), ["203.0.113.7"]);
        assert.equal(State.Size, 0);
    });

    it("lists IPv4 before IPv6, in numeric order", () => {
        const State = new AllowlistState(1000, 10);
        for(const Text of ["2001:db8::2", "10.0.0.1", "9.0.0.1", "2001:db8::1", "203.0.113.7"]){
            State.Refresh(Ip(Text), 0);
        }
        assert.deepEqual(State.List(), ["9.0.0.1", "10.0.0.1", "203.0.113.7", "2001:db8::1", "2001:db8::2"]);
    });

    it("refuses new addresses when full", () => {
        const State = new AllowlistState(1000, 2);
        assert.equal(State.Refresh(Ip("203.0.113.1"), 0), "added");
        assert.equal(State.Refresh(Ip("203.0.113.2"), 0), "added");
        assert.equal(State.Refresh(Ip("203.0.113.3"), 0), "full");
        assert.equal(State.Refresh(Ip("203.0.113.1"), 0), "refreshed");
    });

    it("restores a saved set, skipping stale, malformed and refused entries, never beyond one TTL", () => {
        const State = new AllowlistState(600_000, 10);
        const Loaded = State.Load([
            { ip: "203.0.113.7", expiresAt: 5_000_000 },
            { ip: "198.51.100.2", expiresAt: 999 },
            { ip: "10.0.0.1", expiresAt: 5_000 },
            { ip: "0.0.0.0/0", expiresAt: 5_000 },
            { ip: 7, expiresAt: 5_000 },
            null,
        ], 1000, (Parsed) => Parsed.class === "public");
        assert.equal(Loaded, 1);
        assert.deepEqual(State.Snapshot(), [{ ip: "203.0.113.7", expiresAt: 601_000 }]);
        assert.equal(State.Load("nonsense", 0, () => true), 0);
    });
});

describe("FirewallSync", () => {
    it("applies the first change at once and folds quick changes into one later run, never closer than the interval", async () => {
        const Runs: { at: number; ips: string[] }[] = [];
        let Running = 0;
        let MaxRunning = 0;
        const Sync = new FirewallSync(async (Ips) => {
            Running++;
            MaxRunning = Math.max(MaxRunning, Running);
            Runs.push({ at: Date.now(), ips: [...Ips] });
            await Sleep(50);
            Running--;
        }, 300);

        Sync.Request(["203.0.113.1"]);
        await WaitFor(() => Runs.length === 1, 1000, "first run");
        Sync.Request(["203.0.113.1", "203.0.113.2"]);
        Sync.Request(["203.0.113.1", "203.0.113.2", "203.0.113.3"]);
        await Sync.Settled(3000);
        assert.equal(Runs.length, 2);
        assert.deepEqual(Runs[1].ips, ["203.0.113.1", "203.0.113.2", "203.0.113.3"]);
        assert.ok(Runs[1].at - Runs[0].at >= 290, `runs ${Runs[1].at - Runs[0].at} ms apart`);
        assert.equal(MaxRunning, 1);

        // Asking for what is already applied does nothing.
        Sync.Request(["203.0.113.1", "203.0.113.2", "203.0.113.3"]);
        await Sleep(400);
        assert.equal(Runs.length, 2);

        // A change well after the interval applies at once again.
        const Asked = Date.now();
        Sync.Request([]);
        await WaitFor(() => Runs.length === 3, 1000, "third run");
        assert.ok(Runs[2].at - Asked < 100);
        assert.deepEqual(Runs[2].ips, []);
    });

    it("retries a failed run with backoff and reports each result", async () => {
        let Calls = 0;
        const Results: boolean[] = [];
        const Sync = new FirewallSync(async () => {
            Calls++;
            if(Calls === 1){
                throw new Error("access denied");
            }
        }, 100, (Result) => Results.push(Result.ok));
        Sync.Request(["203.0.113.9"]);
        await WaitFor(() => Results.length === 2, 3000, "retry");
        assert.deepEqual(Results, [false, true]);
        assert.equal(Sync.LastResult?.ok, true);
    });

    it("Final waits for a run in progress, then applies at once", async () => {
        const Runs: string[][] = [];
        const Sync = new FirewallSync(async (Ips) => {
            Runs.push([...Ips]);
            await Sleep(150);
        }, 10_000);
        Sync.Request(["203.0.113.1"]);
        await Sleep(20);
        await Sync.Final([]);
        assert.deepEqual(Runs, [["203.0.113.1"], []]);
        // Stopped: later requests are ignored.
        Sync.Request(["203.0.113.5"]);
        await Sleep(50);
        assert.equal(Runs.length, 2);
    });
});

describe("the firewall script", () => {
    it("is exactly this for an empty set: disable the rule", () => {
        assert.equal(BuildFirewallScript([], "8770-8777"), EMPTY_SCRIPT);
    });

    it("is exactly this for two addresses: one inbound UDP allow rule for the game ports, only from them", () => {
        assert.equal(BuildFirewallScript(["203.0.113.7", "2001:db8::1"], "8770-8777"), TWO_SCRIPT);
        assert.equal(RULE_NAME, "ReforgedRevival-GamePorts-Allowlist");
        assert.equal(RULE_DISPLAY_NAME, "Dauntless Reforged Revival game ports (allowlist)");
    });

    it("refuses anything but canonical single addresses, whatever the caller", () => {
        for(const Bad of ["2001:DB8::1", "1.2.3.4/32", "0.0.0.0", "any", "1.2.3.4'; Remove-NetFirewallRule -All; '", "", "010.1.1.1", "224.0.0.1"]){
            assert.throws(() => BuildFirewallScript([Bad], "8770-8777"), /refusing/, Bad);
        }
        assert.throws(() => BuildFirewallScript(["203.0.113.7"], "8770-8777; Remove-NetFirewallRule"), /ALLOWLIST_PORTS/);
    });
});

const OnWindows = process.platform === "win32";

describe("PowerShell (parse and metadata only; no firewall cmdlet ever runs)", { skip: !OnWindows && "Windows only" }, () => {
    const Exe = DefaultPowerShell();

    // Parses Script with PowerShell's own parser (nothing in it runs) and checks that every
    // command and parameter it uses exists on this Windows (Get-Command reads metadata only).
    function CheckScript(Script: string): Promise<void> {
        assert.ok(!/^'@/m.test(Script));
        const Checker = [
            "$s = @'",
            Script.replace(/\r?\n$/, ""),
            "'@",
            "$errors = $null",
            "$tokens = $null",
            "$ast = [System.Management.Automation.Language.Parser]::ParseInput($s, [ref]$tokens, [ref]$errors)",
            "if ($errors.Count -gt 0) { foreach ($e in $errors) { [Console]::Error.WriteLine('parse: ' + $e.Message) }; exit 5 }",
            "$commands = $ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] }, $true)",
            "foreach ($cmd in $commands) {",
            "    $name = $cmd.GetCommandName()",
            "    $info = Get-Command $name -ErrorAction SilentlyContinue",
            "    if ($null -eq $info) { [Console]::Error.WriteLine('missing command ' + $name); exit 6 }",
            "    foreach ($el in $cmd.CommandElements) {",
            "        if ($el -is [System.Management.Automation.Language.CommandParameterAst]) {",
            "            $p = $el.ParameterName",
            "            $known = @($info.Parameters.Keys | Where-Object { $_ -eq $p -or ($info.Parameters[$_].Aliases -contains $p) })",
            "            if ($known.Count -eq 0) { [Console]::Error.WriteLine('unknown parameter ' + $name + ' -' + $p); exit 7 }",
            "        }",
            "    }",
            "}",
            "if ($commands.Count -lt 3) { exit 8 }",
            "",
        ].join("\r\n");
        return PowerShellRunner(Exe, 60_000)(Checker);
    }

    it("the empty-set and non-empty scripts parse, and use only existing cmdlets and parameters", async () => {
        await CheckScript(EMPTY_SCRIPT);
        await CheckScript(TWO_SCRIPT);
        await CheckScript(BuildFirewallScript(["198.51.100.1", "203.0.113.200", "2001:db8:1::abcd"], "8777"));
    });

    it("the checker itself catches a wrong parameter", async () => {
        await assert.rejects(CheckScript("Set-NetFirewallRule -Name x -NoSuchParameter 1\r\nWrite-Output a\r\nWrite-Output b\r\n"), /unknown parameter Set-NetFirewallRule -NoSuchParameter/);
    });

    it("the runner feeds scripts through stdin and reports exit codes and errors", async () => {
        const Run = PowerShellRunner(Exe, 60_000);
        await Run("$x = 1 + 1\r\nif ($x -ne 2) { exit 9 }\r\n");
        await assert.rejects(Run("exit 3\r\n"), /exited with 3/);
        await assert.rejects(Run("$ErrorActionPreference = 'Stop'\r\nthrow 'broken on purpose'\r\n"), /broken on purpose/);
        // No command-line length limit: a 200 KB script (comments) runs fine.
        await Run("# " + "x".repeat(200_000) + "\r\nexit 0\r\n");
        assert.equal(typeof await IsElevated(Exe), "boolean");
    });
});

type Reply = { status: number; body: any };

function Call(Port: number, Method: string, Path: string, Body?: string, Headers: Record<string, string> = { "x-allowlist-secret": SECRET }): Promise<Reply> {
    return new Promise((resolve, reject) => {
        const Req = http.request({ host: "127.0.0.1", port: Port, method: Method, path: Path, headers: { "content-type": "application/json", ...Headers }, agent: false }, (Res) => {
            let Text = "";
            Res.on("data", (Chunk) => Text += Chunk);
            Res.on("end", () => resolve({ status: Res.statusCode ?? 0, body: Text === "" ? undefined : JSON.parse(Text) }));
        });
        Req.on("error", reject);
        Req.end(Body);
    });
}

function HelperConfig(Port: number, Dir: string, Change: Partial<AllowlistConfig> = {}): AllowlistConfig {
    return {
        bindHost: "127.0.0.1",
        port: Port,
        secret: SECRET,
        dryRun: true,
        allowPrivate: false,
        ttlMs: 600_000,
        minIntervalMs: 200,
        maxEntries: 256,
        ports: "8770-8777",
        auditLog: path.join(Dir, "audit.log"),
        stateFile: path.join(Dir, "state.json"),
        powershell: DefaultPowerShell(),
        firewall: "windows",
        nft: { binary: "/usr/sbin/nft", table: "reforged_revival", iface: undefined },
        ...Change,
    };
}

function AuditEvents(File: string): any[] {
    return fs.readFileSync(File, "utf8").split("\n").filter((Line) => Line !== "").map((Line) => JSON.parse(Line));
}

describe("allowlist helper over HTTP (DRY-RUN)", () => {
    let Dir: string;
    let Config: AllowlistConfig;
    let Helper: AllowlistHelper;
    let Clock = 1_800_000_000_000;
    let Scripts: string[] = [];
    let Log: CapturedLog;

    before(async () => {
        Log = CaptureLog();
        Dir = TempDir("allowlist");
        Config = HelperConfig(P.helper, Dir);
        const Audit = new AuditLog(Config.auditLog);
        const DryRun = DryRunRunner(Audit);
        Helper = new AllowlistHelper(Config, {
            audit: Audit,
            now: () => Clock,
            runner: async (Script) => {
                Scripts.push(Script);
                await DryRun(Script);
            },
        });
        await Helper.Start();
    });

    after(async () => {
        await Helper.Close();
        Log.stop();
    });

    it("disables the rule at start (nothing restored), logged as the exact dry-run script", async () => {
        await Helper.Sync.Settled();
        assert.deepEqual(Scripts, [EMPTY_SCRIPT]);
        const Events = AuditEvents(Config.auditLog);
        assert.equal(Events[0].event, "start");
        assert.equal(Events[0].dryRun, true);
        const DryRun = Events.find((Event) => Event.event === "dry_run");
        assert.equal(DryRun.script, EMPTY_SCRIPT);
        assert.equal(Events.find((Event) => Event.event === "rule_applied").enabled, false);
    });

    it("wants the secret, the right path and method, and a small JSON body", async () => {
        assert.equal((await Call(P.helper, "POST", "/allow", JSON.stringify({ ip: "203.0.113.7" }), {})).status, 401);
        assert.equal((await Call(P.helper, "POST", "/allow", JSON.stringify({ ip: "203.0.113.7" }), { "x-allowlist-secret": SECRET.slice(0, -1) + "x" })).status, 401);
        assert.equal((await Call(P.helper, "GET", "/status", undefined, {})).status, 401);
        assert.equal((await Call(P.helper, "POST", "/nope", "{}")).status, 404);
        assert.equal((await Call(P.helper, "GET", "/allow")).status, 405);
        assert.equal((await Call(P.helper, "POST", "/status", "{}")).status, 405);
        assert.equal((await Call(P.helper, "POST", "/allow", "{\"ip\":\"" + "1".repeat(2000) + "\"}")).status, 413);
        assert.deepEqual((await Call(P.helper, "POST", "/allow", "not json")).body, { error: "bad_request" });
        assert.equal(Helper.State.Size, 0);
        assert.ok(!fs.readFileSync(Config.auditLog, "utf8").includes(SECRET));
    });

    it("refuses ranges, garbage, 0.0.0.0, broadcast, multicast and private addresses with the reason", async () => {
        const Cases: [unknown, string][] = [
            ["0.0.0.0", "unspecified"], ["::", "unspecified"], ["0.0.0.0/0", "range_not_allowed"], ["203.0.113.0/24", "range_not_allowed"],
            ["203.0.113.1-203.0.113.5", "range_not_allowed"], ["Any", "range_not_allowed"], ["255.255.255.255", "reserved"], ["239.1.1.1", "multicast"],
            ["10.0.0.1", "private"], ["192.168.0.10", "private"], ["100.100.100.100", "private"], ["127.0.0.1", "private"], ["fe80::1", "private"],
            ["garbage", "not_an_ip"], ["203.0.113.7 ", "not_an_ip"], [123, "not_a_string"], [null, "not_a_string"],
        ];
        for(const [Given, Reason] of Cases){
            const Reply = await Call(P.helper, "POST", "/allow", JSON.stringify({ ip: Given }));
            assert.equal(Reply.status, 400, String(Given));
            assert.deepEqual(Reply.body, { error: "invalid_ip", reason: Reason }, String(Given));
        }
        assert.equal((await Call(P.helper, "POST", "/allow", "{}")).body.reason, "not_a_string");
        assert.equal(Helper.State.Size, 0);
        const Refused = AuditEvents(Config.auditLog).filter((Event) => Event.event === "refused");
        assert.ok(Refused.length >= Cases.length);
    });

    it("adds a public address, applies the rule with exactly that address, and refreshes it on repeat", async () => {
        const Before = Scripts.length;
        const First = await Call(P.helper, "POST", "/allow", JSON.stringify({ ip: "203.0.113.7" }));
        assert.equal(First.status, 200);
        assert.deepEqual(First.body, { ip: "203.0.113.7", added: true, ttlSeconds: 600 });
        const Again = await Call(P.helper, "POST", "/allow", JSON.stringify({ ip: "203.0.113.7" }));
        assert.deepEqual(Again.body, { ip: "203.0.113.7", added: false, ttlSeconds: 600 });
        await Helper.Sync.Settled();
        assert.equal(Scripts.length, Before + 1);
        assert.equal(Scripts[Scripts.length - 1], BuildFirewallScript(["203.0.113.7"], "8770-8777"));
        assert.match(Scripts[Scripts.length - 1], /-RemoteAddress \$Addresses -Profile Any -Enabled True/);
        assert.match(Scripts[Scripts.length - 1], /\$Addresses = @\('203\.0\.113\.7'\)/);
        const Events = AuditEvents(Config.auditLog);
        assert.ok(Events.some((Event) => Event.event === "added" && Event.ip === "203.0.113.7"));
        assert.equal(Events.filter((Event) => Event.event === "dry_run").pop().script, Scripts[Scripts.length - 1]);
    });

    it("debounces: addresses arriving together become one rule change, at least the interval after the last", async () => {
        await Sleep(250);
        const Before = Scripts.length;
        await Promise.all([
            Call(P.helper, "POST", "/allow", JSON.stringify({ ip: "198.51.100.2" })),
            Call(P.helper, "POST", "/allow", JSON.stringify({ ip: "2001:DB8::5" })),
        ]);
        await Helper.Sync.Settled();
        await Call(P.helper, "POST", "/allow", JSON.stringify({ ip: "8.8.8.8" }));
        await Helper.Sync.Settled();
        const New = Scripts.slice(Before);
        assert.ok(New.length <= 3 && New.length >= 2, `${New.length} rule changes`);
        assert.equal(New[New.length - 1], BuildFirewallScript(["8.8.8.8", "198.51.100.2", "203.0.113.7", "2001:db8::5"], "8770-8777"));
        const Applied = AuditEvents(Config.auditLog).filter((Event) => Event.event === "rule_applied");
        const Times = Applied.map((Event) => Date.parse(Event.t));
        for(let Index = 1; Index < Times.length; Index++){
            assert.ok(Times[Index] - Times[Index - 1] >= 150, "two rule changes closer than the interval");
        }
        const Status = await Call(P.helper, "GET", "/status");
        assert.equal(Status.status, 200);
        assert.deepEqual(Status.body.entries.map((Entry: any) => Entry.ip), ["8.8.8.8", "198.51.100.2", "203.0.113.7", "2001:db8::5"]);
        assert.equal(Status.body.dryRun, true);
        assert.equal(Status.body.lastApply.enabled, true);
        assert.equal(Status.body.lastApply.ok, true);
    });

    it("expires addresses 10 minutes after their last report and disables the rule when none are left", async () => {
        // Keep one address alive: reported again 9 minutes in.
        Clock += 9 * 60_000;
        await Call(P.helper, "POST", "/allow", JSON.stringify({ ip: "8.8.8.8" }));
        Clock += 60_000 + 1000;
        Helper.ExpireNow();
        await Helper.Sync.Settled();
        assert.deepEqual(Helper.State.List(), ["8.8.8.8"]);
        assert.equal(Scripts[Scripts.length - 1], BuildFirewallScript(["8.8.8.8"], "8770-8777"));
        const Expired = AuditEvents(Config.auditLog).filter((Event) => Event.event === "expired").map((Event) => Event.ip).sort();
        assert.deepEqual(Expired, ["198.51.100.2", "2001:db8::5", "203.0.113.7"]);

        Clock += 10 * 60_000;
        Helper.ExpireNow();
        await Helper.Sync.Settled();
        assert.equal(Helper.State.Size, 0);
        assert.equal(Scripts[Scripts.length - 1], EMPTY_SCRIPT);
    });
});

describe("allowlist helper lifecycle (DRY-RUN)", () => {
    let Log: CapturedLog;
    before(() => {
        Log = CaptureLog();
    });
    after(() => Log.stop());

    it("accepts private addresses only with ALLOWLIST_ALLOW_PRIVATE", async () => {
        const Dir = TempDir("allowlist-private");
        const Config = HelperConfig(P.helperPrivate, Dir, { allowPrivate: true });
        const Audit = new AuditLog(Config.auditLog);
        const Helper = new AllowlistHelper(Config, { audit: Audit, runner: DryRunRunner(Audit) });
        await Helper.Start();
        try{
            const Reply = await Call(P.helperPrivate, "POST", "/allow", JSON.stringify({ ip: "127.0.0.1" }));
            assert.equal(Reply.status, 200);
            assert.equal((await Call(P.helperPrivate, "POST", "/allow", JSON.stringify({ ip: "0.0.0.0" }))).status, 400);
            await Helper.Sync.Settled();
            assert.equal(AuditEvents(Config.auditLog).filter((Event) => Event.event === "dry_run").pop().script, BuildFirewallScript(["127.0.0.1"], "8770-8777"));
        }
        finally{
            await Helper.Close();
        }
    });

    it("on stop disables the rule and saves the set; on start restores it and applies it again", async () => {
        const Dir = TempDir("allowlist-restart");
        const Config = HelperConfig(P.helperRestart, Dir);
        const Audit = new AuditLog(Config.auditLog);
        const First = new AllowlistHelper(Config, { audit: Audit, runner: DryRunRunner(Audit) });
        await First.Start();
        await Call(P.helperRestart, "POST", "/allow", JSON.stringify({ ip: "203.0.113.50" }));
        await First.Sync.Settled();
        await First.Close();
        let Events = AuditEvents(Config.auditLog);
        assert.equal(Events.filter((Event) => Event.event === "dry_run").pop().script, EMPTY_SCRIPT);
        assert.equal(Events[Events.length - 1].event, "stop");
        const Saved = JSON.parse(fs.readFileSync(Config.stateFile, "utf8"));
        assert.deepEqual(Saved.entries.map((Entry: any) => Entry.ip), ["203.0.113.50"]);

        const Second = new AllowlistHelper(Config, { audit: Audit, runner: DryRunRunner(Audit) });
        await Second.Start();
        try{
            await Second.Sync.Settled();
            Events = AuditEvents(Config.auditLog);
            const Start = Events.filter((Event) => Event.event === "start").pop();
            assert.equal(Start.restored, 1);
            assert.equal(Events.filter((Event) => Event.event === "dry_run").pop().script, BuildFirewallScript(["203.0.113.50"], "8770-8777"));
        }
        finally{
            await Second.Close();
        }
    });
});
