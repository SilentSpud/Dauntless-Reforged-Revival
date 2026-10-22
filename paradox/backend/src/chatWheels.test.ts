// [Reforged 0017] Default quick-chat and quick-emoji wheels.
import assert from "node:assert/strict";
import test from "node:test";
import {
    BuildStarterManifest,
    CHAT_WHEEL_SLOTS,
    DEFAULT_QUICK_EMOJIS,
    STARTER_EMOJI_CATALOG_IDS,
    WithDefaultChatWheels,
} from "./controllers/starterManifest";

test("new characters own the starter emojis and start with both wheels filled", () => {
    const Manifest = BuildStarterManifest();
    const Owned = new Set(Manifest.stackedItems.map((Item) => Item.catalogId));
    for (const Id of STARTER_EMOJI_CATALOG_IDS) assert.ok(Owned.has(Id), Id);
    assert.equal(Manifest.persistent.quick_chats.length, CHAT_WHEEL_SLOTS);
    assert.equal(Manifest.persistent.emojis.length, CHAT_WHEEL_SLOTS);
    assert.ok(Manifest.persistent.quick_chats.every((Id: string) => /^qc_[a-z0-9_]+$/.test(Id)));
    for (const Id of Manifest.persistent.emojis) if (Id) assert.ok(Owned.has(Id), Id);
});

test("an all-empty saved wheel is filled, a chosen one is kept", () => {
    const Empty = { quick_chats: Array(9).fill(""), emojis: Array(9).fill(""), flare: "X" };
    const Filled = WithDefaultChatWheels(Empty);
    assert.equal(Filled.flare, "X");
    assert.deepEqual(Filled.emojis, [...DEFAULT_QUICK_EMOJIS]);
    assert.ok(Filled.quick_chats.every((Id: string) => Id.startsWith("qc_")));
    assert.deepEqual(Empty.quick_chats, Array(9).fill(""), "input is not mutated");

    const Chosen = { quick_chats: ["", "qc_nice", "", "", "", "", "", "", ""], emojis: ["EJ_PLAYER_ZAI_ANGRY"] };
    assert.deepEqual(WithDefaultChatWheels(Chosen), Chosen);

    assert.ok(WithDefaultChatWheels({}).quick_chats.length === CHAT_WHEEL_SLOTS);
    assert.equal(WithDefaultChatWheels(null), null);
});
