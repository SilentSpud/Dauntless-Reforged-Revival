// Writes the game-data fixtures the Rust host kit's tests compare against (tests/gamedata_node.rs), with the
// Node kit's own code: legacy/ReforgedHost/src/{progression,preset,gamepack,gamedata,captures,publish,paths,run}.ts,
// copied to a temporary folder with their imports given the .ts extension and run by Node's type stripping.
//
//   node rust/services/host/tests/fixtures/node/gen-gamedata.mjs
//
// Everything is fixed (dates, bytes), so running it again writes the same files.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import zlib from "node:zlib";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "..", "..", "..", "..", "..", "..");
const out = path.join(here, "gamedata");
const src = path.join(repo, "legacy", "ReforgedHost", "src");

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "rr-host-gen-"));
for (const name of ["progression", "preset", "gamepack", "gamedata", "captures", "publish", "paths", "run"]) {
    const text = fs.readFileSync(path.join(src, `${name}.ts`), "utf8").replace(/from "\.\/([a-z]+)"/g, 'from "./$1.ts"');
    fs.writeFileSync(path.join(tmp, `${name}.ts`), text);
}
const load = (name) => import(pathToFileURL(path.join(tmp, `${name}.ts`)).href);
const { buildProgressionConfig, writeProgressionSource, PROGRESSION_EXPORT_DIR } = await load("progression");
const { createPreset } = await load("preset");
const { exportGameDataPack } = await load("gamepack");
const { publishRuntime } = await load("publish");

fs.rmSync(out, { recursive: true, force: true });
const at = (...p) => path.join(out, ...p);
function write(file, data) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, data);
}

// ---------------------------------------------------------------- progression
// Rows as CatalogExporter's ProgressionExporter writes them (legacy/ReforgedHost/test/progression.test.ts), BOM,
// CRLF and a status line included.
function reward(rankId, extra = {}) {
    return { rankId, customRewardString: "", stackedItems: [], instancedItems: [], orderedInstancedItems: [], entitlements: [], buffCount: 0, ...extra };
}
function track(rowName, progressionTrack, xp, freeRewards = [], prestigeTrack = "None", base = {}) {
    return {
        rowName,
        prestigeTrack,
        base: {
            progressionTrack, premiumGatingEntitlement: "", resetWithCharacter: false, progressionMultiplier: 1, requirementCount: xp.length,
            requirements: xp.map((requiredXP, rankId) => ({ rankId, requiredXP })),
            freeRewards, premiumRewards: [], ...base,
        },
    };
}
const jsonl = (rows) => "﻿" + rows.map((r) => (typeof r === "string" ? r : JSON.stringify(r))).join("\r\n") + "\r\nstatus: 3 rows\r\n";

// The Node test's export.
const small = at("progression", "small");
write(path.join(small, PROGRESSION_EXPORT_DIR, "player-experience-tracks.jsonl"), jsonl([
    track("PlayerLevel", "ExperienceTrack_PlayerLevel", [0, 100, 250], [
        reward(1, { stackedItems: [{ catalogId: "CURRENCY_RAMS", quantity: 500, priority: 0 }], instancedItems: ["WP_SWORD_00"], entitlements: [{ entitlementId: "ent_title_00", duration: 0 }] }),
    ]),
]));
write(path.join(small, PROGRESSION_EXPORT_DIR, "weapon-experience-tracks.jsonl"), jsonl([track("Sword", "ExperienceTrack_Weapon_Sword", [0, 50], [], "PrestigeTrack_Weapon_Sword"), track("Axe", "ExperienceTrack_Weapon_Axe", [0, 70]), track("Empty", "ExperienceTrack_Weapon_None", [])]));
write(path.join(small, PROGRESSION_EXPORT_DIR, "mastery-tracks.jsonl"), jsonl([track("MasteryPlayer", "MasteryTrack_PlayerLevel", [0, 1, 2]), track("Duplicate", "ExperienceTrack_PlayerLevel", [0, 9])]));
write(path.join(small, PROGRESSION_EXPORT_DIR, "huntpass-season-table.jsonl"), jsonl([{ rowName: "HuntPass_Season43", base: { progressionTrack: "season43", premiumGatingEntitlement: "season43_premium", requirements: [], freeRewards: [], premiumRewards: [] } }]));

// The odd values a real export can hold: numbers as JSON writes them in other ways, ranks out of order, a negative
// or fractional rank, missing fields, a base that is null, text with escapes, a prestige track named by a mastery
// row, the row name as the id, and no Hunt Pass row (the default entitlement).
const odd = at("progression", "odd");
write(path.join(odd, PROGRESSION_EXPORT_DIR, "player-experience-tracks.jsonl"), jsonl([
    '{"rowName":"PlayerLevel","base":{"progressionTrack":"ExperienceTrack_PlayerLevel","premiumGatingEntitlement":"ent_\\u00e9lite \\"x\\"\\t","progressionMultiplier":1.5,"requirements":[{"rankId":2,"requiredXP":2.5e2},{"rankId":0,"requiredXP":0},{"rankId":1.0,"requiredXP":1E2},{"rankId":-1,"requiredXP":5},{"rankId":3.5,"requiredXP":5},{"rankId":4}],"freeRewards":[{"rankId":1,"customRewardString":"Reward \\u2014 one\\n","stackedItems":[{"catalogId":"CURRENCY_RAMS"},{"catalogId":""},{"quantity":3}],"instancedItems":["A","",7,"B"],"orderedInstancedItems":[{"catalogId":"O1","priority":2.0},{"priority":1}],"entitlements":[{"entitlementId":"E1","duration":1e3},{"duration":5}]},{}],"premiumRewards":[{"rankId":9007199254740993}]}}',
    '{"rowName":"NoBase","base":null}',
    '{"rowName":"ExperienceTrack_Weapon_Hammer","base":{"requirements":[{"rankId":0,"requiredXP":0},{"rankId":1,"requiredXP":80}],"progressionMultiplier":0}}',
    "not json {",
    "[1,2,3]",
    "",
    "   ",
]));
write(path.join(odd, PROGRESSION_EXPORT_DIR, "experience-tracks.jsonl"), jsonl([
    track("Generic", "ExperienceTrack_Generic", [0, 10], [], "PrestigeTrack_Generic", { progressionMultiplier: 2.0 }),
]));
write(path.join(odd, PROGRESSION_EXPORT_DIR, "mastery-tracks.jsonl"), jsonl([
    track("Mastery", "MasteryTrack_Sword", [0, 5], [], "PrestigeTrack_Weapon_Hammer"),
]));

for (const name of ["small", "odd"]) {
    const dir = at("progression", name);
    writeProgressionSource(dir, path.join(dir, "progression_config.source.json"));
    const built = buildProgressionConfig(dir);
    write(path.join(dir, "build.json"), JSON.stringify({ tracks: built.tracks, skipped: built.skipped, derived: built.derived }, null, 2) + "\n");
}

// ---------------------------------------------------------------- preset and pack
// A server's game data: real files, one still the example, a supplied source, unicode and a CRLF file.
const data = at("pack", "gamedata");
const gd = { backend: path.join(data, "backend"), director: path.join(data, "director"), raw: path.join(data, "backend", "raw") };
write(path.join(gd.backend, "progression_config.json"), '{"paths":1,"name":"Président — 😀"}\n');
write(path.join(gd.backend, "reward_cache_store.json"), "[]\n");
write(path.join(gd.backend, "reward_cache_store.example.json"), "[]\n");
write(path.join(gd.backend, "ladyluck_store.json"), '[{"live":1}]\n');
write(path.join(gd.backend, "platinum_store.json"), '[{"live":2}]\n');
write(path.join(gd.backend, "slayers_path.json"), '{\r\n  "crlf": true\r\n}\r\n');
write(path.join(gd.director, "player_hunts_table.json"), '{"hunts":2}\n');
write(path.join(gd.raw, "progression_config.source.json"), '{"source":3}\n');
const fake = (preset) => ({ backendGameData: gd.backend, directorGameData: gd.director, rawGameData: gd.raw, preset, workspace: at("pack", "workspace") });

const now = new Date("2026-10-15T14:52:05.055Z");
createPreset(fake(at("preset", "with-exe")), { name: "Friday \"Hunts\" — Público", executable: null, executableSha256: "ab".repeat(32), now });
createPreset(fake(at("preset", "no-exe")), { name: "x", executable: null, now });

const pack = exportGameDataPack(fake(null), at("pack", "pack.json.gz"), now);
write(at("pack", "pack.json"), zlib.gunzipSync(fs.readFileSync(pack.file)));
write(at("pack", "result.json"), JSON.stringify({ included: pack.included, left: pack.left }, null, 2) + "\n");

// ---------------------------------------------------------------- runtime
const runtimeSources = { "MysticParadox.dll": at("runtime", "build", "MysticParadox.dll"), "winmm.dll": at("runtime", "build", "winmm.dll") };
write(runtimeSources["MysticParadox.dll"], Buffer.alloc(6000, 3));
write(runtimeSources["winmm.dll"], Buffer.alloc(2000, 4));
publishRuntime({ runtimePublish: at("runtime", "published") }, new Date("2026-10-15T00:00:00Z"), runtimeSources);
for (const name of ["MysticParadox.dll", "winmm.dll"]) fs.rmSync(at("runtime", "published", name));
fs.rmSync(at("runtime", "build"), { recursive: true });

fs.rmSync(tmp, { recursive: true, force: true });
console.log("fixtures written to", out);
