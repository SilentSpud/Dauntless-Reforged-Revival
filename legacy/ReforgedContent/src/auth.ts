import { createHash } from "node:crypto";

// Account check for downloads (from Dauntless Revived's content server). Every runtime file request
// carries the account key (x-reforged-user-api-key),
// and we ask the backend who it belongs to (GET /reforged/api/GetUserInfo). Answers are cached for a
// few minutes, keyed by the SHA-256 of the key: the key itself is never stored, cached or logged here.

export type AccountInfo = {
    UserId: string;
    Username: string;
    IsAdmin: boolean;
};

export type LookupResult =
    | { status: "ok"; account: AccountInfo }
    | { status: "invalid" }
    | { status: "unavailable"; reason: string };

export type KeyLookup = (Key: string) => Promise<LookupResult>;

export type AuthCacheOptions = {
    lookup: KeyLookup;
    ttlMs?: number;          // good keys (default 5 minutes)
    negativeTtlMs?: number;  // refused keys (default 30 seconds), so a bad key can't hammer the backend
    maxEntries?: number;     // oldest entries are dropped past this
    now?: () => number;
};

type Entry = {
    result: LookupResult;
    expires: number;
};

export function HashKey(Key: string): string {
    return createHash("sha256").update(Key, "utf8").digest("hex");
}

// Printable ASCII without spaces, at most 256 characters. Anything else can't be a key and is refused
// without asking the backend (and can't be smuggled into the header we send it).
export function IsPlausibleKey(Key: unknown): Key is string {
    return typeof Key === "string" && /^[\x21-\x7e]{1,256}$/.test(Key);
}

export class AuthCache {
    private readonly Lookup: KeyLookup;
    private readonly TtlMs: number;
    private readonly NegativeTtlMs: number;
    private readonly MaxEntries: number;
    private readonly Now: () => number;
    private readonly Entries = new Map<string, Entry>();
    private readonly InFlight = new Map<string, Promise<LookupResult>>();

    constructor(Options: AuthCacheOptions){
        this.Lookup = Options.lookup;
        this.TtlMs = Options.ttlMs ?? 5 * 60 * 1000;
        this.NegativeTtlMs = Options.negativeTtlMs ?? 30 * 1000;
        this.MaxEntries = Options.maxEntries ?? 2000;
        this.Now = Options.now ?? Date.now;
    }

    get size(): number {
        return this.Entries.size;
    }

    // True if this key hash is cached (for tests; takes the hash, never the key).
    hasHash(KeyHash: string): boolean {
        return this.Entries.has(KeyHash);
    }

    async check(Key: unknown): Promise<LookupResult> {
        if(!IsPlausibleKey(Key)){
            return { status: "invalid" };
        }

        const KeyHash = HashKey(Key);
        const Now = this.Now();

        const Cached = this.Entries.get(KeyHash);
        if(Cached !== undefined){
            if(Cached.expires > Now){
                return Cached.result;
            }
            this.Entries.delete(KeyHash);
        }

        // Several downloads start at once with the same key: ask the backend once.
        const Pending = this.InFlight.get(KeyHash);
        if(Pending !== undefined){
            return Pending;
        }

        const Promised = this.Lookup(Key)
            .catch((error: unknown): LookupResult => ({ status: "unavailable", reason: error instanceof Error ? error.message : String(error) }))
            .then((Result) => {
                // A backend that is down or confused is not the account's fault: don't cache that.
                if(Result.status !== "unavailable"){
                    this.Store(KeyHash, Result);
                }
                return Result;
            })
            .finally(() => {
                this.InFlight.delete(KeyHash);
            });

        this.InFlight.set(KeyHash, Promised);
        return Promised;
    }

    private Store(KeyHash: string, Result: LookupResult){
        const Ttl = Result.status === "ok" ? this.TtlMs : this.NegativeTtlMs;

        this.Entries.delete(KeyHash);
        this.Entries.set(KeyHash, { result: Result, expires: this.Now() + Ttl });

        while(this.Entries.size > this.MaxEntries){
            const Oldest = this.Entries.keys().next().value;
            if(Oldest === undefined){
                break;
            }
            this.Entries.delete(Oldest);
        }
    }
}

// The real lookup: GET <BACKEND_URL>/reforged/api/GetUserInfo with the key in its header.
// 200 with a UserId is an account, 401/403 is a refused key, anything else means the backend
// can't answer right now.
export function BackendLookup(BackendUrl: string, TimeoutMs = 5000): KeyLookup {
    const Url = `${BackendUrl.replace(/\/+$/, "")}/reforged/api/GetUserInfo`;

    return async (Key: string): Promise<LookupResult> => {
        let Response: Response;
        try{
            Response = await fetch(Url, {
                method: "GET",
                headers: { "x-reforged-user-api-key": Key, "accept": "application/json" },
                signal: AbortSignal.timeout(TimeoutMs),
                redirect: "error",
            });
        }
        catch(error){
            return { status: "unavailable", reason: `backend unreachable: ${error instanceof Error ? error.message : String(error)}` };
        }

        if(Response.status === 401 || Response.status === 403){
            await Response.body?.cancel().catch(() => undefined);
            return { status: "invalid" };
        }

        if(Response.status !== 200){
            await Response.body?.cancel().catch(() => undefined);
            return { status: "unavailable", reason: `backend answered ${Response.status}` };
        }

        let Body: unknown;
        try{
            Body = await Response.json();
        }
        catch{
            return { status: "unavailable", reason: "backend answered with bad JSON" };
        }

        const Info = Body as Record<string, unknown> | null;
        if(Info === null || typeof Info !== "object" || typeof Info.UserId !== "string" || Info.UserId.length === 0){
            return { status: "unavailable", reason: "backend answer has no UserId" };
        }

        return {
            status: "ok",
            account: {
                UserId: Info.UserId,
                Username: typeof Info.Username === "string" ? Info.Username : "",
                IsAdmin: Info.IsAdmin === true,
            },
        };
    };
}
