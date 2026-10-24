// [Dauntless Reforged Revival 0036] Weapons are served at item level 0.
//
// A weapon's itemData carries CurrentLevel (EUpgradeableItemLevel). The game picks the weapon's power
// table for that level (UArchonGlobalTuning::GetWeaponPowerTable(ItemLevel), from
// WeaponPowerTablePerLevels). The 1.14.7 paks only carry the base weapon_power_table (plus
// weapon_power_table_level_01 and an obprog one), so any weapon with CurrentLevel above 0 gets no table:
// the game server logs "Weapon: invalid weapon power table reference on (<weapon>_bp_C)" and every
// value read from the table is 0. Those values include CBlade_MeterGainScale, MSpear_MeterGainScale,
// MSpear_MeterGainPerX/YAttack and the War Pike damage multipliers, so on such a weapon the special
// meter never fills and the War Pike stops wounding (seen on the VPS 2026-10-23 with WP_MS_MCROLLIN and
// WP_CB_MCROLLIN at CurrentLevel 5; level-0 weapons never log the warning).
//
// The stored level is left as it is; only what the game reads is set to 0, so the base table is used.
// KEEP_WEAPON_ITEM_LEVELS=true serves the stored levels again.

export function KeepWeaponItemLevels(): boolean {
    return (process.env.KEEP_WEAPON_ITEM_LEVELS ?? "").trim().toLowerCase() === "true";
}

// itemData with CurrentLevel set to 0 for a weapon (catalog id WP_...) whose level is above 0;
// anything else (other items, no level, unparsable data) is returned unchanged.
export function WeaponItemDataAtBaseLevel(catalogId: unknown, itemData: unknown): unknown {
    if (typeof catalogId !== "string" || !catalogId.startsWith("WP_") || typeof itemData !== "string") return itemData;
    if (!itemData.includes("CurrentLevel")) return itemData;
    let Parsed: any;
    try { Parsed = JSON.parse(itemData); } catch { return itemData; }
    if (Parsed == null || typeof Parsed !== "object" || Array.isArray(Parsed)) return itemData;
    if (typeof Parsed.CurrentLevel !== "number" || Parsed.CurrentLevel <= 0) return itemData;
    return JSON.stringify({ ...Parsed, CurrentLevel: 0 });
}

// A copy of `value` in which every item object ({catalogId, itemData}) found in it, in arrays or
// object fields up to a few levels deep, has its weapon level served as 0. Nothing is mutated.
export function ServeWeaponsAtBaseLevel<T>(value: T, depth: number = 0): T {
    if (KeepWeaponItemLevels() || value == null || typeof value !== "object" || depth > 4) return value;
    if (Array.isArray(value)) {
        let Changed = false;
        const Out = value.map((entry) => {
            const Next = ServeWeaponsAtBaseLevel(entry, depth + 1);
            if (Next !== entry) Changed = true;
            return Next;
        });
        return (Changed ? Out : value) as T;
    }
    const Source = value as any;
    let Out: any = undefined;
    if ("catalogId" in Source && "itemData" in Source) {
        const ItemData = WeaponItemDataAtBaseLevel(Source.catalogId, Source.itemData);
        if (ItemData !== Source.itemData) Out = { ...Source, itemData: ItemData };
    }
    for (const Key of Object.keys(Source)) {
        const Field = Source[Key];
        if (Field == null || typeof Field !== "object") continue;
        const Next = ServeWeaponsAtBaseLevel(Field, depth + 1);
        if (Next !== Field) {
            if (Out === undefined) Out = { ...Source };
            Out[Key] = Next;
        }
    }
    return (Out ?? value) as T;
}
