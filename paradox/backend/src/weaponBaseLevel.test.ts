import { test } from "node:test";
import assert from "node:assert/strict";
import { ServeWeaponsAtBaseLevel, WeaponItemDataAtBaseLevel } from "./weaponBaseLevel";

const Levelled = JSON.stringify({ TransmogCatalogID: "None", UnlockedAsTransmog: false, CurrentLevel: 5, DataVersion: 1, EquippedCells: [], EquippedWeaponParts: [], CurrentXP: 0 });

function WithEnv(Value: string | undefined, Run: () => void) {
    const Before = process.env.KEEP_WEAPON_ITEM_LEVELS;
    if (Value === undefined) delete process.env.KEEP_WEAPON_ITEM_LEVELS;
    else process.env.KEEP_WEAPON_ITEM_LEVELS = Value;
    try { Run(); } finally {
        if (Before === undefined) delete process.env.KEEP_WEAPON_ITEM_LEVELS;
        else process.env.KEEP_WEAPON_ITEM_LEVELS = Before;
    }
}

test("a levelled weapon is served at level 0 with the rest of its data kept", () => {
    const Out = JSON.parse(WeaponItemDataAtBaseLevel("WP_MS_MCROLLIN", Levelled) as string);
    assert.equal(Out.CurrentLevel, 0);
    assert.equal(Out.DataVersion, 1);
    assert.deepEqual(Out.EquippedWeaponParts, []);
});

test("level 0, missing level, other items and bad data are unchanged", () => {
    const Zero = JSON.stringify({ CurrentLevel: 0 });
    assert.equal(WeaponItemDataAtBaseLevel("WP_MS_MCROLLIN", Zero), Zero);
    assert.equal(WeaponItemDataAtBaseLevel("WP_MS_BEGINNER", null), null);
    assert.equal(WeaponItemDataAtBaseLevel("AR_ROCK_HELM", JSON.stringify({ CurrentLevel: 5 })), JSON.stringify({ CurrentLevel: 5 }));
    assert.equal(WeaponItemDataAtBaseLevel("WP_CB_MCROLLIN", "{CurrentLevel"), "{CurrentLevel");
});

test("an inventory response is copied with its weapons at level 0", () => {
    WithEnv(undefined, () => {
        const Inventory = {
            characterId: "c",
            instancedItems: [
                { catalogId: "WP_CB_MCROLLIN", instanceId: "WP_CB_MCROLLIN", itemData: Levelled, updateVersion: 5 },
                { catalogId: "LT_BASIC", instanceId: "x", itemData: null as string | null, updateVersion: 0 },
            ],
            stackedItems: [{ catalogId: "CURRENCY_NOTES", quantity: 3 }],
        };
        const Out = ServeWeaponsAtBaseLevel(Inventory);
        assert.notEqual(Out, Inventory);
        assert.equal(JSON.parse(String(Out.instancedItems[0].itemData)).CurrentLevel, 0);
        assert.equal(Out.instancedItems[0].updateVersion, 5);
        assert.equal(Out.instancedItems[1], Inventory.instancedItems[1]);
        assert.equal(Out.stackedItems, Inventory.stackedItems);
        assert.equal(JSON.parse(String(Inventory.instancedItems[0].itemData)).CurrentLevel, 5, "the stored object is not mutated");
    });
});

test("a single item and a nested transaction result are handled", () => {
    WithEnv(undefined, () => {
        const Item = { catalogId: "WP_MS_FULGAR", instanceId: "i", itemData: Levelled, updateVersion: 2 };
        assert.equal(JSON.parse(ServeWeaponsAtBaseLevel(Item).itemData).CurrentLevel, 0);
        const Result = { payload: { instancedItemsAdded: [Item] } };
        assert.equal(JSON.parse(ServeWeaponsAtBaseLevel(Result).payload.instancedItemsAdded[0].itemData).CurrentLevel, 0);
        const Plain = { a: 1, b: [1, 2] };
        assert.equal(ServeWeaponsAtBaseLevel(Plain), Plain);
    });
});

test("KEEP_WEAPON_ITEM_LEVELS=true serves the stored level", () => {
    WithEnv("true", () => {
        const Item = { catalogId: "WP_MS_FULGAR", instanceId: "i", itemData: Levelled, updateVersion: 2 };
        assert.equal(ServeWeaponsAtBaseLevel(Item), Item);
    });
});
