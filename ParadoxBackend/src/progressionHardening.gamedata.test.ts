import assert from "node:assert/strict";
import test from "node:test";
import { MongoPlayerJourneyRepository } from "./persistence/mongo/repositories/MongoPlayerJourneyRepository";
import { MongoProgressionGrantRepository } from "./persistence/mongo/repositories/MongoProgressionGrantRepository";
import {
    APPROVED_XP_PROGRESSION_TRACK_IDS,
    BuildProgressionGrantRequestHash,
    GrantProgressionXp,
    IsApprovedXpProgressionTrack,
    IsValidProgressionGrantId,
    MergePlayerJourneyNodes,
    IsAuthorizedProgressionReader,
    IsAuthorizedProgressionReporter,
    IsCharacterOwnedByUser
} from "./controllers/progression";
import { CaptureProgressionObjectiveEvent, GetTrackCurve, PrestigeExperienceTrack, PrestigeTrackFor, RankForProgress } from "./controllers/progression";

const Track = {
    userId: "user-1",
    progressionId: "ExperienceTrack_PlayerLevel",
    progress: 125,
    confirmedFremiumRank: 0,
    confirmedPremiumRank: 0,
    updateVersion: 1,
    createdAt: "2026-08-30T00:00:00.000Z",
    updatedAt: "2026-08-30T00:00:00.000Z"
};

test("progression grant request hashes are canonical and bind every mutation field", () => {
    const First = BuildProgressionGrantRequestHash("user-1", "ExperienceTrack_PlayerLevel", 125);
    assert.equal(First, BuildProgressionGrantRequestHash("user-1", "ExperienceTrack_PlayerLevel", 125));
    assert.notEqual(First, BuildProgressionGrantRequestHash("user-1", "ExperienceTrack_PlayerLevel", 126));
    assert.notEqual(First, BuildProgressionGrantRequestHash("user-2", "ExperienceTrack_PlayerLevel", 125));
    assert.equal(IsValidProgressionGrantId("hunt-2026-08-30:0001"), true);
    assert.equal(IsValidProgressionGrantId(""), false);
    assert.equal(IsValidProgressionGrantId("has spaces"), false);
});

test("XP mutation allowlist excludes config-only historical/test tracks", () => {
    assert.equal(IsApprovedXpProgressionTrack("ExperienceTrack_PlayerLevel"), true);
    assert.equal(IsApprovedXpProgressionTrack("single_track"), false);
    assert.equal(IsApprovedXpProgressionTrack("test_minipass01"), false);
    assert.equal(APPROVED_XP_PROGRESSION_TRACK_IDS.includes("MasteryTrack_Behemoth"), true);
});

test("progression route authority cannot be widened by player-host identity", () => {
    assert.equal(IsAuthorizedProgressionReporter({ userId: "user-1" }, "user-1"), true);
    assert.equal(IsAuthorizedProgressionReporter({ userId: "user-1" }, "user-2"), false);
    assert.equal(IsAuthorizedProgressionReporter({ IsPlayerHostRuntime: true, userId: "user-1" }, "user-1"), false);
    assert.equal(IsAuthorizedProgressionReporter({ IsGameserver: true }, "user-2"), true);
    assert.equal(IsAuthorizedProgressionReader({ IsPlayerHostRuntime: true, userId: "user-1" }, "user-1"), true);
});

test("character-scoped progression state requires an existing account-owned character", async () => {
    const Lookups: Array<[string, string]> = [];
    const Repositories: any = {
        characters: {
            findByCharacterIdAndUserId: async (characterId: string, userId: string) => {
                Lookups.push([characterId, userId]);
                return characterId === "char-owned" && userId === "user-1" ? { characterId, userId } : undefined;
            }
        }
    };

    assert.equal(await IsCharacterOwnedByUser("user-1", "char-owned", Repositories), true);
    assert.equal(await IsCharacterOwnedByUser("user-1", "char-other", Repositories), false);
    assert.equal(await IsCharacterOwnedByUser("user-2", "char-owned", Repositories), false);
    assert.equal(await IsCharacterOwnedByUser("user-1", "INVALID", Repositories), false);
    assert.deepEqual(Lookups, [["char-owned", "user-1"], ["char-other", "user-1"], ["char-owned", "user-2"]]);
});

test("absolute player progression reports are retained as diagnostics but never mutate canonical state", async () => {
    let Appended = 0;
    const Diagnostics = {
        appendObjectiveEvent: async () => { Appended++; }
    };
    await CaptureProgressionObjectiveEvent("user-1", {
        progress_tracks: [{ progression_id: "ExperienceTrack_PlayerLevel", progress: 1_000_000_000 }],
        objectives: [{ objective_id: "Objective_1", value: 1_000_000_000, completed_count: 1_000_000 }]
    }, Diagnostics);
    assert.equal(Appended, 1);
});

test("a game server's objective reports are stored with the max-guard; a player's never are", async () => {
    const Stored: Array<[string, number, number]> = [];
    const Repository = {
        appendObjectiveEvent: async () => {},
        setObjectiveIfGreater: async (_user: string, objectiveId: string, value: number, completedCount: number) => {
            Stored.push([objectiveId, value, completedCount]);
            return {} as any;
        }
    };
    const Report = { objectives: [{ objective_id: "Daily_Collectible_Ramsgate", value: 3, completed_count: 1 }] };
    await CaptureProgressionObjectiveEvent("user-1", Report, Repository);
    assert.deepEqual(Stored, [], "a player's own report stays diagnostics-only");
    await CaptureProgressionObjectiveEvent("user-1", Report, Repository, true);
    assert.deepEqual(Stored, [["Daily_Collectible_Ramsgate", 3, 1]]);
});

test("a game server's mastery track deltas are added once per new objective completion", async () => {
    const Stored = new Map<string, number>();
    const Progress = new Map<string, number>();
    const Repository = {
        appendObjectiveEvent: async () => {},
        setObjectiveIfGreater: async (_user: string, objectiveId: string, value: number, completedCount: number) => {
            const Previous = Stored.get(objectiveId) ?? 0;
            Stored.set(objectiveId, Math.max(Previous, completedCount));
            return { previousCompletedCount: Previous } as any;
        },
        increment: async (_user: string, progressionId: string, amount: number) => {
            Progress.set(progressionId, (Progress.get(progressionId) ?? 0) + amount);
            return { progress: Progress.get(progressionId) } as any;
        }
    };
    // Captured from a 1.14.7 hunt: one report per completed objective, each a delta.
    const Breaks = {
        progress_tracks: [{ progression_id: "MasteryTrack_PlayerLevel", progress: 1 }, { progression_id: "MasteryTrack_Weapon_Strikers", progress: 1 }],
        objectives: [{ objective_id: "MasteryObjective_Weapon_Strikers_Generic_Breaks2", value: 10, completed_count: 1 }]
    };
    const Kills = {
        progress_tracks: [{ progression_id: "MasteryTrack_PlayerLevel", progress: 1 }, { progression_id: "MasteryTrack_Weapon_Strikers", progress: 1 }],
        objectives: [{ objective_id: "MasteryObjective_Weapon_Strikers_Generic_Kills1", value: 2, completed_count: 1 }]
    };
    await CaptureProgressionObjectiveEvent("user-1", Breaks, Repository);
    assert.equal(Progress.size, 0, "a player's own report grants no mastery XP");
    await CaptureProgressionObjectiveEvent("user-1", Breaks, Repository, true);
    await CaptureProgressionObjectiveEvent("user-1", Kills, Repository, true);
    assert.equal(Progress.get("MasteryTrack_PlayerLevel"), 2);
    assert.equal(Progress.get("MasteryTrack_Weapon_Strikers"), 2);

    // The same completion reported again (a later hunt) adds nothing; a mixed report adds only the new one.
    await CaptureProgressionObjectiveEvent("user-1", Kills, Repository, true);
    await CaptureProgressionObjectiveEvent("user-1", {
        progress_tracks: [
            { progression_id: "MasteryTrack_PlayerLevel", progress: 2 },
            { progression_id: "MasteryTrack_Behemoth", progress: 1 },
            { progression_id: "MasteryTrack_Weapon_Strikers", progress: 1 }
        ],
        objectives: [
            { objective_id: "MasteryObjective_Behemoth_Host_Kill", value: 1, completed_count: 1 },
            { objective_id: "MasteryObjective_Weapon_Strikers_Generic_Kills1", value: 2, completed_count: 1 }
        ]
    }, Repository, true);
    assert.equal(Progress.get("MasteryTrack_PlayerLevel"), 3);
    assert.equal(Progress.get("MasteryTrack_Behemoth"), 1);
    assert.equal(Progress.get("MasteryTrack_Weapon_Strikers"), 2);

    // Only MasteryTrack_* tracks: the others still arrive through the grant route.
    await CaptureProgressionObjectiveEvent("user-1", {
        progress_tracks: [{ progression_id: "ExperienceTrack_PlayerLevel", progress: 500 }],
        objectives: [{ objective_id: "MasteryObjective_Weapon_Sword_Generic_Kills1", value: 2, completed_count: 1 }]
    }, Repository, true);
    assert.equal(Progress.has("ExperienceTrack_PlayerLevel"), false);
});

test("journey merge preserves concurrent unlocks and stale stored values", () => {
    const Stored = { A: { node_status: 1 }, Conflict: { node_status: 1 } };
    const Incoming = { B: { node_status: 1 }, Conflict: { node_status: 0 } };
    const Merged = MergePlayerJourneyNodes(Stored, Incoming, 3, 3);
    assert.deepEqual(Merged.nodes, {
        A: { node_status: 1 },
        B: { node_status: 1 },
        Conflict: { node_status: 1 }
    });
    assert.equal(Merged.version, 3);
});

test("Mongo journey repository applies an expected-version CAS predicate", async () => {
    let SeenFilter: any;
    const Collection = {
        updateOne: async (Filter: any) => {
            SeenFilter = Filter;
            return { matchedCount: 1 };
        }
    };
    const Db = { collection: () => Collection } as any;
    const Repository = new MongoPlayerJourneyRepository(async () => Db);
    assert.equal(await Repository.updateIfRevision("user-1", "{\"A\":1}", 4, 3), true);
    assert.deepEqual(SeenFilter, { _id: "user-1", revision: 3 });
});

test("Mongo progression grant repository uses the authoritative grant id as unique ledger key", async () => {
    let Inserted: any;
    const Collection = {
        findOne: async () => undefined,
        insertOne: async (Document: any) => { Inserted = Document; }
    };
    const Db = { collection: () => Collection } as any;
    const Repository = new MongoProgressionGrantRepository(async () => Db);
    const Record = {
        grantId: "hunt-1",
        userId: Track.userId,
        progressionId: Track.progressionId,
        amount: 125,
        requestHash: BuildProgressionGrantRequestHash(Track.userId, Track.progressionId, 125),
        status: "applied" as const,
        result: Track,
        createdAt: Track.createdAt
    };
    await Repository.insertApplied(Record, {} as any);
    assert.equal(Inserted._id, "hunt-1");
    assert.equal(Inserted.requestHash, Record.requestHash);
    assert.deepEqual(Inserted.result, Track);
});

test("same authoritative grant replays its recorded result and never increments twice", async () => {
    let Progress = 0;
    let IncrementCount = 0;
    const Session = { transaction: true };
    const Sessions: any[] = [];
    const Ledger = new Map<string, any>();
    const Repositories: any = {
        progressionGrants: {
            findByGrantId: async (GrantId: string, CurrentSession: any) => { Sessions.push(CurrentSession); return Ledger.get(GrantId); },
            insertApplied: async (Record: any, CurrentSession: any) => { Sessions.push(CurrentSession); Ledger.set(Record.grantId, Record); }
        },
        progressionTracks: {
            get: async (_UserId: string, _TrackId: string, CurrentSession: any) => { Sessions.push(CurrentSession); return Progress === 0 ? undefined : { ...Track, progress: Progress }; },
            increment: async (_UserId: string, _TrackId: string, Amount: number, CurrentSession: any) => {
                Sessions.push(CurrentSession);
                IncrementCount++;
                Progress += Amount;
                return { ...Track, progress: Progress };
            }
        }
    };
    const UnitOfWork: any = { withTransaction: async (Action: any) => Action(Repositories, Session) };

    const First = await GrantProgressionXp("user-1", "ExperienceTrack_PlayerLevel", 125, "grant-1", UnitOfWork);
    const Replay = await GrantProgressionXp("user-1", "ExperienceTrack_PlayerLevel", 125, "grant-1", UnitOfWork);
    assert.equal(First.replayed, false);
    assert.equal(Replay.replayed, true);
    assert.equal(Replay.record.progress, 125);
    assert.equal(IncrementCount, 1);
    assert.equal(Sessions.length, 5);
    assert.equal(Sessions.every((CurrentSession) => CurrentSession === Session), true);
});

test("grant request mismatch is rejected before a second mutation", async () => {
    const Ledger = new Map<string, any>();
    let IncrementCount = 0;
    const Repositories: any = {
        progressionGrants: {
            findByGrantId: async (GrantId: string) => Ledger.get(GrantId),
            insertApplied: async (Record: any) => { Ledger.set(Record.grantId, Record); }
        },
        progressionTracks: {
            get: async () => undefined,
            increment: async () => { IncrementCount++; return Track; }
        }
    };
    const UnitOfWork: any = { withTransaction: async (Action: any) => Action(Repositories, {}) };
    await GrantProgressionXp("user-1", "ExperienceTrack_PlayerLevel", 125, "grant-2", UnitOfWork);
    await assert.rejects(
        GrantProgressionXp("user-1", "ExperienceTrack_PlayerLevel", 126, "grant-2", UnitOfWork),
        /already used with a different request/
    );
    assert.equal(IncrementCount, 1);
});

test("a duplicate-key transaction race retries outside the aborted session", async () => {
    const Ledger = new Map<string, any>();
    let Attempts = 0;
    let Increments = 0;
    const Repositories: any = {
        progressionGrants: {
            findByGrantId: async (GrantId: string) => Ledger.get(GrantId),
            insertApplied: async (Record: any) => {
                if(Attempts === 1){
                    // Model the competing transaction committing the durable row while this
                    // transaction receives its duplicate-key abort.
                    Ledger.set(Record.grantId, Record);
                    const ErrorValue: any = new Error("duplicate");
                    ErrorValue.code = 11000;
                    throw ErrorValue;
                }
                Ledger.set(Record.grantId, Record);
            }
        },
        progressionTracks: {
            get: async () => undefined,
            increment: async () => { Increments++; return { ...Track, progress: 125 }; }
        }
    };
    const UnitOfWork: any = {
        withTransaction: async (Action: any) => {
            Attempts++;
            return Action(Repositories, {});
        }
    };
    const Result = await GrantProgressionXp("user-1", "ExperienceTrack_PlayerLevel", 125, "grant-race", UnitOfWork);
    assert.equal(Attempts, 2);
    assert.equal(Increments, 1);
    assert.equal(Result.replayed, true);
});

// [Dauntless Reforged Revival] Weapon reforge: DELETE /progression/<user>/ExperienceTrack_Weapon_<weapon>.
function FakeReforgeUnitOfWork(initial: Record<string, number>) {
    const Rows = new Map<string, any>(Object.entries(initial).map(([Id, Progress]) => [Id, { ...Track, progressionId: Id, progress: Progress, confirmedFremiumRank: 0 }]));
    const progressionTracks: any = {
        get: async (_u: string, Id: string) => Rows.get(Id) ? { ...Rows.get(Id) } : undefined,
        increment: async (_u: string, Id: string, Amount: number) => {
            const Row = Rows.get(Id) ?? { ...Track, progressionId: Id, progress: 0, confirmedFremiumRank: 0 };
            Row.progress += Amount; Rows.set(Id, Row); return { ...Row };
        },
        setConfirmedFremiumRank: async (_u: string, Id: string, Rank: number) => {
            const Row = Rows.get(Id) ?? { ...Track, progressionId: Id, progress: 0, confirmedFremiumRank: 0 };
            Row.confirmedFremiumRank = Math.max(Row.confirmedFremiumRank, Rank); Rows.set(Id, Row); return { ...Row };
        },
        resetForPrestige: async (_u: string, Id: string, Amount: number) => {
            const Row = Rows.get(Id);
            if (Row === undefined || Row.progress < Amount) return undefined;
            Row.progress -= Amount; Row.confirmedFremiumRank = 0; Row.confirmedPremiumRank = 0; return { ...Row };
        }
    };
    return { Rows, unitOfWork: { withTransaction: async (fn: any) => fn({ progressionTracks }, undefined) } };
}

test("a weapon reforge resets the maxed weapon and raises its reforge level each time", async () => {
    const Full = GetTrackCurve("ExperienceTrack_Weapon_Axe")!.cumulativeXpForRank.get(GetTrackCurve("ExperienceTrack_Weapon_Axe")!.maxRank)!;
    const Prestige = GetTrackCurve("PrestigeTrack_Weapon_Axe")!;
    const { Rows, unitOfWork } = FakeReforgeUnitOfWork({ ExperienceTrack_Weapon_Axe: Full });

    const First = await PrestigeExperienceTrack("user-1", "ExperienceTrack_Weapon_Axe", unitOfWork);
    assert.equal(First.ok, true);
    assert.equal(Rows.get("ExperienceTrack_Weapon_Axe").progress, 0);
    assert.equal(First.ok && First.prestigeRank, 2);
    assert.equal(RankForProgress(Prestige, Rows.get("PrestigeTrack_Weapon_Axe").progress), 2);

    // Not maxed again yet: a repeated request does not count a second reforge.
    assert.deepEqual(await PrestigeExperienceTrack("user-1", "ExperienceTrack_Weapon_Axe", unitOfWork), { ok: false, reason: "not_max_rank" });
    assert.equal(RankForProgress(Prestige, Rows.get("PrestigeTrack_Weapon_Axe").progress), 2);

    // Maxed again: the second reforge reaches the next level (the reported bug: it stayed at the first).
    Rows.get("ExperienceTrack_Weapon_Axe").progress = Full + 40;
    const Second = await PrestigeExperienceTrack("user-1", "ExperienceTrack_Weapon_Axe", unitOfWork);
    assert.equal(Second.ok && Second.prestigeRank, 3);
    assert.equal(Rows.get("ExperienceTrack_Weapon_Axe").progress, 40, "XP above the full curve stays");
    assert.equal(Rows.get("PrestigeTrack_Weapon_Axe").confirmedFremiumRank, 3);
    assert.equal(RankForProgress(Prestige, Rows.get("PrestigeTrack_Weapon_Axe").progress), 3);
});

test("only weapon experience tracks can be reforged", async () => {
    const { unitOfWork } = FakeReforgeUnitOfWork({ ExperienceTrack_PlayerLevel: 999999 });
    assert.deepEqual(await PrestigeExperienceTrack("user-1", "ExperienceTrack_PlayerLevel", unitOfWork), { ok: false, reason: "unknown_track" });
    assert.equal(PrestigeTrackFor("ExperienceTrack_Weapon_Hammer"), "PrestigeTrack_Weapon_Hammer");
});
