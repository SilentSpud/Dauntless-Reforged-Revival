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

// The launcher API of a Dauntless Reforged Revival server, modelled on Dauntless Revived's
// /undaunted/api: invite codes, one account key per player, and a live server status.
//
//   POST /reforged/api/Register            {Username, InviteCode} -> 200 {UUK}
//   GET  /reforged/api/GetUserInfo         x-reforged-user-api-key -> {UserId, Username, IsAdmin}
//   GET  /reforged/api/ServerStatus        name, registration, players, instances, game build, game data
//   GET  /reforged/api/RegistrationStatus  {mode}
//   GET  /dauntless-status                 {"show-status": true, name, version, sourceUrl}
//
// Admin routes (only from this machine, never through the gateway, with x-reforged-admin-key):
//   POST   /reforged/api/CreateInvite      {uses?, name?} -> {code}
//   GET    /reforged/api/InviteCodes
//   DELETE /reforged/api/InviteCode/:code
//   POST   /reforged/api/SetAccountStatus  {Username, Status: active|banned|disabled, Reason?}
//
// The account key is the game's AUTH_PASSWORD: with AUTH_MODE=APIKEY the game trades it for its
// session token at /account/api/oauth/token (routes/eos.ts). Keys are stored only as HMACs
// (security/apiKeyHash.ts). Accounts are ordinary launcher accounts (displayName, status), so the
// existing eligibility checks, ban handling and name lookups apply to them unchanged.

import crypto from "node:crypto";
import path from "node:path";
import { Router, Request, Response, NextFunction } from "express";
import { MongoServerError } from "mongodb";
import { logger } from "../logger";
import { GetRepositories, LauncherAccountRecord, AccountOperationalStatus } from "../persistence";
import { GetMongoClient, GetMongoDb } from "../persistence/mongo/client";
import { Collections } from "../persistence/mongo/collections";
import { GetUserIDForAPIKey } from "../controllers/auth";
import { HashApiKey } from "../security/apiKeyHash";
import { IsAccountEligible } from "../security/accountEligibility";
import { sessionRegistry } from "../realtime/SessionRegistry";
import {
    CleanInviteNote,
    CleanUsername,
    DetectGameData,
    ErrorBody,
    GameDataKind,
    GenerateAccountKey,
    GenerateInviteCode,
    IsLocalAdminRequest,
    IsPlausibleAccountKey,
    IsValidInviteCode,
    MAX_INVITE_USES,
    ParseDirectorInstances,
    PlayerWhere,
    ReadRegistrationMode,
    USER_KEY_HEADER
} from "../reforgedApiRules";

const INVITES_COLLECTION = "reforgedInvites";
const STARTED_AT = Date.now();

function HasAdminKey(Req: Request, Res: Response, Next: NextFunction) {
    if (!IsLocalAdminRequest(Req, process.env.REFORGED_ADMIN_KEY)) {
        Res.status(403).json({ error: "forbidden" });
        return;
    }
    Next();
}

type KeyOwner = { UserId: string; Username: string; IsAdmin: boolean; Account: LauncherAccountRecord | undefined };

async function LookUpKey(Key: unknown): Promise<KeyOwner | undefined> {
    if (!IsPlausibleAccountKey(Key)) return undefined;
    const UserId = await GetUserIDForAPIKey(Key);
    if (UserId == undefined) return undefined;
    const Account = await GetRepositories().launcherAccounts.findByUserId(UserId);
    const Username = Account?.displayName ?? (await GetRepositories().accounts.findByUserId(UserId))?.name ?? UserId;
    return { UserId, Username, IsAdmin: Account?.roles.includes("admin") === true, Account };
}

// ------------------------------------------------------------------ invites

type InviteDoc = {
    _id: string;
    usesRemaining: number;
    infiniteUses: boolean;
    note?: string;
    createdAt: string;
};

async function Invites() {
    const Db = await GetMongoDb();
    return Db.collection<InviteDoc>(INVITES_COLLECTION);
}

let GameDataCache: { at: number; kind: GameDataKind } | undefined;

function GameDataKindNow(): GameDataKind {
    if (GameDataCache && Date.now() - GameDataCache.at < 60_000) return GameDataCache.kind;
    const Dir = process.env.PARADOX_GAME_DATA_DIR
        ? path.resolve(process.env.PARADOX_GAME_DATA_DIR)
        : path.join(__dirname, "..", "..", "game-data");
    const Kind = DetectGameData(Dir);
    GameDataCache = { at: Date.now(), kind: Kind };
    return Kind;
}

function CleanHttpsUrl(Value: string | undefined): string | null {
    if (!Value) return null;
    try {
        const Url = new URL(Value.trim());
        return Url.protocol === "https:" && Url.username === "" && Url.password === "" ? Url.toString() : null;
    } catch {
        return null;
    }
}

function ServerName(): string {
    const Name = (process.env.REFORGED_SERVER_NAME ?? "").replace(/[\u0000-\u001f\u007f-\u009f]/g, " ").trim();
    return Name.length > 0 ? Name.slice(0, 64) : "Dauntless Reforged Revival server";
}

function ServerVersion(): string {
    return (process.env.REFORGED_SERVER_VERSION ?? "").trim().slice(0, 48) || "0.1.0";
}

async function FetchDirectorInstances(): Promise<unknown> {
    const Director = (process.env.DEPLOYSERVER_URL ?? "").trim();
    if (!Director) return [];
    const Controller = new AbortController();
    const Timer = setTimeout(() => Controller.abort(), 2000);
    try {
        const Answer = await fetch(`http://${Director}/api/instances`, { signal: Controller.signal });
        return Answer.ok ? await Answer.json() : [];
    } catch {
        return [];
    } finally {
        clearTimeout(Timer);
    }
}

type StatusView = "full" | "limited";
const StatusCache = new Map<StatusView, { at: number; body: unknown }>();

export async function BuildServerStatus(View: StatusView) {
    const Cached = StatusCache.get(View);
    if (Cached && Date.now() - Cached.at < 5000) return Cached.body;

    const Base = {
        name: ServerName(),
        online: true,
        version: ServerVersion(),
        commit: (process.env.REFORGED_SOURCE_COMMIT ?? "").trim().slice(0, 64),
        sourceUrl: CleanHttpsUrl(process.env.REFORGED_SOURCE_URL),
        registration: ReadRegistrationMode(process.env.REGISTRATION_MODE),
        uptimeSeconds: Math.floor((Date.now() - STARTED_AT) / 1000),
        contentPort: null,
        game: {
            build: "rel-1.14.7-Archon",
            changelist: Number(process.env.TARGET_CHANGELIST ?? "647472") || 647472,
            executableSha256: /^[0-9a-f]{64}$/i.test(process.env.APPROVED_EXECUTABLE_SHA256 ?? "")
                ? (process.env.APPROVED_EXECUTABLE_SHA256 as string).toLowerCase()
                : null
        },
        gameData: GameDataKindNow(),
        chat: process.env.REALTIME_XMPP_ENABLED === "true"
    };

    let Body: unknown;
    if (View === "limited") {
        Body = { ...Base, playersOnline: 0, players: [], instances: [], limited: true };
    } else {
        const { instances, huntPlayers } = ParseDirectorInstances(await FetchDirectorInstances());
        const Online = sessionRegistry.onlineAccountIds().slice(0, 500);
        const Repos = GetRepositories();
        const Players: { name: string; where: PlayerWhere; instance: string | null }[] = [];
        for (const UserId of Online) {
            const Account = await Repos.launcherAccounts.findByUserId(UserId).catch(() => undefined);
            const Name = Account?.displayName ?? (await Repos.accounts.findByUserId(UserId).catch(() => undefined))?.name;
            if (!Name) continue;
            const Hunt = huntPlayers.get(UserId);
            Players.push({ name: Name.slice(0, 32), where: Hunt ? "hunt" : "unknown", instance: Hunt ?? null });
        }
        Players.sort((A, B) => A.name.localeCompare(B.name, undefined, { sensitivity: "base" }));
        Body = { ...Base, playersOnline: Players.length, players: Players, instances, limited: false };
    }
    StatusCache.set(View, { at: Date.now(), body: Body });
    return Body;
}

// ------------------------------------------------------------------ routes

export const reforgedApiRouter = Router();

class RegisterRefusal extends Error {
    constructor(public readonly code: "invite_invalid" | "username_taken") {
        super(code);
        this.name = "RegisterRefusal";
    }
}

reforgedApiRouter.post("/reforged/api/Register", async (req, res) => {
    const Mode = ReadRegistrationMode(process.env.REGISTRATION_MODE);
    if (Mode === "NONE") {
        res.status(400).json(ErrorBody("registration_closed"));
        return;
    }
    const Body = req.body != null && typeof req.body === "object" ? req.body : {};
    if (typeof Body.Username !== "string" || (Body.InviteCode !== undefined && typeof Body.InviteCode !== "string")) {
        res.status(400).json(ErrorBody("bad_request"));
        return;
    }
    const Username = CleanUsername(Body.Username);
    if (Username === undefined) {
        res.status(400).json(ErrorBody("username_invalid"));
        return;
    }
    const InviteCode = typeof Body.InviteCode === "string" ? Body.InviteCode.trim() : "";
    if (Mode === "INVITECODE" && !IsValidInviteCode(InviteCode)) {
        res.status(401).json(ErrorBody("invite_invalid"));
        return;
    }

    const UserId = crypto.randomUUID();
    const Key = GenerateAccountKey();
    const Account: LauncherAccountRecord = {
        userId: UserId,
        displayName: Username,
        displayNameNormalized: Username.toLowerCase(),
        status: "active",
        approvalStatus: "approved",
        roles: ["player"],
        createdAt: new Date().toISOString(),
        usernameSet: true
    };

    // One transaction: a taken name never costs the friend a use of their code, and an account is
    // never written without its key. A refusal is thrown, which rolls the transaction back.
    const Client = await GetMongoClient();
    const Session = Client.startSession();
    try {
        await Session.withTransaction(async () => {
            const Db = await GetMongoDb();
            if (Mode === "INVITECODE") {
                const Used = await Db.collection<InviteDoc>(INVITES_COLLECTION).findOneAndUpdate(
                    { _id: InviteCode, $or: [{ infiniteUses: true }, { usesRemaining: { $gt: 0 } }] },
                    [{ $set: { usesRemaining: { $cond: ["$infiniteUses", "$usesRemaining", { $subtract: ["$usesRemaining", 1] }] } } }],
                    { session: Session }
                );
                if (Used == undefined) throw new RegisterRefusal("invite_invalid");
            }
            try {
                await GetRepositories().launcherAccounts.create(Account, Session);
            } catch (Problem) {
                if (Problem instanceof MongoServerError && Problem.code === 11000) throw new RegisterRefusal("username_taken");
                throw Problem;
            }
            await Db.collection(Collections.UserApiKeys).insertOne(
                { _id: UserId as any, userId: UserId, keyHash: HashApiKey(Key, "user") },
                { session: Session }
            );
        });
    } catch (Problem) {
        if (Problem instanceof RegisterRefusal) {
            res.status(Problem.code === "invite_invalid" ? 401 : 409).json(ErrorBody(Problem.code));
            return;
        }
        logger.error(`[reforged] registration failed: ${Problem instanceof Error ? Problem.message : String(Problem)}`);
        res.status(500).send();
        return;
    } finally {
        await Session.endSession();
    }

    logger.info(`[reforged] registered ${UserId} as ${Username}`);
    StatusCache.clear();
    res.status(200).json({ UUK: Key });
});

reforgedApiRouter.get("/reforged/api/GetUserInfo", async (req, res) => {
    const Owner = await LookUpKey(req.headers[USER_KEY_HEADER]).catch(() => undefined);
    if (Owner === undefined) {
        res.status(401).json({ error: "unauthorized" });
        return;
    }
    if (Owner.Account && !IsAccountEligible(Owner.Account)) {
        res.status(403).json({
            error: Owner.Account.status === "banned" ? "account_banned" : "account_disabled",
            reason: (Owner.Account.approvalReason ?? "").slice(0, 500) || "Contact the server's host for details."
        });
        return;
    }
    res.status(200).json({ UserId: Owner.UserId, Username: Owner.Username, IsAdmin: Owner.IsAdmin });
});

reforgedApiRouter.get("/reforged/api/ServerStatus", async (req, res) => {
    const Owner = await LookUpKey(req.headers[USER_KEY_HEADER]).catch(() => undefined);
    const Registered = Owner !== undefined && (Owner.Account === undefined || IsAccountEligible(Owner.Account));
    try {
        const Body = await BuildServerStatus(Registered ? "full" : "limited");
        res.status(200);
        res.set("Cache-Control", "no-store");
        res.vary(USER_KEY_HEADER);
        res.json(Body);
    } catch (Problem) {
        logger.error(`[reforged] status failed: ${Problem instanceof Error ? Problem.message : String(Problem)}`);
        res.status(500).send();
    }
});

reforgedApiRouter.get("/reforged/api/RegistrationStatus", (_req, res) => {
    res.status(200).json({ mode: ReadRegistrationMode(process.env.REGISTRATION_MODE) });
});

// The legacy probe older launchers use to tell "a server of this kind" from "something else".
reforgedApiRouter.get("/dauntless-status", (_req, res) => {
    res.status(200).json({ "show-status": true, name: ServerName(), version: ServerVersion(), sourceUrl: CleanHttpsUrl(process.env.REFORGED_SOURCE_URL) });
});

reforgedApiRouter.post("/reforged/api/CreateInvite", HasAdminKey, async (req, res) => {
    const Body = req.body != null && typeof req.body === "object" ? req.body : {};
    const Uses = Body.uses ?? 1;
    const Infinite = Body.infinite === true;
    if (!Number.isSafeInteger(Uses) || Uses < 1 || Uses > MAX_INVITE_USES || (Body.name !== undefined && typeof Body.name !== "string")) {
        res.status(400).json({ error: "bad_request", message: `uses must be a whole number from 1 to ${MAX_INVITE_USES}, and name a string` });
        return;
    }
    const Collection = await Invites();
    for (let Attempt = 0; Attempt < 5; Attempt++) {
        const Code = GenerateInviteCode();
        try {
            await Collection.insertOne({ _id: Code, usesRemaining: Uses, infiniteUses: Infinite, note: CleanInviteNote(Body.name), createdAt: new Date().toISOString() });
            logger.info(`[reforged] invite created (${Infinite ? "unlimited" : `${Uses} use(s)`})${Body.name ? ` for ${CleanInviteNote(Body.name)}` : ""}`);
            res.status(200).json({ code: Code });
            return;
        } catch (Problem) {
            if (!(Problem instanceof MongoServerError && Problem.code === 11000)) throw Problem;
        }
    }
    res.status(500).send();
});

reforgedApiRouter.get("/reforged/api/InviteCodes", HasAdminKey, async (_req, res) => {
    const Collection = await Invites();
    const Docs = await Collection.find({}).sort({ createdAt: -1 }).limit(500).toArray();
    res.status(200).json({
        InviteCodes: Docs.map((Doc) => ({ code: Doc._id, usesRemaining: Doc.usesRemaining, infiniteUses: Doc.infiniteUses, note: Doc.note ?? null, createdAt: Doc.createdAt }))
    });
});

reforgedApiRouter.delete("/reforged/api/InviteCode/:code", HasAdminKey, async (req, res) => {
    const Code = String(req.params.code ?? "");
    if (!IsValidInviteCode(Code)) {
        res.status(400).json({ error: "bad_request" });
        return;
    }
    const Collection = await Invites();
    const Result = await Collection.deleteOne({ _id: Code });
    res.status(Result.deletedCount === 1 ? 200 : 404).send();
});

reforgedApiRouter.post("/reforged/api/SetAccountStatus", HasAdminKey, async (req, res) => {
    const Body = req.body != null && typeof req.body === "object" ? req.body : {};
    const Username = CleanUsername(Body.Username);
    const Status = Body.Status as AccountOperationalStatus;
    if (Username === undefined || !["active", "banned", "disabled"].includes(Status)) {
        res.status(400).json({ error: "bad_request", message: "Username and Status (active, banned or disabled) are required." });
        return;
    }
    const Repos = GetRepositories();
    const Account = await Repos.launcherAccounts.findByDisplayNameNormalized(Username.toLowerCase());
    if (Account === undefined) {
        res.status(404).json(ErrorBody("not_found"));
        return;
    }
    const Updated = await Repos.launcherAccounts.setAccessState(Account.userId, {
        status: Status,
        approvalUpdatedAt: new Date().toISOString(),
        approvalUpdatedBy: "reforged-admin",
        approvalReason: typeof Body.Reason === "string" ? Body.Reason.slice(0, 500) : undefined
    });
    StatusCache.clear();
    logger.info(`[reforged] ${Account.userId} (${Account.displayName}) is now ${Status}`);
    res.status(200).json({ UserId: Account.userId, Username: Account.displayName, Status: Updated?.status ?? Status });
});
