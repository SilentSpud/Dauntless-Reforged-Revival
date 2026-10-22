// Brings the firewall rule to the desired address set, never more often than once per
// MinIntervalMs: the first change after a quiet period applies at once, and changes that arrive
// while a run is in progress or too soon after one are folded into a single later run.
// A failed run is retried with backoff (MinIntervalMs doubling, at most a minute).

export type ApplyResult = { ok: true; ips: string[]; at: number; ms: number } | { ok: false; ips: string[]; at: number; ms: number; error: string };

function Same(A: readonly string[], B: readonly string[] | undefined): boolean {
    return B !== undefined && A.length === B.length && A.every((Value, Index) => Value === B[Index]);
}

export class FirewallSync {
    private Desired: string[] = [];
    // undefined: not known (at start, and after a failed run), so the next run always applies.
    private Applied: string[] | undefined;
    private Timer: NodeJS.Timeout | undefined;
    private Running: Promise<void> | undefined;
    private LastStart = Number.NEGATIVE_INFINITY;
    private Failures = 0;
    private Stopped = false;
    public Runs = 0;
    public LastResult: ApplyResult | undefined;

    constructor(
        private readonly Apply: (Ips: readonly string[]) => Promise<void>,
        private readonly MinIntervalMs: number,
        private readonly OnResult: (Result: ApplyResult) => void = () => undefined,
        private readonly Now: () => number = Date.now,
    ){}

    Request(Ips: readonly string[]){
        this.Desired = [...Ips];
        this.Schedule(0);
    }

    get Pending(): boolean {
        return this.Timer !== undefined || this.Running !== undefined || !Same(this.Desired, this.Applied);
    }

    private Schedule(Backoff: number){
        if(this.Stopped || this.Timer !== undefined || this.Running !== undefined || Same(this.Desired, this.Applied)){
            return;
        }
        const Delay = Math.max(0, Backoff, this.LastStart + this.MinIntervalMs - this.Now());
        this.Timer = setTimeout(() => {
            this.Timer = undefined;
            this.Running = this.Run().finally(() => {
                this.Running = undefined;
                const Backoff = this.Failures === 0 ? 0 : Math.min(60_000, this.MinIntervalMs * 2 ** (this.Failures - 1));
                this.Schedule(Backoff);
            });
        }, Delay);
    }

    private async Run(){
        if(Same(this.Desired, this.Applied)){
            return;
        }
        const Snapshot = [...this.Desired];
        this.LastStart = this.Now();
        this.Runs++;
        const Started = Date.now();
        try{
            await this.Apply(Snapshot);
            this.Applied = Snapshot;
            this.Failures = 0;
            this.LastResult = { ok: true, ips: Snapshot, at: this.LastStart, ms: Date.now() - Started };
        }
        catch(error){
            this.Applied = undefined;
            this.Failures++;
            this.LastResult = { ok: false, ips: Snapshot, at: this.LastStart, ms: Date.now() - Started, error: error instanceof Error ? error.message : String(error) };
        }
        this.OnResult(this.LastResult);
    }

    // Resolves once the rule matches the desired set (or the sync was stopped).
    async Settled(TimeoutMs = 60_000){
        const Until = Date.now() + TimeoutMs;
        while(this.Pending && !this.Stopped && Date.now() < Until){
            if(this.Running !== undefined){
                await this.Running;
            }
            else{
                await new Promise((resolve) => setTimeout(resolve, 10));
            }
        }
    }

    // Shutdown: waits for a run in progress, then applies Ips once more right away (ignoring
    // the interval) and stops scheduling.
    async Final(Ips: readonly string[]){
        this.Stopped = true;
        if(this.Timer !== undefined){
            clearTimeout(this.Timer);
            this.Timer = undefined;
        }
        if(this.Running !== undefined){
            await this.Running;
        }
        this.Desired = [...Ips];
        if(!Same(this.Desired, this.Applied)){
            await this.Run();
        }
    }
}
