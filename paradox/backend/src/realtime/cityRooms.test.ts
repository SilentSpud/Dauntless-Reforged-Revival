// [Reforged 0025] City chat rooms are open to the accounts matchmaking sent to that hub.
import assert from "node:assert/strict";
import test from "node:test";

import { MayJoinCityRoom, RecordHubSessionServed, ResetCityRoomsForTest } from "./cityRooms";
import { RoomService } from "./RoomService";
import type { RegisteredConnection } from "./SessionRegistry";

class FakeConnection implements RegisteredConnection {
    readonly sent: string[] = [];
    constructor(readonly connId: string, readonly accountId: string, readonly resource: string) {}
    send(frame: string): void { this.sent.push(frame); }
    close(): void { /* no-op */ }
}

const HUB = "179cf2a8-0716-49f5-9027-6e4876ba0d76";
const OTHER = "c069ba56-fe67-40b7-8e19-cb4a4813dc22";
const DOMAIN = "muc.prod.ol.epicgames.com";

test("only the hub an account was last sent to opens its City room", () => {
    ResetCityRoomsForTest();
    assert.equal(MayJoinCityRoom("alice", `City-${HUB}`), false);
    RecordHubSessionServed("alice", HUB);
    assert.equal(MayJoinCityRoom("alice", `City-${HUB}`), true);
    assert.equal(MayJoinCityRoom("alice", `City-${OTHER}`), false);
    assert.equal(MayJoinCityRoom("bob", `City-${HUB}`), false);
    assert.equal(MayJoinCityRoom("alice", `Hunt-${HUB}`), false);
    RecordHubSessionServed("alice", OTHER);
    assert.equal(MayJoinCityRoom("alice", `City-${HUB}`), false);
    assert.equal(MayJoinCityRoom("alice", `City-${OTHER}`), true);
    assert.equal(MayJoinCityRoom("alice", `City-${OTHER}`, Date.now() + 8 * 24 * 3600 * 1000), false);
});

test("with the dev switch off, players sent to the hub chat in its room and others are refused", async () => {
    ResetCityRoomsForTest();
    RecordHubSessionServed("alice", HUB);
    RecordHubSessionServed("bob", HUB);
    const service = new RoomService(false);
    const alice = new FakeConnection("ca", "alice", "ra");
    const bob = new FakeConnection("cb", "bob", "rb");
    const eve = new FakeConnection("ce", "eve", "re");
    const room = `City-${HUB}@${DOMAIN}`;

    assert.match((await service.joinRoom(alice, `${room}/Alice:alice:ra`))[0], /status code="110"/);
    assert.match((await service.joinRoom(bob, `${room}/Bob:bob:rb`))[1], /status code="110"/);
    assert.match((await service.joinRoom(eve, `${room}/Eve:eve:re`))[0], /type="error"/);

    service.groupMessage(alice, room, "m1", "oi");
    assert.match(bob.sent.at(-1)!, /<body>oi<\/body>/);
});
