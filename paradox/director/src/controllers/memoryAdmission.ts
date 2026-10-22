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
 * [Dauntless Reforged Revival, patch 0022] Memory admission for hunt servers, after Dauntless Revived's
 * MemoryAdmission (UndauntedDeployServer/src/controllers/capacity.ts).
 *
 * Every hunt is a whole game process (about 1-1.5 GB, more under Wine), and MAX_CONCURRENT_HUNT_SERVERS
 * alone does not know how much memory the machine has left: on a small server the last hunts it allows
 * can run the machine out of memory, and then the kernel kills whatever it picks (Ramsgate, MongoDB, ...).
 *
 * With GAMESERVER_MEMORY_GUARD=1 a new hunt starts only when the memory available now, minus what the
 * hunts started in the last GAMESERVER_MEMORY_RESERVATION_SECONDS still have to allocate, covers one more
 * hunt (GAMESERVER_HUNT_MEMORY_MB) and leaves GAMESERVER_MIN_FREE_MB free. Otherwise the request is
 * refused like a full hunt cap (no process is started); running hunts and the hubs are never touched.
 * "Available" is Linux's MemAvailable (what can be had without swapping), elsewhere os.freemem().
 * Without GAMESERVER_MEMORY_GUARD=1 nothing changes.
 *
 * Pure: the memory reader and the clock are injected, so the tests need no real memory pressure.
 */

import fs from "node:fs";
import os from "node:os";

const MB = 1024 * 1024;

export type MemoryAdmissionOptions = {
    enabled: boolean;
    minFreeMb: number;
    huntMb: number;
    holdMs: number;
    // Bytes available now; null when unknown.
    available?: () => number | null;
    now?: () => number;
};

export type MemoryAdmissionStatus = {
    enabled: boolean;
    availableMb: number | null;
    reservedMb: number;
    neededMb: number;
};

export class HuntMemoryUnavailableError extends Error {
    constructor(public readonly status: MemoryAdmissionStatus) {
        super(`Not enough free memory for another hunt server (${status.availableMb ?? "?"} MB available, ${status.reservedMb} MB reserved for hunts still starting, ${status.neededMb} MB needed)`);
        this.name = "HuntMemoryUnavailableError";
    }
}

// MemAvailable from /proc/meminfo on Linux; os.freemem() elsewhere (or when /proc cannot be read).
export function ReadAvailableMemoryBytes(meminfoFile: string = "/proc/meminfo", platform: NodeJS.Platform = process.platform): number | null {
    if (platform === "linux") {
        try {
            const Match = /^MemAvailable:\s+(\d+)\s+kB\s*$/m.exec(fs.readFileSync(meminfoFile, "utf8"));
            if (Match) return Number(Match[1]) * 1024;
        } catch {
            /* fall back to os.freemem() */
        }
    }
    const Free = os.freemem();
    return Number.isFinite(Free) && Free > 0 ? Free : null;
}

export class MemoryAdmission {
    private readonly reservations = new Map<symbol, { until: number; bytes: number }>();
    private readonly available: () => number | null;
    private readonly now: () => number;

    constructor(private readonly options: MemoryAdmissionOptions) {
        this.available = options.available ?? (() => ReadAvailableMemoryBytes());
        this.now = options.now ?? (() => Date.now());
    }

    get enabled(): boolean { return this.options.enabled; }

    status(): MemoryAdmissionStatus {
        const Now = this.now();
        for (const [Key, Reservation] of this.reservations) if (Reservation.until <= Now) this.reservations.delete(Key);
        let Reserved = 0;
        for (const Reservation of this.reservations.values()) Reserved += Reservation.bytes;
        const Available = this.options.enabled ? this.available() : null;
        return {
            enabled: this.options.enabled,
            availableMb: Available === null ? null : Math.floor(Available / MB),
            reservedMb: Math.round(Reserved / MB),
            neededMb: this.options.huntMb + this.options.minFreeMb,
        };
    }

    // Reserves one hunt's memory, or throws HuntMemoryUnavailableError. The returned function gives the
    // reservation back early (a start that failed); otherwise it lapses after holdMs. Unknown available
    // memory admits the hunt: the guard never blocks a server it cannot measure.
    reserve(): () => void {
        if (!this.options.enabled) return () => {};
        const Status = this.status();
        if (Status.availableMb !== null && Status.availableMb - Status.reservedMb < Status.neededMb) {
            throw new HuntMemoryUnavailableError(Status);
        }
        const Token = Symbol("hunt");
        this.reservations.set(Token, { until: this.now() + this.options.holdMs, bytes: this.options.huntMb * MB });
        return () => { this.reservations.delete(Token); };
    }
}

// The settings from the environment. A value that is not a whole number in range keeps its default and is
// reported in `problems` (logged by the caller), so a typo never stops the Director.
export function MemoryAdmissionFromEnv(env: NodeJS.ProcessEnv = process.env): { admission: MemoryAdmission; problems: string[] } {
    const Problems: string[] = [];
    const Read = (Name: string, Fallback: number, Min: number, Max: number): number => {
        const Raw = env[Name]?.trim();
        if (Raw === undefined || Raw === "") return Fallback;
        const Value = Number(Raw);
        if (!/^\d+$/.test(Raw) || !Number.isSafeInteger(Value) || Value < Min || Value > Max) {
            Problems.push(`${Name}='${Raw}' is not a whole number from ${Min} to ${Max}; using ${Fallback}`);
            return Fallback;
        }
        return Value;
    };
    const Enabled = /^(1|true|yes|on)$/i.test((env.GAMESERVER_MEMORY_GUARD ?? "").trim());
    const admission = new MemoryAdmission({
        enabled: Enabled,
        minFreeMb: Read("GAMESERVER_MIN_FREE_MB", 1024, 0, 1048576),
        huntMb: Read("GAMESERVER_HUNT_MEMORY_MB", 1536, 1, 1048576),
        holdMs: Read("GAMESERVER_MEMORY_RESERVATION_SECONDS", 120, 0, 3600) * 1000,
    });
    return { admission, problems: Problems };
}
