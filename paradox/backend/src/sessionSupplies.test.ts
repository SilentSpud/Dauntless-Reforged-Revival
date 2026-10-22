import assert from "node:assert/strict";
import test from "node:test";
import { ApplySessionSupplies, ParseSessionSupplies } from "./sessionSupplies";

const User = "8ab0a97c-9ceb-46ad-a273-9bbb515d122c";

test("the file names quantities per account", () => {
    const Supplies = ParseSessionSupplies(JSON.stringify({ accounts: { [User]: { QI_GRENADE_ICE: 1000, QI_STAMINA_POTION_00: 0 } } }));
    assert.deepEqual([...Supplies.get(User)!], [["QI_GRENADE_ICE", 1000], ["QI_STAMINA_POTION_00", 0]]);
    assert.equal(Supplies.get("someone-else"), undefined);
});

test("malformed files, currencies and instanced items are refused", () => {
    assert.throws(() => ParseSessionSupplies("{}"));
    assert.throws(() => ParseSessionSupplies("not json"));
    assert.throws(() => ParseSessionSupplies(JSON.stringify({ accounts: { [User]: { QI_GRENADE_ICE: -1 } } })));
    assert.throws(() => ParseSessionSupplies(JSON.stringify({ accounts: { [User]: { QI_GRENADE_ICE: 1.5 } } })));
    assert.throws(() => ParseSessionSupplies(JSON.stringify({ accounts: { [User]: { QI_GRENADE_ICE: "1000" } } })));
    assert.throws(() => ParseSessionSupplies(JSON.stringify({ accounts: { [User]: { CURRENCY_NOTES: 1000 } } })));
    assert.throws(() => ParseSessionSupplies(JSON.stringify({ accounts: { [User]: { "bad id!": 1 } } })));
    assert.throws(() => ParseSessionSupplies(JSON.stringify({ accounts: { [User]: { WEAPON_X: 1 } } }), (Id) => Id === "WEAPON_X"));
});

test("quantities are set, not added, and other items are left alone", () => {
    const Targets = new Map([["QI_GRENADE_ICE", 1000], ["QI_DEFENSE_PYLON", 1000], ["QI_LANTERN_POTION", 0]]);
    const Stacked = [
        { catalogId: "QI_GRENADE_ICE", quantity: 3 },
        { catalogId: "BREAK_SOMETHING", quantity: 7 },
        { catalogId: "QI_LANTERN_POTION", quantity: 2 },
    ];
    assert.deepEqual(ApplySessionSupplies(Stacked, Targets), [
        { catalogId: "QI_GRENADE_ICE", quantity: 1000 },
        { catalogId: "BREAK_SOMETHING", quantity: 7 },
        { catalogId: "QI_DEFENSE_PYLON", quantity: 1000 },
    ]);
    assert.equal(ApplySessionSupplies([{ catalogId: "QI_GRENADE_ICE", quantity: 1000 }, { catalogId: "QI_DEFENSE_PYLON", quantity: 1000 }], Targets), undefined);
    // Duplicate entries collapse to one.
    assert.deepEqual(ApplySessionSupplies([{ catalogId: "QI_GRENADE_ICE", quantity: 1 }, { catalogId: "QI_GRENADE_ICE", quantity: 1 }], new Map([["QI_GRENADE_ICE", 5]])),
        [{ catalogId: "QI_GRENADE_ICE", quantity: 5 }]);
});
