import { ParseIp, ParsedIp } from "../ip";

export type RefreshResult = "added" | "refreshed" | "full";

type Entry = { ip: ParsedIp; expiresAt: number };

// The live set: addresses of players seen logging in or sending heartbeats, each kept for TTL
// after it was last reported. Callers validate addresses first (CheckAllowlistIp).
export class AllowlistState {
    private readonly Entries = new Map<string, Entry>();

    constructor(private readonly TtlMs: number, private readonly MaxEntries: number){}

    Refresh(Ip: ParsedIp, Now: number): RefreshResult {
        const Existing = this.Entries.get(Ip.canonical);
        if(Existing !== undefined){
            Existing.expiresAt = Now + this.TtlMs;
            return "refreshed";
        }
        if(this.Entries.size >= this.MaxEntries){
            return "full";
        }
        this.Entries.set(Ip.canonical, { ip: Ip, expiresAt: Now + this.TtlMs });
        return "added";
    }

    // Removes and returns the addresses whose time is up.
    Expire(Now: number): string[] {
        const Gone: string[] = [];
        for(const [Key, Entry] of this.Entries){
            if(Entry.expiresAt <= Now){
                this.Entries.delete(Key);
                Gone.push(Key);
            }
        }
        return Gone;
    }

    NextExpiry(): number | undefined {
        let Next: number | undefined;
        for(const Entry of this.Entries.values()){
            if(Next === undefined || Entry.expiresAt < Next){
                Next = Entry.expiresAt;
            }
        }
        return Next;
    }

    // Sorted IPv4 first, then IPv6, each by numeric value: the same set always gives the same rule.
    List(): string[] {
        return [...this.Entries.values()]
            .sort((A, B) => {
                if(A.ip.version !== B.ip.version){
                    return A.ip.version - B.ip.version;
                }
                for(let Index = 0; Index < A.ip.bytes.length; Index++){
                    if(A.ip.bytes[Index] !== B.ip.bytes[Index]){
                        return A.ip.bytes[Index] - B.ip.bytes[Index];
                    }
                }
                return 0;
            })
            .map((Entry) => Entry.ip.canonical);
    }

    Snapshot(): { ip: string; expiresAt: number }[] {
        return this.List().map((Ip) => ({ ip: Ip, expiresAt: this.Entries.get(Ip)!.expiresAt }));
    }

    // Restores a saved snapshot; entries that are gone, malformed or not allowed are skipped.
    Load(Saved: unknown, Now: number, Accept: (Ip: ParsedIp) => boolean): number {
        if(!Array.isArray(Saved)){
            return 0;
        }
        let Loaded = 0;
        for(const Item of Saved){
            if(Item === null || typeof Item !== "object"){
                continue;
            }
            const Ip = ParseIp((Item as any).ip);
            const ExpiresAt = (Item as any).expiresAt;
            if(Ip === undefined || !Accept(Ip) || typeof ExpiresAt !== "number" || !Number.isFinite(ExpiresAt) || ExpiresAt <= Now){
                continue;
            }
            if(this.Entries.size >= this.MaxEntries){
                break;
            }
            // Never longer than one TTL from now, whatever the file says.
            this.Entries.set(Ip.canonical, { ip: Ip, expiresAt: Math.min(ExpiresAt, Now + this.TtlMs) });
            Loaded++;
        }
        return Loaded;
    }

    get Size(): number {
        return this.Entries.size;
    }
}
