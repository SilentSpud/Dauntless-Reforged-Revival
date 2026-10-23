//! The progression config (`GET /progression/config`) built from the host's own game, for hosts who
//! have no captured server response to supply (`legacy/ReforgedHost/src/progression.ts`):
//! `gamedata --progression`. After RB11, `backend gamedata` makes it.
//!
//! The 1.14.7 client loads its player data only when the config has the tracks it reads: the
//! `ExperienceTrack_*` (player and weapon levels), `MasteryTrack_*` and `PrestigeTrack_*` tracks, and
//! a path for the active Hunt Pass. Without them the progression, Hunt Pass, quest and bounty
//! components never finish loading, the 60-second player-data timeout fires and the hub leaves its
//! map. Mystic Paradox's example config has none of them.
//!
//! Those tracks are the game's own data tables, which CatalogExporter writes with
//! `EXPORT_PROGRESSION=1` to `Items_Analysis/progression_1_14_7/*.jsonl`: per-rank XP costs and the
//! rewards of each rank. They are converted here into the shape of the live service's response. The
//! Hunt Pass rows in the game hold no tiers, so the active pass gets the live service's usual 50 ranks
//! of 100 XP and no rewards.
//!
//! Each experience track names its prestige track, and the client asks for those too. The export has
//! no rank table for them, so a prestige track the export does not hold gets the ranks of the track
//! that names it, and no rewards. Every weapon gets one whether or not its row names it: the Slayer
//! level badge indexes its frame list with the weapon's prestige level, which is -1 when the track is
//! not in the config, and the client crashes on entering Ramsgate.
//!
//! The file is written as the Node kit writes it (`JSON.stringify(config, null, 2)` and a newline),
//! byte for byte.

use std::collections::HashSet;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::error::HostError;
use crate::gamepack::{read_text, write_file};
use crate::hostconfig::stringify_pretty;

pub const PROGRESSION_EXPORT_DIR: &str = "progression_1_14_7";
/// The files with track rows (`{"rowName", "base": {...}}`), in order of preference for a track id.
pub const TRACK_EXPORTS: [&str; 4] = [
    "player-experience-tracks.jsonl",
    "weapon-experience-tracks.jsonl",
    "experience-tracks.jsonl",
    "mastery-tracks.jsonl",
];
pub const HUNT_PASS_EXPORT: &str = "huntpass-season-table.jsonl";
/// The backend's `ACTIVE_HUNTPASS_PROGRESSION_ID`.
pub const ACTIVE_HUNT_PASS: &str = "season43";
/// Marks a source this module wrote, so the next export replaces it (a captured response is never
/// touched).
pub const GENERATED_BY: &str = "Dauntless Reforged Revival host kit, from the CatalogExporter progression export";

const TRACK_START: &str = "2019-01-01T14:00:00+00:00";
const TRACK_END: &str = "2099-01-01T14:00:00+00:00";
const HUNT_PASS_RANKS: i64 = 50;
const HUNT_PASS_XP_PER_RANK: i64 = 100;
const REQUIRED_TRACKS: [&str; 1] = ["ExperienceTrack_PlayerLevel"];
const WEAPON_EXPERIENCE_PREFIX: &str = "ExperienceTrack_Weapon_";
const WEAPON_PRESTIGE_PREFIX: &str = "PrestigeTrack_Weapon_";
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// JavaScript's `String.prototype.trim` whitespace (Unicode white space and the BOM).
fn js_trim(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// The object rows of a JSON-lines file; a status or partial line is not a row. Nothing when the
/// file is not there.
pub fn read_jsonl(file: &Path) -> Result<Vec<Map<String, Value>>, HostError> {
    if !file.exists() {
        return Ok(Vec::new());
    }
    Ok(parse_jsonl(&read_text(file)?))
}

/// [`read_jsonl`] on the file's text.
pub fn parse_jsonl(text: &str) -> Vec<Map<String, Value>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rows = Vec::new();
    for line in text.split('\n') {
        let line = js_trim(line.strip_suffix('\r').unwrap_or(line));
        if line.is_empty() {
            continue;
        }
        if let Ok(Value::Object(row)) = service_kit::jsjson::parse(line) {
            rows.push(row);
        }
    }
    rows
}

/// A whole number of at least 0 (`Number.isSafeInteger(v) && v >= 0`), else `fallback`.
fn whole(v: Option<&Value>, fallback: i64) -> i64 {
    v.and_then(Value::as_i64)
        .filter(|n| (0..=MAX_SAFE_INTEGER).contains(n))
        .unwrap_or(fallback)
}

fn text(v: Option<&Value>) -> &str {
    v.and_then(Value::as_str).unwrap_or("")
}

/// `value.key` when `value` is an object.
fn field<'a>(value: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    value.and_then(|v| v.get(key))
}

fn list(value: Option<&Value>) -> &[Value] {
    value.and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn convert_reward(r: &Value) -> Value {
    let get = |key: &str| r.get(key);
    let stacked: Vec<Value> = list(get("stackedItems"))
        .iter()
        .filter(|s| !text(s.get("catalogId")).is_empty())
        .map(|s| {
            json!({
                "catalog_id": text(s.get("catalogId")),
                "hidden": false,
                "priority": whole(s.get("priority"), 0),
                "quantity": whole(s.get("quantity"), 1),
            })
        })
        .collect();
    let instanced: Vec<Value> = list(get("instancedItems"))
        .iter()
        .filter(|s| s.as_str().is_some_and(|s| !s.is_empty()))
        .cloned()
        .collect();
    let ordered: Vec<Value> = list(get("orderedInstancedItems"))
        .iter()
        .filter(|s| !text(s.get("catalogId")).is_empty())
        .map(|s| json!({ "catalog_id": text(s.get("catalogId")), "priority": whole(s.get("priority"), 0) }))
        .collect();
    let entitlements: Vec<Value> = list(get("entitlements"))
        .iter()
        .filter(|e| !text(e.get("entitlementId")).is_empty())
        .map(|e| json!({ "duration": whole(e.get("duration"), 0), "entitlement": text(e.get("entitlementId")) }))
        .collect();
    let custom = text(get("customRewardString"));
    json!({
        "rank_id": whole(get("rankId"), 0),
        "custom_reward_string": { "culture_invariant_string": if custom.is_empty() { Value::Null } else { Value::from(custom) } },
        "stacked_items": stacked,
        "instanced_items": instanced,
        "ordered_instanced_items": ordered,
        "entitlements": entitlements,
        "buffs": [],
    })
}

fn empty_reward(rank: i64) -> Value {
    json!({ "rank_id": rank, "stacked_items": [], "instanced_items": [], "ordered_instanced_items": [], "entitlements": [] })
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgressionBuild {
    /// `{ generated_by, code: null, message: "OK", payload: { paths } }`
    pub config: Value,
    pub paths: usize,
    pub tracks: Vec<String>,
    pub skipped: Vec<String>,
    /// Prestige tracks given the ranks of the track that names them.
    pub derived: Vec<String>,
}

/// Builds the config from an `Items_Analysis` folder, or fails with what is missing.
pub fn build_progression_config(exports_dir: &Path, active_hunt_pass: &str) -> Result<ProgressionBuild, HostError> {
    let dir = exports_dir.join(PROGRESSION_EXPORT_DIR);
    if !dir.exists() {
        return Err(HostError::message(format!(
            "no {PROGRESSION_EXPORT_DIR} in {} (inject CatalogExporter with EXPORT_PROGRESSION=1, then copy Items_Analysis)",
            exports_dir.display()
        )));
    }
    let tracks = TRACK_EXPORTS
        .iter()
        .map(|file| read_jsonl(&dir.join(file)))
        .collect::<Result<Vec<_>, _>>()?;
    let seasons = read_jsonl(&dir.join(HUNT_PASS_EXPORT))?;
    build_from_rows(&tracks, &seasons, active_hunt_pass)
}

/// The config from the rows of each of [`TRACK_EXPORTS`] (in that order) and of [`HUNT_PASS_EXPORT`].
pub fn build_from_rows(
    tracks: &[Vec<Map<String, Value>>],
    seasons: &[Map<String, Value>],
    active_hunt_pass: &str,
) -> Result<ProgressionBuild, HostError> {
    let mut paths: Vec<Value> = Vec::new();
    // Insertion order is kept for `tracks`, as a JavaScript Set keeps it.
    let mut seen: Vec<String> = Vec::new();
    let mut seen_set: HashSet<String> = HashSet::new();
    let mut skipped = Vec::new();
    let mut prestige: Vec<(String, Vec<Value>)> = Vec::new();
    for rows in tracks {
        for row in rows {
            let base = row.get("base").filter(|b| !b.is_null());
            let mut id = text(field(base, "progressionTrack"));
            if id.is_empty() {
                id = text(row.get("rowName"));
            }
            if id.is_empty() || seen_set.contains(id) {
                continue;
            }
            let mut ranks: Vec<(i64, i64)> = list(field(base, "requirements"))
                .iter()
                .map(|q| (whole(q.get("rankId"), -1), whole(q.get("requiredXP"), -1)))
                .filter(|(rank, xp)| *rank >= 0 && *xp >= 0)
                .collect();
            ranks.sort_by_key(|(rank, _)| *rank);
            let requirements: Vec<Value> = ranks
                .iter()
                .map(|(rank, xp)| json!({ "rank_id": rank, "xp_required": xp }))
                .collect();
            let named = text(row.get("prestigeTrack"));
            if !named.is_empty() && named != "None" && !requirements.is_empty() {
                prestige.push((named.to_owned(), requirements.clone()));
            }
            if let Some(weapon) = id
                .strip_prefix(WEAPON_EXPERIENCE_PREFIX)
                .filter(|_| !requirements.is_empty())
            {
                prestige.push((format!("{WEAPON_PRESTIGE_PREFIX}{weapon}"), requirements.clone()));
            }
            if requirements.is_empty() {
                skipped.push(id.to_owned());
                continue;
            }
            seen.push(id.to_owned());
            seen_set.insert(id.to_owned());
            let multiplier = field(base, "progressionMultiplier")
                .filter(|m| m.as_f64().is_some_and(|m| m > 0.0))
                .cloned()
                .unwrap_or_else(|| Value::from(1));
            paths.push(json!({
                "progression_id": id,
                "premium_gating_entitlement": text(field(base, "premiumGatingEntitlement")),
                "progression_multiplier": multiplier,
                "start_date": TRACK_START,
                "end_date": TRACK_END,
                "requirements": requirements,
                "free_rewards": list(field(base, "freeRewards")).iter().map(convert_reward).collect::<Vec<_>>(),
                "premium_rewards": list(field(base, "premiumRewards")).iter().map(convert_reward).collect::<Vec<_>>(),
            }));
        }
    }
    let mut derived = Vec::new();
    for (id, requirements) in prestige {
        if seen_set.contains(&id) {
            continue;
        }
        seen.push(id.clone());
        seen_set.insert(id.clone());
        derived.push(id.clone());
        paths.push(json!({
            "progression_id": id,
            "premium_gating_entitlement": "",
            "progression_multiplier": 1,
            "start_date": TRACK_START,
            "end_date": TRACK_END,
            "requirements": requirements,
            "free_rewards": [],
            "premium_rewards": [],
        }));
    }
    let missing: Vec<&str> = REQUIRED_TRACKS
        .into_iter()
        .filter(|id| !seen_set.contains(*id))
        .collect();
    if !missing.is_empty() {
        return Err(HostError::message(format!(
            "the progression export has no {} (an incomplete export: inject CatalogExporter again)",
            missing.join(", ")
        )));
    }

    // The active Hunt Pass: its identity from the game's season table, 50 ranks of 100 XP, no rewards.
    let season = seasons.iter().find(|r| {
        text(field(r.get("base"), "progressionTrack")) == active_hunt_pass
            || text(r.get("rowName")).to_lowercase() == active_hunt_pass.to_lowercase()
    });
    if !seen_set.contains(active_hunt_pass) {
        let ranks = 0..=HUNT_PASS_RANKS;
        let entitlement = text(field(season.and_then(|s| s.get("base")), "premiumGatingEntitlement"));
        let entitlement = if entitlement.is_empty() {
            format!("{active_hunt_pass}_premium")
        } else {
            entitlement.to_owned()
        };
        paths.push(json!({
            "progression_id": active_hunt_pass,
            "premium_gating_entitlement": entitlement,
            "progression_multiplier": 1,
            "start_date": TRACK_START,
            "end_date": TRACK_END,
            "requirements": ranks.clone().map(|rank| json!({ "rank_id": rank, "xp_required": if rank == 0 { 0 } else { HUNT_PASS_XP_PER_RANK } })).collect::<Vec<_>>(),
            "free_rewards": ranks.clone().map(empty_reward).collect::<Vec<_>>(),
            "premium_rewards": ranks.map(empty_reward).collect::<Vec<_>>(),
            "prestige": null,
        }));
        seen.push(active_hunt_pass.to_owned());
    }
    let count = paths.len();
    Ok(ProgressionBuild {
        config: json!({ "generated_by": GENERATED_BY, "code": null, "message": "OK", "payload": { "paths": paths } }),
        paths: count,
        tracks: seen,
        skipped,
        derived,
    })
}

/// The config file's text, as the Node kit writes it.
pub fn progression_text(build: &ProgressionBuild) -> String {
    stringify_pretty(&build.config) + "\n"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceWritten {
    /// The host supplied a captured response: left alone.
    Supplied,
    NoExport,
    Written {
        written: usize,
        derived: usize,
    },
}

/// The source file the backend's generator reads: written from the export unless the host supplied
/// a captured response. Returns what it did.
pub fn write_progression_source(exports_dir: &Path, source_file: &Path) -> Result<SourceWritten, HostError> {
    if source_file.exists() {
        let text = read_text(source_file)?;
        let existing = service_kit::jsjson::parse(text.strip_prefix('\u{feff}').unwrap_or(&text));
        let ours = existing
            .ok()
            .and_then(|v| v.get("generated_by").and_then(Value::as_str).map(|g| g == GENERATED_BY))
            .unwrap_or(false);
        if !ours {
            return Ok(SourceWritten::Supplied);
        }
    }
    if !exports_dir.join(PROGRESSION_EXPORT_DIR).exists() {
        return Ok(SourceWritten::NoExport);
    }
    let built = build_progression_config(exports_dir, ACTIVE_HUNT_PASS)?;
    write_file(source_file, progression_text(&built))?;
    Ok(SourceWritten::Written {
        written: built.paths,
        derived: built.derived.len(),
    })
}
