import http from "node:http";
import { logger, LogThrottle } from "./log";

export type FeedOptions = {
    // http://127.0.0.1:61005 (the allowlist helper; loopback only)
    url: string;
    secret: string;
    // An address the helper accepted is not sent again for this long. The helper keeps an
    // entry for 10 minutes, and a playing client sends a heartbeat every 20 seconds.
    refreshMs: number;
    timeoutMs: number;
    additionalUrls?: string[];
};

// Tells the allowlist helper which addresses belong to logged-in players. Fire and forget: a
// player's request never waits for the helper, and a helper that is down only costs log lines.
export class AllowlistFeed {
    private readonly LastSent = new Map<string, number>();
    private readonly Pending = new Map<string, Promise<void>>();
    private readonly Agent = new http.Agent({ keepAlive: true, maxSockets: 4 });
    private readonly Warnings = new LogThrottle(60_000);
    private readonly Target: URL;
    private readonly Others: AllowlistFeed[];
    // Requests sent to the helper, for tests and the status log.
    public Sent = 0;

    constructor(private readonly Options: FeedOptions, private readonly Now: () => number = Date.now){
        this.Target = new URL("/allow", Options.url);
        this.Others = (Options.additionalUrls ?? []).map(url => new AllowlistFeed({ ...Options, url, additionalUrls: undefined }, Now));
    }

    Report(Ip: string){
        for (const Feed of this.Others) Feed.Report(Ip);
        if(Ip === "unknown"){
            return;
        }
        const Now = this.Now();
        const Last = this.LastSent.get(Ip);
        if(Last !== undefined && Now - Last < this.Options.refreshMs){
            return;
        }
        if(this.Pending.has(Ip)){
            return;
        }

        if(this.LastSent.size > 50_000){
            for(const [Key, At] of this.LastSent){
                if(Now - At >= this.Options.refreshMs){
                    this.LastSent.delete(Key);
                }
            }
        }

        const Work = this.Post(Ip).then((Status) => {
            if(Status === 200){
                this.LastSent.set(Ip, this.Now());
                logger.debug("allowlist refreshed", { ip: Ip });
                return;
            }
            if(Status === 400){
                // The helper refused this address (a private one, say). Asking again every
                // heartbeat would not change that, so wait like after a success.
                this.LastSent.set(Ip, this.Now());
                if(this.Warnings.Allow(`refused:${Ip}`) !== undefined){
                    logger.warn("allowlist helper refused a player's address; that player's game ports stay closed", { ip: Ip });
                }
                return;
            }
            this.WarnDown(`status ${Status}`);
        }).catch((error: Error) => {
            this.WarnDown(error.message);
        }).finally(() => {
            this.Pending.delete(Ip);
        });
        this.Pending.set(Ip, Work);
    }

    private WarnDown(Detail: string){
        const Suppressed = this.Warnings.Allow("down");
        if(Suppressed !== undefined){
            logger.warn("allowlist helper did not take an address; players who log in now get no game ports until it answers", { detail: Detail, suppressed: Suppressed });
        }
    }

    private Post(Ip: string): Promise<number> {
        const Body = JSON.stringify({ ip: Ip });
        this.Sent++;
        return new Promise((resolve, reject) => {
            const Req = http.request(this.Target, {
                method: "POST",
                agent: this.Agent,
                headers: {
                    "content-type": "application/json",
                    "content-length": Buffer.byteLength(Body),
                    "x-allowlist-secret": this.Options.secret,
                },
            }, (Res) => {
                Res.resume();
                Res.on("end", () => resolve(Res.statusCode ?? 0));
                Res.on("error", reject);
            });
            Req.setTimeout(this.Options.timeoutMs, () => Req.destroy(new Error("timed out")));
            Req.on("error", reject);
            Req.end(Body);
        });
    }

    // Resolves when nothing is on its way to the helper (tests and shutdown).
    async Idle(){
        await Promise.all(this.Others.map(Feed => Feed.Idle()));
        while(this.Pending.size > 0){
            await Promise.all([...this.Pending.values()]);
        }
    }

    Close(){
        for (const Feed of this.Others) Feed.Close();
        this.Agent.destroy();
    }
}
