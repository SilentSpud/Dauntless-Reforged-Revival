// The progression config (GET /progression/config) built from the host's own game, for hosts who have
// no captured server response to supply.
//
// The 1.14.7 client loads its player data only when the config has the tracks it reads: the
// ExperienceTrack_* (player and weapon levels), MasteryTrack_* and PrestigeTrack_* tracks, and a path for
// the active Hunt Pass. Without them the progression, Hunt Pass, quest and bounty components never finish
// loading, the 60-second player-data timeout fires and the hub leaves its map (Ramsgate: "Loading
// PlayerData Failed", then "LoadMap: failed to Listen"). Mystic Paradox's example config has none of them.
//
// Those tracks are the game's own data tables, which CatalogExporter writes with EXPORT_PROGRESSION=1 to
// Items_Analysis/progression_1_14_7/*.jsonl: per-rank XP costs (incremental, as the server sends them)
// and the rewards of each rank. They are converted here into the shape of the live service's response
// (field names checked against a captured response). The Hunt Pass rows in the game hold no tiers, so the
// active pass gets the live service's usual 50 ranks of 100 XP and no rewards. The backend then gives it
// the current window and adds the newer seasons (WidenHuntPassWindows).
//
// Each experience track names its prestige track (PrestigeTrack_Weapon_Sword for the sword, ...), and the
// client asks for those too ("Track PrestigeTrack_Weapon_Sword not found in the cached progression
// configs"). The export has no rank table for them, so a prestige track the export does not hold gets the
// ranks of the track that names it, and no rewards. Every weapon gets one whether or not its row names it
// (PrestigeTrack_Weapon_<weapon> for ExperienceTrack_Weapon_<weapon>): the Slayer level badge
// (USlayerLevelWidget) picks its frame by the weapon's prestige level, which is -1 when the track is not in
// the config, and indexes its frame list with it - the client crashes on entering Ramsgate.

import fs from "node:fs";
import path from "node:path";

export const PROGRESSION_EXPORT_DIR = "progression_1_14_7";
// The files with track rows ({"rowName", "base": {...}}), in order of preference for a track id.
export const TRACK_EXPORTS = ["player-experience-tracks.jsonl", "weapon-experience-tracks.jsonl", "experience-tracks.jsonl", "mastery-tracks.jsonl"];
export const HUNT_PASS_EXPORT = "huntpass-season-table.jsonl";
export const ACTIVE_HUNT_PASS = "season43"; // the backend's ACTIVE_HUNTPASS_PROGRESSION_ID
// Marks a source this module wrote, so the next export replaces it (a captured response is never touched).
export const GENERATED_BY = "Dauntless Reforged Revival host kit, from the CatalogExporter progression export";

const TRACK_START = "2019-01-01T14:00:00+00:00";
const TRACK_END = "2099-01-01T14:00:00+00:00";
const HUNT_PASS_RANKS = 50;
const HUNT_PASS_XP_PER_RANK = 100;
const REQUIRED_TRACKS = ["ExperienceTrack_PlayerLevel"];
const WEAPON_EXPERIENCE_PREFIX = "ExperienceTrack_Weapon_";
const WEAPON_PRESTIGE_PREFIX = "PrestigeTrack_Weapon_";

interface ExportedReward {
    rankId?: number;
    customRewardString?: string;
    stackedItems?: { catalogId?: string; quantity?: number; priority?: number }[];
    instancedItems?: string[];
    orderedInstancedItems?: { catalogId?: string; priority?: number }[];
    entitlements?: { entitlementId?: string; duration?: number }[];
}

interface ExportedBase {
    progressionTrack?: string;
    premiumGatingEntitlement?: string;
    progressionMultiplier?: number;
    requirements?: { rankId?: number; requiredXP?: number }[];
    freeRewards?: ExportedReward[];
    premiumRewards?: ExportedReward[];
}

interface ExportedRow {
    rowName?: string;
    base?: ExportedBase;
    prestigeTrack?: string;
}

export function readJsonl(file: string): ExportedRow[] {
    if (!fs.existsSync(file)) return [];
    const rows: ExportedRow[] = [];
    for (const line of fs.readFileSync(file, "utf8").replace(/^﻿/, "").split(/\r?\n/)) {
        const text = line.trim();
        if (!text) continue;
        try {
            const row = JSON.parse(text) as unknown;
            if (row && typeof row === "object" && !Array.isArray(row)) rows.push(row as ExportedRow);
        } catch {
            /* a status or partial line: not a row */
        }
    }
    return rows;
}

const whole = (v: unknown, fallback = 0) => (typeof v === "number" && Number.isSafeInteger(v) && v >= 0 ? v : fallback);
const text = (v: unknown) => (typeof v === "string" ? v : "");

function convertReward(r: ExportedReward): Record<string, unknown> {
    return {
        rank_id: whole(r.rankId),
        custom_reward_string: { culture_invariant_string: text(r.customRewardString) || null },
        stacked_items: (r.stackedItems ?? []).filter((s) => text(s.catalogId)).map((s) => ({ catalog_id: text(s.catalogId), hidden: false, priority: whole(s.priority), quantity: whole(s.quantity, 1) })),
        instanced_items: (r.instancedItems ?? []).filter((s) => typeof s === "string" && s.length > 0),
        ordered_instanced_items: (r.orderedInstancedItems ?? []).filter((s) => text(s.catalogId)).map((s) => ({ catalog_id: text(s.catalogId), priority: whole(s.priority) })),
        entitlements: (r.entitlements ?? []).filter((e) => text(e.entitlementId)).map((e) => ({ duration: whole(e.duration), entitlement: text(e.entitlementId) })),
        buffs: [],
    };
}

function emptyReward(rank: number): Record<string, unknown> {
    return { rank_id: rank, stacked_items: [], instanced_items: [], ordered_instanced_items: [], entitlements: [] };
}

export interface ProgressionBuild {
    config: { generated_by: string; code: null; message: string; payload: { paths: Record<string, unknown>[] } };
    tracks: string[];
    skipped: string[];
    // Prestige tracks given the ranks of the track that names them.
    derived: string[];
}

// Builds the config from an Items_Analysis folder, or throws with what is missing.
export function buildProgressionConfig(exportsDir: string, activeHuntPass = ACTIVE_HUNT_PASS): ProgressionBuild {
    const dir = path.join(exportsDir, PROGRESSION_EXPORT_DIR);
    if (!fs.existsSync(dir)) throw new Error(`no ${PROGRESSION_EXPORT_DIR} in ${exportsDir} (inject CatalogExporter with EXPORT_PROGRESSION=1, then copy Items_Analysis)`);
    const paths: Record<string, unknown>[] = [];
    const seen = new Set<string>();
    const skipped: string[] = [];
    const prestige: { id: string; requirements: { rank_id: number; xp_required: number }[] }[] = [];
    for (const file of TRACK_EXPORTS) {
        for (const row of readJsonl(path.join(dir, file))) {
            const base = row.base ?? {};
            const id = text(base.progressionTrack) || text(row.rowName);
            if (!id || seen.has(id)) continue;
            const requirements = (base.requirements ?? [])
                .map((q) => ({ rank_id: whole(q.rankId, -1), xp_required: whole(q.requiredXP, -1) }))
                .filter((q) => q.rank_id >= 0 && q.xp_required >= 0)
                .sort((a, b) => a.rank_id - b.rank_id);
            const named = text(row.prestigeTrack);
            if (named && named !== "None" && requirements.length > 0) prestige.push({ id: named, requirements });
            if (id.startsWith(WEAPON_EXPERIENCE_PREFIX) && requirements.length > 0) {
                prestige.push({ id: WEAPON_PRESTIGE_PREFIX + id.slice(WEAPON_EXPERIENCE_PREFIX.length), requirements });
            }
            if (requirements.length === 0) {
                skipped.push(id);
                continue;
            }
            seen.add(id);
            paths.push({
                progression_id: id,
                premium_gating_entitlement: text(base.premiumGatingEntitlement),
                progression_multiplier: typeof base.progressionMultiplier === "number" && base.progressionMultiplier > 0 ? base.progressionMultiplier : 1,
                start_date: TRACK_START,
                end_date: TRACK_END,
                requirements,
                free_rewards: (base.freeRewards ?? []).map(convertReward),
                premium_rewards: (base.premiumRewards ?? []).map(convertReward),
            });
        }
    }
    const derived: string[] = [];
    for (const p of prestige) {
        if (seen.has(p.id)) continue;
        seen.add(p.id);
        derived.push(p.id);
        paths.push({
            progression_id: p.id,
            premium_gating_entitlement: "",
            progression_multiplier: 1,
            start_date: TRACK_START,
            end_date: TRACK_END,
            requirements: p.requirements,
            free_rewards: [],
            premium_rewards: [],
        });
    }
    const missing = REQUIRED_TRACKS.filter((id) => !seen.has(id));
    if (missing.length > 0) throw new Error(`the progression export has no ${missing.join(", ")} (an incomplete export: inject CatalogExporter again)`);

    // The active Hunt Pass: its identity from the game's season table, 50 ranks of 100 XP, no rewards.
    const season = readJsonl(path.join(dir, HUNT_PASS_EXPORT)).find((r) => text(r.base?.progressionTrack) === activeHuntPass || text(r.rowName).toLowerCase() === activeHuntPass.toLowerCase());
    const ranks = Array.from({ length: HUNT_PASS_RANKS + 1 }, (_, rank) => rank);
    if (!seen.has(activeHuntPass)) {
        paths.push({
            progression_id: activeHuntPass,
            premium_gating_entitlement: text(season?.base?.premiumGatingEntitlement) || `${activeHuntPass}_premium`,
            progression_multiplier: 1,
            start_date: TRACK_START,
            end_date: TRACK_END,
            requirements: ranks.map((rank) => ({ rank_id: rank, xp_required: rank === 0 ? 0 : HUNT_PASS_XP_PER_RANK })),
            free_rewards: ranks.map(emptyReward),
            premium_rewards: ranks.map(emptyReward),
            prestige: null,
        });
        seen.add(activeHuntPass);
    }
    return { config: { generated_by: GENERATED_BY, code: null, message: "OK", payload: { paths } }, tracks: [...seen], skipped, derived };
}

// The source file the backend's generator reads: written from the export unless the host supplied a
// captured response. Returns what it did.
export function writeProgressionSource(exportsDir: string, sourceFile: string): "supplied" | "no_export" | { written: number; derived: number } {
    if (fs.existsSync(sourceFile)) {
        try {
            const existing = JSON.parse(fs.readFileSync(sourceFile, "utf8").replace(/^﻿/, "")) as { generated_by?: unknown };
            if (existing.generated_by !== GENERATED_BY) return "supplied";
        } catch {
            return "supplied";
        }
    }
    if (!fs.existsSync(path.join(exportsDir, PROGRESSION_EXPORT_DIR))) return "no_export";
    const built = buildProgressionConfig(exportsDir);
    fs.mkdirSync(path.dirname(sourceFile), { recursive: true });
    fs.writeFileSync(sourceFile, JSON.stringify(built.config, null, 2) + "\n");
    return { written: built.config.payload.paths.length, derived: built.derived.length };
}
