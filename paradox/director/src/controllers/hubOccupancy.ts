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

/*
 * [Dauntless Reforged Revival, patch 0033] How many players a hub has, and when an on-demand hub has been
 * empty long enough to stop.
 *
 * The Director does not see the game servers' connections. The runtime does, and writes every change of its
 * remote-client count to its own log, mysticparadox_dll_port<port>.log beside the game executable:
 *     [12:00:01.234] pid=292 [NetConnEdge] tick=1234 connCount 0 -> 1 connMax=... NetDriver=...
 * (the first one, "-2 -> 0", when the server starts listening). RuntimeLogOccupancy follows that file from
 * where it ended when the hub was started, so only the current process's lines count (under Wine the pid in
 * the log is not the one the Director knows). A count it has not read yet is unknown, and an unknown count
 * never stops anything.
 *
 * Pure apart from reading that one file; the clock is passed in.
 */

import fs from "node:fs";

const EDGE = /\[NetConnEdge\] tick=\d+ connCount (-?\d+) -> (-?\d+)/;
// More than this many new bytes between two reads: only the last part is read (the newest edge is all that counts).
const MAX_READ_BYTES = 4 * 1024 * 1024;

export class RuntimeLogOccupancy {
    private position = 0;
    private partial = "";
    private count: number | null = null;

    constructor(readonly file: string) {}

    // Called just before the hub's process starts: everything the log holds now belongs to earlier runs.
    begin(): void {
        this.position = FileSize(this.file);
        this.partial = "";
        this.count = null;
    }

    // Reads what the log gained since the last call. The newest connection count, or null before the first edge.
    poll(): number | null {
        const Size = FileSize(this.file);
        if (Size < this.position) {
            // The log was truncated or replaced: start over from its beginning.
            this.position = 0;
            this.partial = "";
        }
        if (Size === this.position) return this.count;
        let Start = this.position;
        if (Size - Start > MAX_READ_BYTES) {
            Start = Size - MAX_READ_BYTES;
            this.partial = "";
        }
        let Text: string;
        try {
            const Fd = fs.openSync(this.file, "r");
            try {
                const Bytes = Buffer.alloc(Size - Start);
                const Read = fs.readSync(Fd, Bytes, 0, Bytes.length, Start);
                Text = Bytes.subarray(0, Read).toString("latin1");
                this.position = Start + Read;
            } finally {
                fs.closeSync(Fd);
            }
        } catch {
            return this.count;
        }
        const Lines = (this.partial + Text).split("\n");
        this.partial = Lines.pop() ?? "";
        if (this.partial.length > 65536) this.partial = "";
        for (const Line of Lines) {
            const Match = EDGE.exec(Line);
            if (Match) {
                const After = Number(Match[2]);
                this.count = After < 0 ? null : After;
            }
        }
        return this.count;
    }
}

function FileSize(file: string): number {
    try {
        return fs.statSync(file).size;
    } catch {
        return 0;
    }
}

// When an on-demand hub has been empty long enough: no remote client for idleMs, counted from the latest of
// "the count went to 0", "the hub became ready" and "someone asked for the hub" (a player on the way in has
// no connection yet). 0 never stops.
export class IdleStopPolicy {
    private idleSince: number | null = null;

    constructor(readonly idleMs: number) {}

    reset(): void {
        this.idleSince = null;
    }

    shouldStop(now: number, count: number | null, readyAt: number, lastRequestAt: number): boolean {
        if (this.idleMs <= 0) return false;
        if (count === null || count > 0) {
            this.idleSince = null;
            return false;
        }
        if (this.idleSince === null) this.idleSince = now;
        return now - Math.max(this.idleSince, readyAt, lastRequestAt) >= this.idleMs;
    }
}
