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

// The rules behind the Reforged launcher API (routes/reforgedApi.ts), kept free of database and
// environment access so they can be tested on their own.

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import type { Request } from "express";

export const USER_KEY_HEADER = "x-reforged-user-api-key";
export const ADMIN_KEY_HEADER = "x-reforged-admin-key";
// Set by the Dauntless Reforged Revival gateway on every request it forwards. A request that
// carries it came from outside this machine, whatever its socket address says.
export const GATEWAY_HEADER = "x-reforged-gateway";

export type RegistrationMode = "OPEN" | "INVITECODE" | "NONE";

export function ReadRegistrationMode(Value: string | undefined): RegistrationMode {
    const Mode = (Value ?? "").trim().toUpperCase();
    return Mode === "OPEN" || Mode === "NONE" ? Mode : "INVITECODE";
}

// ------------------------------------------------------------------ names, codes and keys

// Usernames: 3-16 letters, digits and underscores, unique regardless of case (as in Dauntless Revived).
export const USERNAME_RULE = /^[A-Za-z0-9_]{3,16}$/;

export function CleanUsername(Value: unknown): string | undefined {
    if (typeof Value !== "string") return undefined;
    const Trimmed = Value.trim();
    return USERNAME_RULE.test(Trimmed) ? Trimmed : undefined;
}

export function IsValidInviteCode(Value: unknown): Value is string {
    return typeof Value === "string" && /^[A-Za-z0-9-]{4,64}$/.test(Value);
}

// Twelve characters from an alphabet without look-alikes (no 0/O, 1/I/L), in three groups: about
// 60 random bits, which no one guesses through the gateway's rate limit.
const INVITE_ALPHABET = "ABCDEFGHJKMNPQRSTUVWXYZ23456789";

export function GenerateInviteCode(Random: (Max: number) => number = (Max) => crypto.randomInt(Max)): string {
    const Groups: string[] = [];
    for (let G = 0; G < 3; G++) {
        let Group = "";
        for (let I = 0; I < 4; I++) Group += INVITE_ALPHABET[Random(INVITE_ALPHABET.length)];
        Groups.push(Group);
    }
    return Groups.join("-");
}

// Account keys: RRK_ and 48 hex characters (192 random bits). Plain characters only: the key goes
// on the game's command line as -AUTH_PASSWORD.
export function GenerateAccountKey(): string {
    return `RRK_${crypto.randomBytes(24).toString("hex")}`;
}

export function IsPlausibleAccountKey(Value: unknown): Value is string {
    return typeof Value === "string" && /^[A-Za-z0-9_-]{8,128}$/.test(Value);
}

export const MAX_INVITE_USES = 1000;

export function CleanInviteNote(Value: unknown): string | undefined {
    if (typeof Value !== "string") return undefined;
    const Text = Value.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ").trim().slice(0, 64);
    return Text.length > 0 ? Text : undefined;
}

type AccountError = "username_invalid" | "registration_closed" | "bad_request" | "invite_invalid" | "username_taken" | "not_found";

const MESSAGES: Record<AccountError, string> = {
    username_invalid: "Usernames are 3-16 characters: letters, numbers and underscore.",
    registration_closed: "Registration is closed on this server.",
    bad_request: "The request body is not JSON, or a field is missing or of the wrong type.",
    invite_invalid: "That invite code is not valid or has already been used.",
    username_taken: "That username is taken.",
    not_found: "No such account."
};

export function ErrorBody(Error: AccountError) {
    return { error: Error, message: MESSAGES[Error] };
}

// ------------------------------------------------------------------ who is asking

function IsLoopbackAddress(Address: string | undefined): boolean {
    if (!Address) return false;
    return Address === "::1" || /^(::ffff:)?127\.\d+\.\d+\.\d+$/.test(Address);
}

// Admin routes: a direct connection from this machine (not forwarded by the gateway) with the
// configured admin key, compared in constant time.
export function IsLocalAdminRequest(Req: Pick<Request, "headers" | "socket">, AdminKey: string | undefined): boolean {
    if (!AdminKey || AdminKey.length < 32) return false;
    if (!IsLoopbackAddress(Req.socket?.remoteAddress)) return false;
    if (Req.headers[GATEWAY_HEADER] !== undefined || Req.headers["x-forwarded-for"] !== undefined) return false;
    const Given = Req.headers[ADMIN_KEY_HEADER];
    if (typeof Given !== "string") return false;
    const A = crypto.createHash("sha256").update(Given, "utf8").digest();
    const B = crypto.createHash("sha256").update(AdminKey, "utf8").digest();
    return crypto.timingSafeEqual(A, B);
}


// ------------------------------------------------------------------ server status

export type PlayerWhere = "menu" | "city" | "hunt" | "dojo" | "tutorial" | "unknown";

export type StatusInstance = {
    id: string;
    kind: "city" | "hunt" | "dojo" | "tutorial";
    title: string;
    map: string;
    behemoth: string | null;
    players: number;
    maxPlayers: number;
    startedAt: string | null;
};

type DirectorInstance = {
    id?: unknown;
    kind?: unknown;
    map?: unknown;
    behemoth?: unknown;
    players?: unknown;
    maxPlayers?: unknown;
    playerIds?: unknown;
    startedAt?: unknown;
};

// The Director's GET /api/instances answer, checked field by field (it is a local service, but a
// status page must never fail because of one odd entry).
export function ParseDirectorInstances(Raw: unknown): { instances: StatusInstance[]; huntPlayers: Map<string, string> } {
    const Instances: StatusInstance[] = [];
    const HuntPlayers = new Map<string, string>();
    if (!Array.isArray(Raw)) return { instances: Instances, huntPlayers: HuntPlayers };
    for (const Entry of Raw.slice(0, 100) as DirectorInstance[]) {
        if (typeof Entry !== "object" || Entry === null) continue;
        const Id = typeof Entry.id === "string" ? Entry.id.slice(0, 64) : undefined;
        const Kind = Entry.kind === "city" || Entry.kind === "hunt" || Entry.kind === "dojo" || Entry.kind === "tutorial" ? Entry.kind : undefined;
        if (!Id || !Kind) continue;
        const Map = typeof Entry.map === "string" ? Entry.map.slice(0, 160) : "";
        const Behemoth = typeof Entry.behemoth === "string" ? Entry.behemoth.slice(0, 160) : null;
        const Players = typeof Entry.players === "number" && Number.isFinite(Entry.players) ? Math.max(0, Math.floor(Entry.players)) : 0;
        const MaxPlayers = typeof Entry.maxPlayers === "number" && Number.isFinite(Entry.maxPlayers) ? Math.max(0, Math.floor(Entry.maxPlayers)) : 0;
        const StartedAt = typeof Entry.startedAt === "string" && Number.isFinite(Date.parse(Entry.startedAt)) ? new Date(Entry.startedAt).toISOString() : null;
        Instances.push({
            id: Id,
            kind: Kind,
            title: InstanceTitle(Kind, Behemoth, Map),
            map: ShortAssetName(Map),
            behemoth: Behemoth ? ShortAssetName(Behemoth) : null,
            players: Players,
            maxPlayers: MaxPlayers,
            startedAt: StartedAt
        });
        if (Kind === "hunt" && Array.isArray(Entry.playerIds)) {
            for (const PlayerId of Entry.playerIds.slice(0, 16)) {
                if (typeof PlayerId === "string" && PlayerId.length <= 128) HuntPlayers.set(PlayerId, Id);
            }
        }
    }
    return { instances: Instances, huntPlayers: HuntPlayers };
}

// "/Game/Maps/islands/foo/foo_persistent" -> "foo_persistent"; "/Game/Behemoths/Skarn.Skarn_C" -> "Skarn"
export function ShortAssetName(Asset: string): string {
    const Last = Asset.split("/").pop() ?? Asset;
    return Last.split(".")[0].slice(0, 64);
}

export function InstanceTitle(Kind: StatusInstance["kind"], Behemoth: string | null, Map: string): string {
    if (Kind === "city") return "Ramsgate";
    if (Kind === "dojo") return "Training Grounds";
    if (Kind === "tutorial") return "Tutorial";
    if (Behemoth) return ShortAssetName(Behemoth).replace(/^BP_|^BPC_|_C$/g, "").replace(/_/g, " ").slice(0, 64);
    return ShortAssetName(Map).replace(/_persistent$/i, "").replace(/_/g, " ").slice(0, 64) || "Hunt";
}

// Which game data the services run on: "real" when every required file exists and none is the
// shipped synthetic example, "synthetic" when the examples are in use (startup tests only, not
// playable), "missing" otherwise. The launcher shows it, so players are told before they try.
export type GameDataKind = "real" | "synthetic" | "missing";

export const BACKEND_GAME_DATA_FILES = [
    "progression_config.json",
    "slayers_path.json",
    "slayers_path_definitions.json",
    "ladyluck_store.json",
    "platinum_store.json",
    "reward_cache_store.json",
    "inventory_storage_policy.json"
];

export function DetectGameData(Dir: string, Files: readonly string[] = BACKEND_GAME_DATA_FILES): GameDataKind {
    let Synthetic = false;
    for (const Name of Files) {
        const Full = path.join(Dir, Name);
        let Data: Buffer;
        try {
            Data = fs.readFileSync(Full);
        } catch {
            return "missing";
        }
        const Example = path.join(Dir, Name.replace(/\.json$/, ".example.json"));
        try {
            if (fs.readFileSync(Example).equals(Data)) Synthetic = true;
        } catch {
            /* no example next to it */
        }
    }
    return Synthetic ? "synthetic" : "real";
}

