import assert from "node:assert/strict";
import test from "node:test";
import { IsTailscaleHost, MultiplyRewardItems, MultiplyXpGrant, ReadTestMultipliers } from "./testMultipliers";

const Private = { REFORGED_SERVER_MODE: "private", REFORGED_SERVER_HOST: "100.101.102.103", REFORGED_XP_MULTIPLIER: "5", REFORGED_REWARD_MULTIPLIER: "3" };
const Never = () => false;

test("the multipliers act only on a private server at a Tailscale address", () => {
    assert.deepEqual(ReadTestMultipliers(Private), { xp: 5, rewards: 3 });
    assert.deepEqual(ReadTestMultipliers({ ...Private, REFORGED_SERVER_HOST: "my-pc.tail1234.ts.net" }), { xp: 5, rewards: 3 });
    for (const Env of [
        { ...Private, REFORGED_SERVER_MODE: "public" },
        { ...Private, REFORGED_SERVER_MODE: undefined },
        { ...Private, REFORGED_SERVER_HOST: "203.0.113.7" },
        { ...Private, REFORGED_SERVER_HOST: "192.168.1.10" },
        { ...Private, REFORGED_SERVER_HOST: "hunts.example.net" },
        { ...Private, REFORGED_XP_MULTIPLIER: "1000" },
        { ...Private, REFORGED_REWARD_MULTIPLIER: "0.5" },
        { ...Private, REFORGED_XP_MULTIPLIER: "abc" },
        { REFORGED_SERVER_MODE: "private", REFORGED_SERVER_HOST: "100.101.102.103" },
    ]) {
        const M = ReadTestMultipliers(Env as NodeJS.ProcessEnv);
        assert.equal(M.xp, 1, JSON.stringify(Env));
        assert.equal(M.rewards, 1, JSON.stringify(Env));
    }
    assert.equal(IsTailscaleHost("100.63.255.255"), false);
    assert.equal(IsTailscaleHost("100.128.0.1"), false);
});

test("XP grants are multiplied and capped at the route maximum", () => {
    assert.equal(MultiplyXpGrant(125, 100000, { xp: 5, rewards: 1 }), 625);
    assert.equal(MultiplyXpGrant(50000, 100000, { xp: 5, rewards: 1 }), 100000);
    assert.equal(MultiplyXpGrant(125, 100000, { xp: 1, rewards: 1 }), 125);
    assert.equal(MultiplyXpGrant(3, 100000, { xp: 1.5, rewards: 1 }), 4);
});

test("only currencies and materials of a reward transaction are multiplied", () => {
    const M = { xp: 1, rewards: 3 };
    const Loot = [
        { catalogId: "TOKEN_KILL_COUNT_LERAWR_BETA", quantity: 1 },
        { catalogId: "CURRENCY_NOTES", quantity: 100 },
        { catalogId: "BREAK_LERAWR_HIDE", quantity: 3 },
        { catalogId: "CURRENCY_PJM_WEAPON", quantity: 10 },
        { catalogId: "GATHER_COMMON_01", quantity: 1 },
    ];
    assert.deepEqual(MultiplyRewardItems("PlayerLoot", Loot, Never, M), [
        { catalogId: "TOKEN_KILL_COUNT_LERAWR_BETA", quantity: 1 },
        { catalogId: "CURRENCY_NOTES", quantity: 300 },
        { catalogId: "BREAK_LERAWR_HIDE", quantity: 9 },
        { catalogId: "CURRENCY_PJM_WEAPON", quantity: 30 },
        { catalogId: "GATHER_COMMON_01", quantity: 3 },
    ]);
    assert.deepEqual(MultiplyRewardItems("BUNDLE_GATHER_COMMON_01", [{ catalogId: "GATHER_COMMON_01", quantity: 1 }], Never, M), [{ catalogId: "GATHER_COMMON_01", quantity: 3 }]);
    const Quest = [{ catalogId: "TOKEN_UNLOCK_WEAPON_AXE", quantity: 1 }, { catalogId: "DYE_PURPLE06_DURABLE", quantity: 1 }];
    assert.equal(MultiplyRewardItems("quest", Quest, Never, M), Quest);
});

test("crafting, purchases, excluded items and an x1 server keep their quantities", () => {
    const M = { xp: 1, rewards: 3 };
    const Notes = [{ catalogId: "CURRENCY_NOTES", quantity: 100 }];
    for (const Source of ["armorrecipe", "PJM Unlock: GliderUpgrade_0", "UBountyComponent::GrantBounty", "default", undefined]) {
        assert.equal(MultiplyRewardItems(Source, Notes, Never, M), Notes, String(Source));
    }
    assert.equal(MultiplyRewardItems("PlayerLoot", Notes, Never, { xp: 1, rewards: 1 }), Notes);
    const Prestige = [{ catalogId: "CURRENCY_PJM_PRESTIGE_EMPTY", quantity: 5 }];
    assert.equal(MultiplyRewardItems("patrol_chest", Prestige, (Id) => Id === "CURRENCY_PJM_PRESTIGE_EMPTY", M), Prestige);
});
