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

import { IdleStopPolicy, RuntimeLogOccupancy } from "./hubOccupancy";

function Edge(before: number, after: number, pid = 292): string {
    return `[12:00:00.000] pid=${pid} [NetConnEdge] tick=42 connCount ${before} -> ${after} connMax=16 NetDriver=0x1\n`;
}

function TempLog(): string {
    return path.join(fs.mkdtempSync(path.join(os.tmpdir(), "occupancy-")), "mysticparadox_dll_port8776.log");
}

test("follows the runtime's connection edges, from where the log ended when the hub started", () => {
    const File = TempLog();
    fs.writeFileSync(File, Edge(-2, 0, 100) + Edge(0, 3, 100));     // an earlier run with three players
    const Occupancy = new RuntimeLogOccupancy(File);
    Occupancy.begin();
    assert.equal(Occupancy.poll(), null, "nothing from this process yet: unknown");
    fs.appendFileSync(File, "[12:00:00.000] pid=292 [FlagCache] flag-file cache ON\n" + Edge(-2, 0));
    assert.equal(Occupancy.poll(), 0);
    fs.appendFileSync(File, Edge(0, 1) + Edge(1, 2));
    assert.equal(Occupancy.poll(), 2);
    fs.appendFileSync(File, Edge(2, 1) + "[12:00:01.000] pid=292 [NetConnEdge] tick=43 connC");   // a line still being written
    assert.equal(Occupancy.poll(), 1);
    fs.appendFileSync(File, "ount 1 -> 0 connMax=16 NetDriver=0x1\n");
    assert.equal(Occupancy.poll(), 0, "the split line is read whole");
    assert.equal(Occupancy.poll(), 0, "no new bytes: the last count stays");
});

test("an unreadable or missing log is unknown, and a truncated one is read again from its start", () => {
    const File = TempLog();
    const Occupancy = new RuntimeLogOccupancy(File);
    Occupancy.begin();
    assert.equal(Occupancy.poll(), null);
    fs.writeFileSync(File, "x".repeat(200) + "\n" + Edge(0, 2));
    assert.equal(Occupancy.poll(), 2);
    fs.writeFileSync(File, Edge(-2, 0));                             // replaced by a shorter file
    assert.equal(Occupancy.poll(), 0);
});

test("stops only after the idle time, counted from the last of empty, ready and the last request", () => {
    const Minute = 60_000;
    const Policy = new IdleStopPolicy(5 * Minute);
    assert.equal(Policy.shouldStop(0, 0, 0, 0), false, "just went empty");
    assert.equal(Policy.shouldStop(4 * Minute, 0, 0, 0), false);
    assert.equal(Policy.shouldStop(5 * Minute, 0, 0, 0), true);
    Policy.reset();
    assert.equal(Policy.shouldStop(10 * Minute, 0, 0, 0), false, "after a reset the clock starts again");
    assert.equal(Policy.shouldStop(16 * Minute, 0, 0, 12 * Minute), false, "someone asked for it 4 min ago");
    assert.equal(Policy.shouldStop(17 * Minute, 0, 0, 12 * Minute), true);
    Policy.reset();
    assert.equal(Policy.shouldStop(20 * Minute, 1, 0, 0), false, "a player is in");
    assert.equal(Policy.shouldStop(30 * Minute, null, 0, 0), false, "unknown never stops");
    assert.equal(Policy.shouldStop(31 * Minute, 0, 0, 0), false, "empty again: the clock starts now");
    assert.equal(Policy.shouldStop(36 * Minute, 0, 0, 0), true);
    assert.equal(new IdleStopPolicy(0).shouldStop(1e9, 0, 0, 0), false, "0 never stops");
});
