import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { CheckAllowlistIp, ParseIp } from "../ip";
import { logger, LogThrottle } from "../log";
import { AllowlistConfig } from "./config";
import { BuildScriptFor, ScriptRunner } from "./firewall";
import { AllowlistState } from "./state";
import { ApplyResult, FirewallSync } from "./sync";

const MAX_BODY = 1024;

// Append-only JSON lines: every address added or expired, every refusal, every rule change
// (with the exact script in dry-run mode) and every failed secret. Never the secret itself.
export class AuditLog {
    constructor(private readonly File: string | undefined){
        if(File !== undefined){
            fs.mkdirSync(path.dirname(File), { recursive: true });
        }
    }

    Write(Event: string, Fields: Record<string, unknown> = {}){
        const Line = JSON.stringify({ t: new Date().toISOString(), event: Event, ...Fields });
        if(this.File !== undefined){
            try{
                fs.appendFileSync(this.File, Line + "\n");
            }
            catch(error){
                logger.error("audit log write failed", { file: this.File, error: error instanceof Error ? error.message : String(error) });
            }
        }
        logger.info(`allowlist ${Event}`, Fields);
    }
}

export function DryRunRunner(Audit: AuditLog): ScriptRunner {
    return async (Script: string) => {
        Audit.Write("dry_run", { script: Script });
    };
}

function SecretMatches(Given: string | string[] | undefined, Expected: string): boolean {
    if(typeof Given !== "string"){
        return false;
    }
    // Hashing first gives equal lengths, so the comparison time says nothing about the secret.
    const A = crypto.createHash("sha256").update(Given).digest();
    const B = crypto.createHash("sha256").update(Expected).digest();
    return crypto.timingSafeEqual(A, B);
}

function IsLoopback(Address: string | undefined): boolean {
    const Parsed = ParseIp(Address);
    return Parsed !== undefined && Parsed.class === "loopback";
}

function Json(Res: http.ServerResponse, Status: number, Body: unknown, Close = false){
    const Text = JSON.stringify(Body);
    Res.writeHead(Status, {
        "content-type": "application/json",
        "content-length": Buffer.byteLength(Text),
        "cache-control": "no-store",
        ...(Close ? { connection: "close" } : {}),
    });
    Res.end(Text);
}

export type HelperDeps = {
    runner: ScriptRunner;
    audit: AuditLog;
    now?: () => number;
};

export class AllowlistHelper {
    readonly Server: http.Server;
    readonly State: AllowlistState;
    readonly Sync: FirewallSync;
    private readonly Now: () => number;
    private readonly Audit: AuditLog;
    private readonly Throttle = new LogThrottle(60_000);
    private ExpiryTimer: NodeJS.Timeout | undefined;
    private SaveTimer: NodeJS.Timeout | undefined;
    private Dirty = false;

    constructor(private readonly Config: AllowlistConfig, Deps: HelperDeps){
        this.Now = Deps.now ?? Date.now;
        this.Audit = Deps.audit;
        this.State = new AllowlistState(Config.ttlMs, Config.maxEntries);

        const Runner = Deps.runner;
        // The interval between rule changes is real time, whatever clock the entries use.
        this.Sync = new FirewallSync(
            (Ips) => Runner(BuildScriptFor(Config, Ips)),
            Config.minIntervalMs,
            (Result) => this.OnApplied(Result),
        );

        this.Server = http.createServer({ headersTimeout: 5_000, requestTimeout: 10_000, keepAliveTimeout: 30_000 }, (Req, Res) => this.Handle(Req, Res));
        this.Server.maxConnections = 64;
    }

    private OnApplied(Result: ApplyResult){
        if(Result.ok){
            this.Audit.Write("rule_applied", { enabled: Result.ips.length > 0, ips: Result.ips, dryRun: this.Config.dryRun, ms: Result.ms });
        }
        else{
            this.Audit.Write("rule_failed", { enabled: Result.ips.length > 0, ips: Result.ips, error: Result.error });
        }
    }

    // Restores unexpired entries from the state file, then brings the rule to that set (an
    // empty set disables it): the rule never keeps addresses from before a crash.
    async Start(){
        let Restored = 0;
        try{
            const Saved = JSON.parse(fs.readFileSync(this.Config.stateFile, "utf8"));
            Restored = this.State.Load(Saved?.entries, this.Now(), (Ip) => CheckAllowlistIp(Ip.canonical, this.Config.allowPrivate).ok);
        }
        catch(error){
            if((error as NodeJS.ErrnoException).code !== "ENOENT"){
                logger.warn("allowlist state file unreadable; starting empty", { file: this.Config.stateFile, error: error instanceof Error ? error.message : String(error) });
            }
        }
        this.Audit.Write("start", { dryRun: this.Config.dryRun, restored: Restored, ttlSeconds: this.Config.ttlMs / 1000, ports: this.Config.ports, allowPrivate: this.Config.allowPrivate });
        this.Sync.Request(this.State.List());
        this.ScheduleExpiry();
        this.SaveTimer = setInterval(() => this.SaveIfDirty(), 30_000);
        this.SaveTimer.unref();

        await new Promise<void>((resolve, reject) => {
            const OnError = (error: Error) => reject(error);
            this.Server.once("error", OnError);
            this.Server.listen(this.Config.port, this.Config.bindHost, () => {
                this.Server.off("error", OnError);
                resolve();
            });
        });
    }

    // Stops listening, disables the rule (fail closed) and saves the live set, so a restart
    // re-opens the ports for the same players right away.
    async Close(){
        if(this.ExpiryTimer !== undefined){
            clearTimeout(this.ExpiryTimer);
        }
        if(this.SaveTimer !== undefined){
            clearInterval(this.SaveTimer);
        }
        await new Promise<void>((resolve) => {
            this.Server.close(() => resolve());
            this.Server.closeAllConnections();
        });
        this.Dirty = true;
        this.SaveIfDirty();
        await this.Sync.Final([]);
        this.Audit.Write("stop", { saved: this.State.Size });
    }

    private SaveIfDirty(){
        if(!this.Dirty){
            return;
        }
        this.Dirty = false;
        try{
            const Temp = this.Config.stateFile + ".tmp";
            fs.writeFileSync(Temp, JSON.stringify({ version: 1, entries: this.State.Snapshot() }));
            fs.renameSync(Temp, this.Config.stateFile);
        }
        catch(error){
            logger.warn("allowlist state file not saved", { file: this.Config.stateFile, error: error instanceof Error ? error.message : String(error) });
        }
    }

    private ScheduleExpiry(){
        if(this.ExpiryTimer !== undefined){
            clearTimeout(this.ExpiryTimer);
            this.ExpiryTimer = undefined;
        }
        const Next = this.State.NextExpiry();
        if(Next === undefined){
            return;
        }
        this.ExpiryTimer = setTimeout(() => {
            this.ExpiryTimer = undefined;
            this.ExpireNow();
        }, Math.max(0, Next - this.Now()) + 5);
        this.ExpiryTimer.unref();
    }

    // Also called by tests with a moved clock.
    ExpireNow(){
        const Gone = this.State.Expire(this.Now());
        for(const Ip of Gone){
            this.Audit.Write("expired", { ip: Ip });
        }
        if(Gone.length > 0){
            this.Dirty = true;
            this.SaveIfDirty();
            this.Sync.Request(this.State.List());
        }
        this.ScheduleExpiry();
    }

    Status(){
        return {
            dryRun: this.Config.dryRun,
            allowPrivate: this.Config.allowPrivate,
            ttlSeconds: this.Config.ttlMs / 1000,
            ports: this.Config.ports,
            entries: this.State.Snapshot().map((Entry) => ({ ip: Entry.ip, expiresAt: new Date(Entry.expiresAt).toISOString() })),
            pending: this.Sync.Pending,
            lastApply: this.Sync.LastResult === undefined ? null : {
                ok: this.Sync.LastResult.ok,
                enabled: this.Sync.LastResult.ips.length > 0,
                ips: this.Sync.LastResult.ips,
                at: new Date(this.Sync.LastResult.at).toISOString(),
                ...(this.Sync.LastResult.ok ? {} : { error: this.Sync.LastResult.error }),
            },
        };
    }

    private Handle(Req: http.IncomingMessage, Res: http.ServerResponse){
        // Defence in depth: the bind address is loopback already.
        if(!IsLoopback(Req.socket.remoteAddress)){
            Json(Res, 403, { error: "forbidden" }, true);
            return;
        }

        const Path = (Req.url ?? "").split("?")[0];
        if(Path !== "/allow" && Path !== "/status"){
            Json(Res, 404, { error: "not_found" });
            return;
        }
        const Method = Path === "/allow" ? "POST" : "GET";
        if(Req.method !== Method){
            Res.setHeader("allow", Method);
            Json(Res, 405, { error: "method_not_allowed" });
            return;
        }

        if(!SecretMatches(Req.headers["x-allowlist-secret"], this.Config.secret)){
            const Suppressed = this.Throttle.Allow("unauthorized");
            if(Suppressed !== undefined){
                this.Audit.Write("unauthorized", { path: Path, suppressed: Suppressed });
            }
            Json(Res, 401, { error: "unauthorized" }, true);
            return;
        }

        if(Path === "/status"){
            Json(Res, 200, this.Status());
            return;
        }

        const Declared = Number(Req.headers["content-length"] ?? 0);
        if(Declared > MAX_BODY){
            Json(Res, 413, { error: "body_too_large" }, true);
            return;
        }

        const Chunks: Buffer[] = [];
        let Size = 0;
        let TooLarge = false;
        Req.on("data", (Chunk: Buffer) => {
            Size += Chunk.length;
            if(Size > MAX_BODY){
                TooLarge = true;
                return;
            }
            Chunks.push(Chunk);
        });
        Req.on("end", () => {
            if(TooLarge){
                Json(Res, 413, { error: "body_too_large" }, true);
                return;
            }
            let Body: unknown;
            try{
                Body = JSON.parse(Buffer.concat(Chunks).toString("utf8"));
            }
            catch{
                Json(Res, 400, { error: "bad_request" });
                return;
            }
            const Given = Body !== null && typeof Body === "object" ? (Body as any).ip : undefined;
            this.Allow(Given, Res);
        });
        Req.on("error", () => Res.destroy());
    }

    private Allow(Given: unknown, Res: http.ServerResponse){
        const Check = CheckAllowlistIp(Given, this.Config.allowPrivate);
        if(!Check.ok){
            const Shown = typeof Given === "string" ? Given.slice(0, 64).replace(/[^\x20-\x7e]/g, "?") : typeof Given;
            const Suppressed = this.Throttle.Allow(`refused:${Check.reason}:${Shown}`);
            if(Suppressed !== undefined){
                this.Audit.Write("refused", { value: Shown, reason: Check.reason, suppressed: Suppressed });
            }
            Json(Res, 400, { error: "invalid_ip", reason: Check.reason });
            return;
        }

        const Result = this.State.Refresh(Check.ip, this.Now());
        if(Result === "full"){
            const Suppressed = this.Throttle.Allow("full");
            if(Suppressed !== undefined){
                this.Audit.Write("full", { ip: Check.ip.canonical, max: this.Config.maxEntries, suppressed: Suppressed });
            }
            Json(Res, 503, { error: "allowlist_full" });
            return;
        }

        this.Dirty = true;
        if(Result === "added"){
            this.Audit.Write("added", { ip: Check.ip.canonical });
            this.SaveIfDirty();
            this.Sync.Request(this.State.List());
            this.ScheduleExpiry();
        }
        else if(this.ExpiryTimer === undefined){
            this.ScheduleExpiry();
        }

        Json(Res, 200, { ip: Check.ip.canonical, added: Result === "added", ttlSeconds: this.Config.ttlMs / 1000 });
    }
}
