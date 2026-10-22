/*
 * Copyright (C) 2026 Dauntless Reforged Revival contributors
 *
 * Licensed under the GNU Affero General Public License v3.0.
 * You may obtain a copy of the License at the root of this repository.
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * Part of the Dauntless Reforged Revival patch set for Mystic Paradox (see upstream/README.md in
 * the Dauntless Reforged Revival repository). This file is new; it carries no Mystic Paradox code.
 */

import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { HuntMemoryUnavailableError, MemoryAdmission, MemoryAdmissionFromEnv, ReadAvailableMemoryBytes } from "./memoryAdmission";

const MB = 1024 * 1024;

function Guard(availableMb: number | null, clock = { now: 0 }) {
    return new MemoryAdmission({
        enabled: true,
        minFreeMb: 1024,
        huntMb: 1536,
        holdMs: 120_000,
        available: () => (availableMb === null ? null : availableMb * MB),
        now: () => clock.now,
    });
}

test("a hunt starts only when one more hunt fits and the floor stays free", () => {
    assert.doesNotThrow(() => Guard(2560).reserve());
    assert.throws(() => Guard(2559).reserve(), HuntMemoryUnavailableError);
});

test("reservations add up, lapse after the hold, and can be given back early", () => {
    const Clock = { now: 0 };
    const G = Guard(5000, Clock);                   // room for 5000 - 1024 = 3976 MB of hunts: two reservations
    G.reserve();
    const Second = G.reserve();
    assert.equal(G.status().reservedMb, 3072);
    assert.throws(() => G.reserve(), HuntMemoryUnavailableError);
    Second();                                       // a start that failed gives its memory back
    assert.doesNotThrow(() => G.reserve());
    Clock.now = 120_000;                            // both lapse: the processes have allocated it by now
    assert.equal(G.status().reservedMb, 0);
});

test("off, or memory it cannot read, never blocks a hunt", () => {
    const Off = new MemoryAdmission({ enabled: false, minFreeMb: 1024, huntMb: 1536, holdMs: 1, available: () => 0 });
    assert.doesNotThrow(() => Off.reserve());
    assert.doesNotThrow(() => Guard(null).reserve());
});

test("settings come from the environment, and a bad value keeps its default", () => {
    assert.equal(MemoryAdmissionFromEnv({}).admission.enabled, false, "off unless GAMESERVER_MEMORY_GUARD=1");
    const On = MemoryAdmissionFromEnv({ GAMESERVER_MEMORY_GUARD: "1", GAMESERVER_MIN_FREE_MB: "512", GAMESERVER_HUNT_MEMORY_MB: "2048" });
    assert.equal(On.admission.enabled, true);
    assert.deepEqual(On.problems, []);
    assert.equal(On.admission.status().neededMb, 2560);
    const Bad = MemoryAdmissionFromEnv({ GAMESERVER_MEMORY_GUARD: "1", GAMESERVER_MIN_FREE_MB: "lots" });
    assert.equal(Bad.problems.length, 1);
    assert.equal(Bad.admission.status().neededMb, 1024 + 1536);
});

test("reads Linux's MemAvailable, and falls back to os.freemem()", () => {
    const Dir = fs.mkdtempSync(path.join(os.tmpdir(), "meminfo-"));
    const File = path.join(Dir, "meminfo");
    fs.writeFileSync(File, "MemTotal:       16384000 kB\nMemFree:          512000 kB\nMemAvailable:    8192000 kB\n");
    assert.equal(ReadAvailableMemoryBytes(File, "linux"), 8192000 * 1024);
    const Fallback = ReadAvailableMemoryBytes(path.join(Dir, "missing"), "linux");
    assert.ok(Fallback === null || Fallback > 0);
    fs.rmSync(Dir, { recursive: true, force: true });
});
