// Download slots. Each account may have a few files streaming at once (the launcher downloads several
// in parallel), and the whole server has a ceiling so a crowd of first-time downloads can't starve the
// host's upload for the players already in a hunt.

export type AcquireResult =
    | { ok: true; release: () => void }
    | { ok: false; reason: "account_limit" | "total_limit" };

export class StreamLimiter {
    private readonly PerAccount: number;
    private readonly Total: number;
    private readonly Active = new Map<string, number>();
    private ActiveTotal = 0;

    constructor(PerAccount: number, Total: number){
        this.PerAccount = PerAccount;
        this.Total = Total;
    }

    active(Account?: string): number {
        return Account === undefined ? this.ActiveTotal : (this.Active.get(Account) ?? 0);
    }

    tryAcquire(Account: string): AcquireResult {
        const Current = this.Active.get(Account) ?? 0;

        if(Current >= this.PerAccount){
            return { ok: false, reason: "account_limit" };
        }

        if(this.ActiveTotal >= this.Total){
            return { ok: false, reason: "total_limit" };
        }

        this.Active.set(Account, Current + 1);
        this.ActiveTotal++;

        let Released = false;
        return {
            ok: true,
            release: () => {
                if(Released){
                    return;
                }
                Released = true;

                const Now = (this.Active.get(Account) ?? 1) - 1;
                if(Now <= 0){
                    this.Active.delete(Account);
                }
                else{
                    this.Active.set(Account, Now);
                }
                this.ActiveTotal--;
            },
        };
    }
}
