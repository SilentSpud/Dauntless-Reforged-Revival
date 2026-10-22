/*
 * [Dauntless Reforged Revival] XP and reward multipliers for test servers.
 *
 * A private server (friends over Tailscale) can multiply the XP and the loot its game servers grant, so a tester
 * reaches late-game content quickly. They are off unless the host kit turns them on, and they never act on a public
 * server: the host kit only writes them for --mode private, and this module checks again that the backend was set
 * up as a private server on a Tailscale address before it multiplies anything.
 *
 *   REFORGED_SERVER_MODE        private | public (written by ReforgedHost)
 *   REFORGED_SERVER_HOST        the invite's host: a Tailscale IPv4 (100.64.0.0/10) or a *.ts.net name
 *   REFORGED_XP_MULTIPLIER      1 to 100, default 1 (off)
 *   REFORGED_REWARD_MULTIPLIER  1 to 100, default 1 (off)
 *
 * What is multiplied:
 *   - XP: every game-server grant on POST /progression/<user>/<track>/<amount> (Slayer level, weapons, Hunt Pass...),
 *     capped at the grant route's own maximum.
 *   - Rewards: the stacked items a game server ADDS in a reward transaction (its `source` is a hunt, a part break, a
 *     gathering node, a patrol chest or a quest), and only for currencies and materials. Unlock tokens, cosmetics,
 *     equipment, season coins (funded by a claimed challenge) and prestige-gated rewards are never touched, nor is
 *     anything a crafting or a purchase removes.
 */

import net from "node:net";

export const MAX_TEST_MULTIPLIER = 100;

// The game's own `source` strings for reward transactions (seen in [GrantMon] lines of a 1.14.7 server).
const REWARD_SOURCES = new Set(["PlayerLoot", "MISC", "patrol_chest", "quest"]);
const REWARD_SOURCE_PREFIXES = ["BUNDLE_GATHER_"];

// Currencies and materials. Everything else (TOKEN_*, EM_*, BNC_*, DYE_*, TITLE_*, ...) keeps its quantity.
const MULTIPLIED_ITEM_PREFIXES = ["CURRENCY_NOTES", "CURRENCY_PJM_", "BREAK_", "GATHER_"];

export interface TestMultipliers {
    xp: number;
    rewards: number;
    // Why they are off when they are, for the start-up log line.
    reason?: string;
}

export function IsTailscaleHost(host: string): boolean {
    const Host = host.trim().toLowerCase();
    if (net.isIPv4(Host)) {
        const [A, B] = Host.split(".").map(Number);
        return A === 100 && B >= 64 && B <= 127;
    }
    return /^[a-z0-9-]+(\.[a-z0-9-]+)*\.ts\.net$/.test(Host);
}

function ParseMultiplier(raw: string | undefined): number | undefined {
    if (raw === undefined || raw.trim() === "") return 1;
    const Value = Number(raw);
    if (!Number.isFinite(Value) || Value < 1 || Value > MAX_TEST_MULTIPLIER) return undefined;
    return Value;
}

export function ReadTestMultipliers(env: NodeJS.ProcessEnv = process.env): TestMultipliers {
    const Xp = ParseMultiplier(env.REFORGED_XP_MULTIPLIER);
    const Rewards = ParseMultiplier(env.REFORGED_REWARD_MULTIPLIER);
    if (Xp === undefined || Rewards === undefined) {
        return { xp: 1, rewards: 1, reason: `a multiplier is not a number from 1 to ${MAX_TEST_MULTIPLIER}` };
    }
    if (Xp === 1 && Rewards === 1) return { xp: 1, rewards: 1, reason: "not set" };
    if (env.REFORGED_SERVER_MODE !== "private") return { xp: 1, rewards: 1, reason: "this is not a private server" };
    if (!IsTailscaleHost(env.REFORGED_SERVER_HOST ?? "")) return { xp: 1, rewards: 1, reason: "the server host is not a Tailscale address" };
    return { xp: Xp, rewards: Rewards };
}

let Cached: TestMultipliers | undefined;
export function GetTestMultipliers(): TestMultipliers {
    Cached ??= ReadTestMultipliers();
    return Cached;
}

export function MultiplyXpGrant(amount: number, maxAmount: number, multipliers: TestMultipliers = GetTestMultipliers()): number {
    if (multipliers.xp === 1) return amount;
    return Math.min(maxAmount, Math.max(amount, Math.floor(amount * multipliers.xp)));
}

export function IsRewardSource(source: unknown): boolean {
    return typeof source === "string" && (REWARD_SOURCES.has(source) || REWARD_SOURCE_PREFIXES.some((Prefix) => source.startsWith(Prefix)));
}

export function IsMultipliedRewardItem(catalogId: unknown): boolean {
    return typeof catalogId === "string" && MULTIPLIED_ITEM_PREFIXES.some((Prefix) => catalogId.startsWith(Prefix));
}

/**
 * Returns the stacked items to add with the reward multiplier applied, or the same array when nothing changes.
 * `exclude` says no for an item that must keep its quantity whatever its prefix (prestige-gated, challenge-funded).
 */
export function MultiplyRewardItems(
    source: unknown,
    stackedItemsToAdd: unknown,
    exclude: (catalogId: string, quantity: number) => boolean,
    multipliers: TestMultipliers = GetTestMultipliers()
): unknown {
    if (multipliers.rewards === 1 || !IsRewardSource(source) || !Array.isArray(stackedItemsToAdd)) return stackedItemsToAdd;
    let Changed = false;
    const Out = stackedItemsToAdd.map((Item: any) => {
        const Quantity = Number(Item?.quantity);
        if (!IsMultipliedRewardItem(Item?.catalogId) || !Number.isInteger(Quantity) || Quantity <= 0 || exclude(Item.catalogId, Quantity)) return Item;
        Changed = true;
        return { ...Item, quantity: Math.floor(Quantity * multipliers.rewards) };
    });
    return Changed ? Out : stackedItemsToAdd;
}
