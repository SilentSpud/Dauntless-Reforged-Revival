import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { describe, it } from "node:test";
import { ACTIVE_HUNT_PASS, buildProgressionConfig, GENERATED_BY, PROGRESSION_EXPORT_DIR, writeProgressionSource } from "../src/progression";
import { tempDir, write } from "./helpers";

// Rows as CatalogExporter's ProgressionExporter writes them (one JSON object per line).
function reward(rankId: number, extra: Record<string, unknown> = {}) {
    return { rankId, customRewardString: "", stackedItems: [], instancedItems: [], orderedInstancedItems: [], entitlements: [], buffCount: 0, ...extra };
}
function track(rowName: string, progressionTrack: string, xp: number[], freeRewards: unknown[] = [], prestigeTrack = "None") {
    return {
        rowName,
        prestigeTrack,
        base: {
            progressionTrack, premiumGatingEntitlement: "", resetWithCharacter: false, progressionMultiplier: 1, requirementCount: xp.length,
            requirements: xp.map((requiredXP, rankId) => ({ rankId, requiredXP })),
            freeRewards, premiumRewards: [],
        },
    };
}
const jsonl = (rows: unknown[]) => "﻿" + rows.map((r) => JSON.stringify(r)).join("\r\n") + "\r\nstatus: 3 rows\r\n";

function makeExport(withPlayerLevel = true): string {
    const root = tempDir("progression");
    const dir = path.join(root, PROGRESSION_EXPORT_DIR);
    write(path.join(dir, "player-experience-tracks.jsonl"), jsonl(withPlayerLevel ? [
        track("PlayerLevel", "ExperienceTrack_PlayerLevel", [0, 100, 250], [
            reward(1, { stackedItems: [{ catalogId: "CURRENCY_RAMS", quantity: 500, priority: 0 }], instancedItems: ["WP_SWORD_00"], entitlements: [{ entitlementId: "ent_title_00", duration: 0 }] }),
        ]),
    ] : []));
    write(path.join(dir, "weapon-experience-tracks.jsonl"), jsonl([track("Sword", "ExperienceTrack_Weapon_Sword", [0, 50], [], "PrestigeTrack_Weapon_Sword"), track("Axe", "ExperienceTrack_Weapon_Axe", [0, 70]), track("Empty", "ExperienceTrack_Weapon_None", [])]));
    // A later row with an id already taken is ignored.
    write(path.join(dir, "mastery-tracks.jsonl"), jsonl([track("MasteryPlayer", "MasteryTrack_PlayerLevel", [0, 1, 2]), ...(withPlayerLevel ? [track("Duplicate", "ExperienceTrack_PlayerLevel", [0, 9])] : [])]));
    write(path.join(dir, "huntpass-season-table.jsonl"), jsonl([{ rowName: "HuntPass_Season43", base: { progressionTrack: "season43", premiumGatingEntitlement: "season43_premium", requirements: [], freeRewards: [], premiumRewards: [] } }]));
    return root;
}

describe("the progression config from the game's own tables", () => {
    it("converts the exported tracks to the live service's shape and adds the active Hunt Pass", () => {
        const built = buildProgressionConfig(makeExport());
        const paths = built.config.payload.paths as Record<string, any>[];
        const ids = paths.map((p) => p.progression_id);
        assert.deepEqual(ids, ["ExperienceTrack_PlayerLevel", "ExperienceTrack_Weapon_Sword", "ExperienceTrack_Weapon_Axe", "MasteryTrack_PlayerLevel", "PrestigeTrack_Weapon_Sword", "PrestigeTrack_Weapon_Axe", ACTIVE_HUNT_PASS]);
        assert.deepEqual(built.skipped, ["ExperienceTrack_Weapon_None"], "a track without ranks is left out");
        assert.deepEqual(built.derived, ["PrestigeTrack_Weapon_Sword", "PrestigeTrack_Weapon_Axe"], "each weapon's prestige track, named by its row or not");
        assert.deepEqual(paths[5].requirements, [{ rank_id: 0, xp_required: 0 }, { rank_id: 1, xp_required: 70 }]);
        const prestige = paths[4];
        assert.deepEqual(prestige.requirements, [{ rank_id: 0, xp_required: 0 }, { rank_id: 1, xp_required: 50 }], "with the sword's ranks");
        assert.deepEqual([prestige.free_rewards, prestige.premium_rewards], [[], []]);

        const level = paths[0];
        assert.deepEqual(level.requirements, [{ rank_id: 0, xp_required: 0 }, { rank_id: 1, xp_required: 100 }, { rank_id: 2, xp_required: 250 }]);
        assert.equal(level.start_date, "2019-01-01T14:00:00+00:00");
        assert.equal(level.end_date, "2099-01-01T14:00:00+00:00");
        assert.deepEqual(level.free_rewards[0], {
            rank_id: 1,
            custom_reward_string: { culture_invariant_string: null },
            stacked_items: [{ catalog_id: "CURRENCY_RAMS", hidden: false, priority: 0, quantity: 500 }],
            instanced_items: ["WP_SWORD_00"],
            ordered_instanced_items: [],
            entitlements: [{ duration: 0, entitlement: "ent_title_00" }],
            buffs: [],
        });

        const pass = paths[6];
        assert.equal(pass.premium_gating_entitlement, "season43_premium");
        assert.equal(pass.requirements.length, 51);
        assert.deepEqual(pass.requirements.slice(0, 2), [{ rank_id: 0, xp_required: 0 }, { rank_id: 1, xp_required: 100 }]);
        assert.equal(pass.free_rewards.length, 51);
        assert.equal(built.config.code, null);
        assert.equal(built.config.message, "OK");
    });

    it("refuses an export without the player level track", () => {
        assert.throws(() => buildProgressionConfig(makeExport(false)), /ExperienceTrack_PlayerLevel/);
        assert.throws(() => buildProgressionConfig(tempDir("no-export")), /EXPORT_PROGRESSION=1/);
    });

    it("writes the source from the export, replaces its own next time and never a supplied response", () => {
        const exportsDir = makeExport();
        const source = path.join(tempDir("raw"), "progression_config.source.json");
        assert.equal(writeProgressionSource(tempDir("none"), source), "no_export");
        assert.deepEqual(writeProgressionSource(exportsDir, source), { written: 7, derived: 2 });
        assert.equal(JSON.parse(fs.readFileSync(source, "utf8")).generated_by, GENERATED_BY);
        assert.deepEqual(writeProgressionSource(exportsDir, source), { written: 7, derived: 2 }, "a source it wrote is rebuilt");
        fs.writeFileSync(source, JSON.stringify({ code: null, message: "OK", payload: { paths: [] } }));
        assert.equal(writeProgressionSource(exportsDir, source), "supplied");
        assert.deepEqual(JSON.parse(fs.readFileSync(source, "utf8")).payload.paths, [], "a captured response is kept");
    });
});
