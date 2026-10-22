/*
 * Reforged patch 0025: who may join a City (Ramsgate / dojo) chat room.
 *
 * The client names a hub's room City-<gameSessionId>, from serverInfo.gameSessionId in the
 * /candidate/status answer that sent it to that hub. The backend therefore knows exactly which
 * accounts were sent to which hub: an account may join City-<id> when the last hub session it was
 * sent to is <id>. This replaces the insecure dev switch (any account, any City room), which
 * production must keep off, so Ramsgate chat, quick chat and emojis (all sent to the City room)
 * were refused for everyone.
 */

const HUB_SESSION_TTL_MS = 7 * 24 * 60 * 60 * 1000;
const MAX_ACCOUNTS = 50_000;

const lastHubSessionByAccount = new Map<string, { gameSessionId: string; at: number }>();

/** Called when matchmaking tells an account to travel to a hub with this game session id. */
export function RecordHubSessionServed(accountId: string, gameSessionId: string, now = Date.now()): void {
    if (!accountId || !gameSessionId) return;
    lastHubSessionByAccount.delete(accountId);
    lastHubSessionByAccount.set(accountId, { gameSessionId, at: now });
    while (lastHubSessionByAccount.size > MAX_ACCOUNTS) {
        const oldest = lastHubSessionByAccount.keys().next().value;
        if (oldest === undefined) break;
        lastHubSessionByAccount.delete(oldest);
    }
}

/** True when roomLocal is City-<id> and <id> is the hub session this account was last sent to. */
export function MayJoinCityRoom(accountId: string, roomLocal: string, now = Date.now()): boolean {
    if (!roomLocal.startsWith("City-")) return false;
    const entry = lastHubSessionByAccount.get(accountId);
    if (!entry || now - entry.at > HUB_SESSION_TTL_MS) return false;
    return roomLocal.slice("City-".length) === entry.gameSessionId;
}

/** Tests only. */
export function ResetCityRoomsForTest(): void {
    lastHubSessionByAccount.clear();
}
