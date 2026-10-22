/*
 * [Dauntless Reforged Revival] Supplies refilled at every game login.
 *
 * The host can name, per account, stacked items (tonics, pylons, grenades...) that go back to a fixed
 * quantity each time that account starts a game session (POST /login). Whatever the player used or
 * picked up since is replaced by that quantity on their next login; nothing else in the inventory changes.
 *
 *   REFORGED_SESSION_SUPPLIES_FILE   a JSON file, read again at every login (no restart needed):
 *
 *   { "accounts": { "<user id>": { "QI_GRENADE_ICE": 1000, "QI_STAMINA_POTION_00": 1000 } } }
 *
 * A missing file or an account it does not name changes nothing. Currencies (CURRENCY_*) live in the
 * wallet and are refused, as are items the storage policy keeps as instanced (equipment, cells...).
 */

import fs from "node:fs";
import { logger } from "./logger";
import { GetRepositories } from "./persistence";
import { GetInventoryStoragePolicy } from "./inventoryStoragePolicy";
import { EnsureStarterBootstrapRecords } from "./controllers/starterManifest";

export const MAX_SESSION_SUPPLY_QUANTITY = 1_000_000;
const CATALOG_ID = /^[A-Za-z0-9_\-]{1,128}$/;

export type SessionSupplies = Map<string, Map<string, number>>;

// Parses the file's text. Throws with a readable reason on anything malformed, so a typo is logged
// instead of half-applied.
export function ParseSessionSupplies(text: string, isInstanced: (catalogId: string) => boolean = () => false): SessionSupplies {
    const Raw = JSON.parse(text);
    if (Raw == null || typeof Raw !== "object" || Raw.accounts == null || typeof Raw.accounts !== "object" || Array.isArray(Raw.accounts)) {
        throw new Error(`expected {"accounts": {"<user id>": {"<catalog id>": <quantity>}}}`);
    }
    const Result: SessionSupplies = new Map();
    for (const [UserId, Items] of Object.entries<any>(Raw.accounts)) {
        if (UserId.length === 0 || Items == null || typeof Items !== "object" || Array.isArray(Items)) {
            throw new Error(`account ${JSON.stringify(UserId)}: expected an object of catalog ids and quantities`);
        }
        const Targets = new Map<string, number>();
        for (const [CatalogId, Quantity] of Object.entries<any>(Items)) {
            if (!CATALOG_ID.test(CatalogId)) throw new Error(`account ${UserId}: invalid catalog id ${JSON.stringify(CatalogId)}`);
            if (CatalogId.startsWith("CURRENCY_")) throw new Error(`account ${UserId}: ${CatalogId} is a currency (wallet), not a stacked item`);
            if (isInstanced(CatalogId)) throw new Error(`account ${UserId}: ${CatalogId} is an instanced item, not a stacked one`);
            if (!Number.isSafeInteger(Quantity) || Quantity < 0 || Quantity > MAX_SESSION_SUPPLY_QUANTITY) {
                throw new Error(`account ${UserId}: quantity of ${CatalogId} must be a whole number from 0 to ${MAX_SESSION_SUPPLY_QUANTITY}`);
            }
            Targets.set(CatalogId, Quantity);
        }
        Result.set(UserId, Targets);
    }
    return Result;
}

// Sets each target item's quantity in a stacked-items list (a quantity of 0 removes the entry).
// Returns the new list, or undefined when it already matches.
export function ApplySessionSupplies(stackedItems: any[], targets: Map<string, number>): any[] | undefined {
    let Changed = false;
    const Next: any[] = [];
    const Seen = new Set<string>();
    for (const Item of stackedItems) {
        const CatalogId = Item?.catalogId;
        if (typeof CatalogId === "string" && targets.has(CatalogId)) {
            if (Seen.has(CatalogId)) { Changed = true; continue; }
            Seen.add(CatalogId);
            const Quantity = targets.get(CatalogId)!;
            if (Quantity === 0) { Changed = true; continue; }
            if (Item.quantity !== Quantity) Changed = true;
            Next.push({ ...Item, quantity: Quantity });
            continue;
        }
        Next.push(Item);
    }
    for (const [CatalogId, Quantity] of targets) {
        if (!Seen.has(CatalogId) && Quantity > 0) {
            Next.push({ catalogId: CatalogId, quantity: Quantity });
            Changed = true;
        }
    }
    return Changed ? Next : undefined;
}

function ReadTargetsForUser(userId: string, env: NodeJS.ProcessEnv): Map<string, number> | undefined {
    const File = env.REFORGED_SESSION_SUPPLIES_FILE;
    if (!File || !fs.existsSync(File)) return undefined;
    const Supplies = ParseSessionSupplies(fs.readFileSync(File, "utf8"), (CatalogId) => GetInventoryStoragePolicy(CatalogId)?.storage === "instanced");
    const Targets = Supplies.get(userId);
    return Targets && Targets.size > 0 ? Targets : undefined;
}

// Called at the game's login. Never throws: a failure is logged and the login goes on.
export async function RefillSessionSupplies(userId: string, env: NodeJS.ProcessEnv = process.env): Promise<void> {
    try {
        const Targets = ReadTargetsForUser(userId, env);
        if (Targets == undefined) return;

        const Repos = GetRepositories();
        const Characters = await Repos.characters.findManyByUserId(userId);
        for (const Character of Characters) {
            await EnsureStarterBootstrapRecords(userId, Character.characterId);
            for (let Attempt = 0; Attempt < 5; Attempt++) {
                const Inventory = await Repos.inventories.findByCharacterId(Character.characterId);
                if (Inventory == undefined) break;
                const Next = ApplySessionSupplies(JSON.parse(Inventory.stackedItems), Targets);
                if (Next == undefined) break;
                const Updated = await Repos.inventories.updateBothIfRevisionMatches(
                    Character.characterId, Inventory.instancedItems, JSON.stringify(Next), Inventory.revision ?? 0);
                if (Updated != undefined) {
                    logger.info(`[SessionSupplies] refilled ${Targets.size} item(s) for ${userId} (character ${Character.characterId})`);
                    break;
                }
            }
        }
    } catch (Error) {
        logger.error(`[SessionSupplies] could not refill supplies for ${userId}: ${(Error as any)?.message ?? Error}`);
    }
}
