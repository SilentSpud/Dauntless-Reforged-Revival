// Token buckets per key (an IPv4 address or an IPv6 /64). A bucket holds up to `burst` requests
// and refills at `perMinute`. Full buckets are forgotten, so memory only holds recently busy peers.

export type BucketSpec = {
    burst: number;
    perMinute: number;
};

export type TakeResult = { ok: true } | { ok: false; retryAfterSeconds: number };

type Bucket = { tokens: number; at: number };

export class RateLimiter {
    private readonly Buckets = new Map<string, Bucket>();
    private readonly PerMs: number;

    constructor(readonly Spec: BucketSpec, private readonly MaxKeys = 100_000){
        this.PerMs = Spec.perMinute / 60_000;
    }

    private Refilled(Entry: Bucket, Now: number): number {
        return Math.min(this.Spec.burst, Entry.tokens + Math.max(0, Now - Entry.at) * this.PerMs);
    }

    Take(Key: string, Now = Date.now()): TakeResult {
        let Entry = this.Buckets.get(Key);
        if(Entry === undefined){
            if(this.Buckets.size >= this.MaxKeys){
                this.Sweep(Now);
                if(this.Buckets.size >= this.MaxKeys){
                    // Still full: forget the oldest peer rather than grow without bound.
                    const Oldest = this.Buckets.keys().next().value;
                    if(Oldest !== undefined){
                        this.Buckets.delete(Oldest);
                    }
                }
            }
            Entry = { tokens: this.Spec.burst, at: Now };
            this.Buckets.set(Key, Entry);
        }
        else{
            Entry.tokens = this.Refilled(Entry, Now);
            Entry.at = Now;
        }

        if(Entry.tokens >= 1){
            Entry.tokens -= 1;
            return { ok: true };
        }

        const Missing = 1 - Entry.tokens;
        // (The small epsilon keeps floating-point dust from adding a second.)
        const Seconds = this.PerMs > 0 ? Math.ceil(Missing / this.PerMs / 1000 - 1e-6) : 3600;
        return { ok: false, retryAfterSeconds: Math.max(1, Seconds) };
    }

    // Drops buckets that have refilled completely: they behave exactly like a new one.
    Sweep(Now = Date.now()){
        for(const [Key, Entry] of this.Buckets){
            if(this.Refilled(Entry, Now) >= this.Spec.burst){
                this.Buckets.delete(Key);
            }
        }
    }

    get Size(): number {
        return this.Buckets.size;
    }
}

// "<burst>,<per minute>", for example "300,180". Decimals are fine for slow refills ("5,0.2" is
// one request every five minutes after a burst of five).
export function ParseBucketSpec(Text: string, Name: string): BucketSpec {
    const Match = /^\s*(\d+)\s*,\s*(\d+(?:\.\d+)?)\s*$/.exec(Text);
    if(Match === null){
        throw new Error(`${Name} must look like "<burst>,<per minute>", for example "300,180"`);
    }
    const Spec = { burst: Number(Match[1]), perMinute: Number(Match[2]) };
    if(Spec.burst < 1 || Spec.burst > 1_000_000 || Spec.perMinute > 1_000_000){
        throw new Error(`${Name}: burst must be 1 to 1000000 and the rate at most 1000000 per minute`);
    }
    return Spec;
}
