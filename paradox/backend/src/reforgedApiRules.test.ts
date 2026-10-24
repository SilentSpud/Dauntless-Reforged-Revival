/*
 * Copyright (C) 2026 Dauntless Reforged Revival contributors
 * SPDX-License-Identifier: AGPL-3.0-only
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import {
    ADMIN_KEY_HEADER,
    BACKEND_GAME_DATA_FILES,
    CleanInviteNote,
    CleanUsername,
    DetectGameData,
    GATEWAY_HEADER,
    GenerateAccountKey,
    GenerateInviteCode,
    InstanceTitle,
    IsLocalAdminRequest,
    IsPlausibleAccountKey,
    IsValidInviteCode,
    KickAccount,
    ParseDirectorInstances,
    ReadRegistrationMode,
    ShortAssetName
} from "./reforgedApiRules";

test("usernames follow the Dauntless Revived rule", () => {
    assert.equal(CleanUsername("Slayer_01"), "Slayer_01");
    assert.equal(CleanUsername("  Slayer  "), "Slayer");
    assert.equal(CleanUsername("ab"), undefined);
    assert.equal(CleanUsername("a".repeat(17)), undefined);
    assert.equal(CleanUsername("bad name"), undefined);
    assert.equal(CleanUsername("nãome"), undefined);
    assert.equal(CleanUsername(42), undefined);
});

test("invite codes are three groups of four look-alike-free characters", () => {
    for (let I = 0; I < 50; I++) {
        const Code = GenerateInviteCode();
        assert.match(Code, /^[A-HJ-KM-NP-Z2-9]{4}-[A-HJ-KM-NP-Z2-9]{4}-[A-HJ-KM-NP-Z2-9]{4}$/);
        assert.ok(IsValidInviteCode(Code));
    }
    assert.equal(GenerateInviteCode(() => 0), "AAAA-AAAA-AAAA");
    assert.ok(!IsValidInviteCode("abc"));
    assert.ok(!IsValidInviteCode("has space"));
});

test("account keys fit on the game's command line", () => {
    const Key = GenerateAccountKey();
    assert.match(Key, /^RRK_[0-9a-f]{48}$/);
    assert.ok(IsPlausibleAccountKey(Key));
    assert.ok(!IsPlausibleAccountKey("short"));
    assert.ok(!IsPlausibleAccountKey("with space in it"));
    assert.ok(!IsPlausibleAccountKey("-AUTH_PASSWORD=x\"y"));
});

test("registration mode defaults to invite codes", () => {
    assert.equal(ReadRegistrationMode(undefined), "INVITECODE");
    assert.equal(ReadRegistrationMode("open"), "OPEN");
    assert.equal(ReadRegistrationMode(" NONE "), "NONE");
    assert.equal(ReadRegistrationMode("whatever"), "INVITECODE");
});

test("invite notes lose control characters and are capped", () => {
    assert.equal(CleanInviteNote("for\nAlex"), "for Alex");
    assert.equal(CleanInviteNote("   "), undefined);
    assert.equal(CleanInviteNote("x".repeat(100))?.length, 64);
    assert.equal(CleanInviteNote(5), undefined);
});

function FakeRequest(Address: string, Headers: Record<string, string>) {
    return { socket: { remoteAddress: Address }, headers: Headers } as any;
}

test("admin routes need a direct local connection and the admin key", () => {
    const Key = "k".repeat(40);
    assert.ok(IsLocalAdminRequest(FakeRequest("127.0.0.1", { [ADMIN_KEY_HEADER]: Key }), Key));
    assert.ok(IsLocalAdminRequest(FakeRequest("::ffff:127.0.0.1", { [ADMIN_KEY_HEADER]: Key }), Key));
    assert.ok(IsLocalAdminRequest(FakeRequest("::1", { [ADMIN_KEY_HEADER]: Key }), Key));
    // Wrong key, no key, a remote peer, anything the gateway forwarded, or no configured key at all.
    assert.ok(!IsLocalAdminRequest(FakeRequest("127.0.0.1", { [ADMIN_KEY_HEADER]: "x".repeat(40) }), Key));
    assert.ok(!IsLocalAdminRequest(FakeRequest("127.0.0.1", {}), Key));
    assert.ok(!IsLocalAdminRequest(FakeRequest("203.0.113.9", { [ADMIN_KEY_HEADER]: Key }), Key));
    assert.ok(!IsLocalAdminRequest(FakeRequest("127.0.0.1", { [ADMIN_KEY_HEADER]: Key, [GATEWAY_HEADER]: "s" }), Key));
    assert.ok(!IsLocalAdminRequest(FakeRequest("127.0.0.1", { [ADMIN_KEY_HEADER]: Key, "x-forwarded-for": "1.2.3.4" }), Key));
    assert.ok(!IsLocalAdminRequest(FakeRequest("127.0.0.1", { [ADMIN_KEY_HEADER]: "short" }), "short"));
    assert.ok(!IsLocalAdminRequest(FakeRequest("127.0.0.1", { [ADMIN_KEY_HEADER]: Key }), undefined));
});

test("director instances are checked field by field", () => {
    const { instances, huntPlayers } = ParseDirectorInstances([
        { id: "city-8777", kind: "city", map: "/Game/Maps/ramsgate/ramsgate_01_persistent", behemoth: null, players: 0, maxPlayers: 0, startedAt: "2026-10-01T10:00:00Z" },
        { id: "hunt-8770", kind: "hunt", map: "/Game/Maps/islands/a/IslandA_persistent", behemoth: "/Game/Behemoths/BP_Skarn.BP_Skarn_C", players: 2, maxPlayers: 4, playerIds: ["u1", "u2", 7], startedAt: "nope" },
        { id: "x", kind: "spaceship" },
        "junk",
        null
    ]);
    assert.equal(instances.length, 2);
    assert.equal(instances[0].title, "Ramsgate");
    assert.equal(instances[0].map, "ramsgate_01_persistent");
    assert.equal(instances[1].title, "Skarn");
    assert.equal(instances[1].behemoth, "BP_Skarn");
    assert.equal(instances[1].startedAt, null);
    assert.equal(huntPlayers.get("u1"), "hunt-8770");
    assert.equal(huntPlayers.size, 2);
    assert.deepEqual(ParseDirectorInstances({}).instances, []);
});

test("asset names and titles", () => {
    assert.equal(ShortAssetName("/Game/A/B/c_d.e_C"), "c_d");
    assert.equal(InstanceTitle("dojo", null, ""), "Training Grounds");
    assert.equal(InstanceTitle("hunt", null, "/Game/Maps/x/Shattered_Isles_persistent"), "Shattered Isles");
    assert.equal(InstanceTitle("hunt", null, ""), "Hunt");
});

test("game data is told apart from the synthetic examples", () => {
    const Dir = fs.mkdtempSync(path.join(os.tmpdir(), "reforged-gamedata-"));
    try {
        assert.equal(DetectGameData(Dir), "missing");
        for (const Name of BACKEND_GAME_DATA_FILES) {
            fs.writeFileSync(path.join(Dir, Name.replace(/\.json$/, ".example.json")), "{\"example\":true}");
            fs.writeFileSync(path.join(Dir, Name), "{\"example\":true}");
        }
        assert.equal(DetectGameData(Dir), "synthetic");
        for (const Name of BACKEND_GAME_DATA_FILES) fs.writeFileSync(path.join(Dir, Name), "{\"real\":true}");
        assert.equal(DetectGameData(Dir), "real");
        fs.rmSync(path.join(Dir, BACKEND_GAME_DATA_FILES[0]));
        assert.equal(DetectGameData(Dir), "missing");
    } finally {
        fs.rmSync(Dir, { recursive: true, force: true });
    }
});

test("a kick revokes the account's tokens, then closes every chat connection it holds", async () => {
    const Calls: string[] = [];
    const Connection = (Name: string) => ({ close: (Code: number, Reason: string) => void Calls.push(`${Name} ${Code} ${Reason}`) });
    const Closed = await KickAccount("u1", {
        connectionsFor: (Id) => (Id === "u1" ? [Connection("pc"), Connection("game")] : []),
        revokeRefreshSessions: async (Id) => void Calls.push(`refresh ${Id}`),
        revokeExchangeCodes: async (Id) => void Calls.push(`codes ${Id}`)
    });
    assert.equal(Closed, 2);
    assert.deepEqual(Calls, ["refresh u1", "codes u1", "pc 1008 kicked by the host", "game 1008 kicked by the host"]);
    const Offline = await KickAccount("u2", { connectionsFor: () => [], revokeRefreshSessions: async () => {}, revokeExchangeCodes: async () => {} });
    assert.equal(Offline, 0);
});
