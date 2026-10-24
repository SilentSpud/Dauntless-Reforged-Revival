//! The hunt manifest lookup (`paradox/shared/hunt-manifest`, 2026-10-14 M1): for a player hunt id and
//! its party, which map, Behemoth and expected-player string a game server starts with. The Metagame
//! resolves with the same code, so error messages and the manifest's JSON stay as the Node module
//! writes them; `golden-digest.json` pins every manifest the real tables produce.
//!
//! Rows are kept as parsed JSON in JavaScript's key order (`service_kit::jsjson`): the tables are the
//! game's DataTable exports, read as the Node code reads them. Table lookups read only a table's own
//! keys, so a HuntId such as "constructor" finds nothing.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Map, Value, json};
use service_kit::{js, jsjson};

pub const TRIALS_ROTATION_ROW_COUNT: i64 = 181;
pub const DEFAULT_TRIALS_ROTATION_MINUTES: i64 = 7 * 24 * 60;
const MAX_TRIALS_ROTATION_MINUTES: i64 = 525_600;

pub const PLAYER_TABLE: &str = "player_hunts_table.json";
/// The matchmaker tables by name and file. The first is the main table and wins a row-name collision;
/// the two `_new` Trials tables came with 1.14.7 (reached only through tag routing).
pub const MATCHMAKER_TABLES: [(&str, &str); 6] = [
    ("matchmaker_hunts_table", "matchmaker_hunts_table.json"),
    ("arena_easy_matchmaker_hunts", "arena_easy_matchmaker_hunts.json"),
    ("arena_hard_matchmaker_hunts", "arena_hard_matchmaker_hunts.json"),
    ("arena_elite_matchmaker_hunts", "arena_elite_matchmaker_hunts.json"),
    (
        "arena_hard_matchmaker_hunts_new",
        "arena_hard_matchmaker_hunts_new.json",
    ),
    (
        "arena_elite_matchmaker_hunts_new",
        "arena_elite_matchmaker_hunts_new.json",
    ),
];

#[derive(Debug, Clone, Default)]
pub struct Tables {
    pub player_rows: Map<String, Value>,
    /// `(name, rows)`; the first is the main matchmaker table.
    pub matchmaker_sources: Vec<(String, Map<String, Value>)>,
}

impl Tables {
    pub fn matchmaker_rows(&self) -> Option<&Map<String, Value>> {
        self.matchmaker_sources.first().map(|(_, rows)| rows)
    }
}

fn read_table(dir: &Path, file: &str) -> Result<Map<String, Value>, String> {
    let path = dir.join(file);
    let text = std::fs::read_to_string(&path)
        .map_err(|_| format!("[hunt-manifest] Required hunt table is missing: {}", path.display()))?;
    let document = jsjson::parse(text.strip_prefix('\u{feff}').unwrap_or(&text))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    match document.get(0).and_then(|d| d.get("Rows")) {
        Some(Value::Object(rows)) => Ok(rows.clone()),
        _ => Err(format!(
            "[hunt-manifest] {} is not a DataTable export (no Rows)",
            path.display()
        )),
    }
}

/// Reads every hunt table from `dir`; an error when one is missing, as the Director always had.
pub fn load_tables(dir: &Path) -> Result<Tables, String> {
    let player_rows = read_table(dir, PLAYER_TABLE)?;
    let mut matchmaker_sources = Vec::new();
    for (name, file) in MATCHMAKER_TABLES {
        matchmaker_sources.push((name.to_owned(), read_table(dir, file)?));
    }
    Ok(Tables {
        player_rows,
        matchmaker_sources,
    })
}

pub trait HuntLog: Send + Sync {
    fn info(&self, message: &str);
    fn warn(&self, message: &str);
}

pub struct NoLog;

impl HuntLog for NoLog {
    fn info(&self, _: &str) {}
    fn warn(&self, _: &str) {}
}

/// TRIALS_ROTATION_MINUTES; an invalid value warns and uses the default.
pub fn parse_trials_rotation_minutes(value: Option<&str>, log: &dyn HuntLog) -> i64 {
    let parsed = match value {
        None => DEFAULT_TRIALS_ROTATION_MINUTES as f64,
        Some(text) => js::number(Some(&Value::String(text.to_owned()))),
    };
    if parsed.fract() != 0.0 || !(1.0..=MAX_TRIALS_ROTATION_MINUTES as f64).contains(&parsed) {
        log.warn(&format!(
            "[TrialsRotation] invalid TRIALS_ROTATION_MINUTES='{}'; using {DEFAULT_TRIALS_ROTATION_MINUTES}",
            value.unwrap_or("")
        ));
        return DEFAULT_TRIALS_ROTATION_MINUTES;
    }
    parsed as i64
}

/// The three-digit trials row featured in the UTC bucket holding `now_ms` (the runtime uses the same formula).
pub fn current_trials_week(now_ms: i64, rotation_minutes: i64) -> String {
    let bucket = now_ms.div_euclid(rotation_minutes * 60_000);
    format!("{:03}", bucket.rem_euclid(TRIALS_ROTATION_ROW_COUNT) + 1)
}

/// `uid:huntId` pairs joined by commas.
pub fn expected_player_args<'a>(players: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    players
        .into_iter()
        .map(|(uid, hunt)| format!("{uid}:{hunt}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn tag_matches(row_tag: &str, query_tag: &str) -> bool {
    row_tag == query_tag || row_tag.starts_with(&format!("{query_tag}."))
}

/// Evaluates a serialized FGameplayTagQuery against a row's HuntTags, as UE's FQueryEvaluator does:
/// `[version, hasRoot, exprType, ...]`. Types 1-3 (Any/All/NoTagsMatch) carry a count and that many
/// TagDictionary indices, 4-6 (Any/All/NoExprMatch) a count and that many nested expressions. Matching
/// is prefix-aware on whole segments ("Hunt.Arena.Easy.X" satisfies "Hunt.Arena.Easy"). An unknown
/// expression type or a truncated stream matches nothing.
pub fn evaluate_tag_query(query: &Value, row_tags: &[Value]) -> bool {
    let pick = |names: &[&str]| names.iter().find_map(|n| query.get(n).filter(|v| !v.is_null()));
    // Vendored (FModel) rows say QueryTokenStream; the raw CatalogExporter capture says tokenStream.
    let stream = match pick(&["QueryTokenStream", "queryTokenStream", "tokenStream"]) {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    let dictionary: Vec<Value> = match pick(&["TagDictionary", "tagDictionary"]) {
        Some(Value::Array(items)) => items
            .iter()
            .map(|entry| match entry {
                Value::String(_) => entry.clone(),
                _ => entry
                    .get("TagName")
                    .filter(|v| !v.is_null())
                    .cloned()
                    .unwrap_or(json!("")),
            })
            .collect(),
        _ => Vec::new(),
    };
    if stream.len() < 3 {
        return false;
    }
    let row_tags: Vec<&str> = row_tags.iter().filter_map(Value::as_str).collect();
    let mut eval = Evaluator {
        stream: &stream,
        dictionary: &dictionary,
        row_tags: &row_tags,
        index: 0,
        overflowed: false,
    };
    eval.next();
    if !js::truthy(eval.next()) {
        return false;
    }
    let result = eval.expr();
    !eval.overflowed && result
}

struct Evaluator<'a> {
    stream: &'a [Value],
    dictionary: &'a [Value],
    row_tags: &'a [&'a str],
    index: usize,
    overflowed: bool,
}

impl<'a> Evaluator<'a> {
    fn next(&mut self) -> Option<&'a Value> {
        let value = self.stream.get(self.index);
        self.index += 1;
        value
    }

    fn count(&mut self) -> usize {
        let n = self.next().and_then(Value::as_f64).unwrap_or(0.0);
        if n > 0.0 { n.ceil() as usize } else { 0 }
    }

    fn expr(&mut self) -> bool {
        if self.index >= self.stream.len() {
            self.overflowed = true;
            return false;
        }
        let kind = self.next().and_then(Value::as_f64).unwrap_or(f64::NAN);
        if (1.0..=3.0).contains(&kind) {
            let count = self.count();
            let mut tags = Vec::new();
            for _ in 0..count {
                let tag = self
                    .next()
                    .and_then(Value::as_f64)
                    .filter(|i| i.fract() == 0.0 && *i >= 0.0)
                    .and_then(|i| self.dictionary.get(i as usize))
                    .and_then(Value::as_str);
                if let Some(tag) = tag.filter(|t| !t.is_empty()) {
                    tags.push(tag);
                }
            }
            let matched = |query: &&str| self.row_tags.iter().any(|row| tag_matches(row, query));
            let any = tags.iter().any(matched);
            let all = tags.iter().all(matched);
            return if kind == 1.0 {
                any
            } else if kind == 2.0 {
                all
            } else if kind == 3.0 {
                !any
            } else {
                // A fractional type (2.5) is NoTagsMatch to the Node code: neither === 1 nor === 2.
                !any
            };
        }
        if (4.0..=6.0).contains(&kind) {
            let count = self.count();
            let results: Vec<bool> = (0..count).map(|_| self.expr()).collect();
            return if kind == 4.0 {
                results.iter().any(|r| *r)
            } else if kind == 5.0 {
                results.iter().all(|r| *r)
            } else {
                !results.iter().any(|r| *r)
            };
        }
        self.overflowed = true;
        false
    }
}

fn own_row<'a>(rows: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    rows.get(key).filter(|row| !row.is_null())
}

fn is_arena_week_row(id: &str) -> bool {
    ["Easy", "Hard", "Elite"].iter().any(|level| {
        id.strip_prefix("Arena_MatchmakerHunt_")
            .and_then(|rest| rest.strip_prefix(level))
            .and_then(|rest| rest.strip_prefix('_'))
            .is_some_and(|week| week.len() == 3 && week.bytes().all(|b| b.is_ascii_digit()))
    })
}

fn usable_map(entry: &Value) -> Option<&str> {
    entry
        .get("MapAssetName")
        .and_then(Value::as_str)
        .filter(|m| m.starts_with("/Game/") && m.contains('.'))
}

type RandomInt = Box<dyn Fn(usize, usize) -> usize + Send + Sync>;
type MissingHunt = Box<dyn Fn(&str) + Send + Sync>;

pub struct ResolverOptions {
    pub tables: Tables,
    pub log: Box<dyn HuntLog>,
    pub trials_rotation_minutes: i64,
    pub now: Box<dyn Fn() -> i64 + Send + Sync>,
    /// An integer in `[min, max)`.
    pub random_int: RandomInt,
    /// Called once per request for an absent player hunt row.
    pub on_missing_hunt: MissingHunt,
}

impl ResolverOptions {
    pub fn new(tables: Tables) -> Self {
        Self {
            tables,
            log: Box::new(NoLog),
            trials_rotation_minutes: DEFAULT_TRIALS_ROTATION_MINUTES,
            now: Box::new(|| chrono::Utc::now().timestamp_millis()),
            random_int: Box::new(crate::random::int_in),
            on_missing_hunt: Box::new(|_| {}),
        }
    }
}

pub struct HuntResolver {
    pub tables: Tables,
    pub trials_rotation_minutes: i64,
    log: Box<dyn HuntLog>,
    now: Box<dyn Fn() -> i64 + Send + Sync>,
    random_int: RandomInt,
    on_missing_hunt: MissingHunt,
    quiet: AtomicBool,
}

impl HuntResolver {
    pub fn new(options: ResolverOptions) -> Self {
        Self {
            tables: options.tables,
            trials_rotation_minutes: options.trials_rotation_minutes,
            log: options.log,
            now: options.now,
            random_int: options.random_int,
            on_missing_hunt: options.on_missing_hunt,
            quiet: AtomicBool::new(false),
        }
    }

    fn info(&self, message: &str) {
        if !self.quiet.load(Ordering::Relaxed) {
            self.log.info(message);
        }
    }

    fn warn(&self, message: &str) {
        if !self.quiet.load(Ordering::Relaxed) {
            self.log.warn(message);
        }
    }

    fn random(&self, len: usize) -> usize {
        (self.random_int)(0, len).min(len.saturating_sub(1))
    }

    /// A matchmaker row from every registered table; the main table wins a name collision.
    pub fn find_matchmaker_row(&self, id: &str) -> Option<&Value> {
        self.tables
            .matchmaker_sources
            .iter()
            .find_map(|(_, rows)| own_row(rows, id))
    }

    /// MatchmakerHuntsByTag resolved to concrete matchmaker row names.
    pub fn tag_routed_matchmaker_hunt_ids(&self, player_hunt_id: &str, row: &Value) -> Vec<String> {
        let Some(Value::Array(lists)) = row.get("MatchmakerHuntsByTag") else {
            return Vec::new();
        };
        let mut resolved = Vec::new();
        for list in lists {
            // The vendored shape nests the table name in an ObjectName like "DataTable'arena_easy...'".
            let object_name = list
                .get("MatchmakerTable")
                .and_then(|t| t.get("ObjectName"))
                .filter(|v| !v.is_null())
                .or_else(|| list.get("matchmakerTable").filter(|v| !v.is_null()))
                .and_then(Value::as_str)
                .unwrap_or("");
            let table_name = if object_name.contains('\'') {
                object_name.split('\'').nth(1).unwrap_or("")
            } else {
                object_name
            };
            let Some((_, rows)) = self
                .tables
                .matchmaker_sources
                .iter()
                .find(|(name, _)| name == table_name)
            else {
                self.warn(&format!(
                    "[HuntTagRouting] {player_hunt_id} references matchmaker table '{table_name}' which is not registered; add it to HUNT_TABLE_FILES in shared/hunt-manifest"
                ));
                continue;
            };
            let queries = match list
                .get("HuntTags")
                .filter(|v| !v.is_null())
                .or_else(|| list.get("queries").filter(|v| !v.is_null()))
            {
                Some(Value::Array(queries)) if !queries.is_empty() => queries,
                _ => continue,
            };
            for (row_name, matchmaker_row) in rows {
                let tags = match matchmaker_row.get("HuntTags") {
                    Some(Value::Array(tags)) if !tags.is_empty() => tags,
                    _ => continue,
                };
                // Queries are ANDed.
                if queries.iter().all(|q| evaluate_tag_query(q, tags)) {
                    resolved.push(row_name.clone());
                }
            }
        }
        resolved
    }

    fn direct_matchmaker_hunt_ids(&self, row: &Value) -> Vec<String> {
        match row.get("MatchmakerHuntIDs") {
            Some(Value::Array(entries)) => entries
                .iter()
                .filter_map(|e| e.get("RowName").and_then(Value::as_str))
                .filter(|name| !name.is_empty() && self.find_matchmaker_row(name).is_some())
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The matchmaker row a player hunt starts with: a direct reference, else tag routing; random
    /// among the candidates, except Trials, which take the featured week's row.
    pub fn matchmaker_hunt_id_for(&self, player_hunt_id: &str) -> Result<String, String> {
        let Some(row) = own_row(&self.tables.player_rows, player_hunt_id) else {
            // Only a genuinely absent row is a coverage gap.
            (self.on_missing_hunt)(player_hunt_id);
            return Err(format!(
                "player_hunts_table has no usable row for HuntId '{player_hunt_id}' (missing row or empty MatchmakerHuntIDs)"
            ));
        };
        let mut usable = self.direct_matchmaker_hunt_ids(row);
        let mut tag_routed = false;
        if usable.is_empty() {
            usable = self.tag_routed_matchmaker_hunt_ids(player_hunt_id, row);
            if !usable.is_empty() {
                tag_routed = true;
                self.info(&format!(
                    "[HuntTagRouting] {player_hunt_id} resolved to {} matchmaker row(s) via tag query",
                    usable.len()
                ));
            }
        }
        if usable.is_empty() {
            return Err(format!(
                "player_hunts_table row '{player_hunt_id}' has no resolvable 1.12 MatchmakerHuntIDs"
            ));
        }
        // Arena rows are tag-routed to every numbered week: pick the current UTC bucket's, the row the
        // replicated schedule advertises. Other tag-routed hunts stay random.
        if tag_routed {
            let arena: Vec<&String> = usable.iter().filter(|id| is_arena_week_row(id)).collect();
            if !arena.is_empty() {
                let week = current_trials_week((self.now)(), self.trials_rotation_minutes);
                let suffix = format!("_{week}");
                let pinned: Vec<&&String> = arena.iter().filter(|id| id.ends_with(&suffix)).collect();
                if !pinned.is_empty() {
                    let chosen = (*pinned[self.random(pinned.len())]).clone();
                    self.info(&format!(
                        "[TrialsRotation] interval={}m featured=_{week} -> {chosen}",
                        self.trials_rotation_minutes
                    ));
                    return Ok(chosen);
                }
                self.warn(&format!(
                    "[TrialsRotation] no arena row matched featured week _{week}; falling back to a numbered arena candidate"
                ));
                return Ok(arena[self.random(arena.len())].clone());
            }
        }
        Ok(usable[self.random(usable.len())].clone())
    }

    /// The fixed Behemoth of a matchmaker row, or `None` for Hunting Grounds, which generate their
    /// encounters (no MonsterClass is passed, so the server runs its normal encounter generation).
    pub fn behemoth_path_for(&self, id: &str) -> Result<Option<Value>, String> {
        let row = self
            .find_matchmaker_row(id)
            .ok_or_else(|| format!("matchmaker_hunts_table has no row for MatchmakerHuntId '{id}'"))?;
        let asset = row
            .get("SpecificBehemoth")
            .and_then(|b| b.get("BehemothAsset"))
            .and_then(|a| a.get("AssetPathName"))
            .filter(|v| !v.is_null());
        Ok(asset
            .filter(|a| a.as_str() != Some("None") && !js::trim(&js::string(Some(a))).is_empty())
            .cloned())
    }

    pub fn map_path_for(&self, id: &str) -> Result<String, String> {
        let list = match self.find_matchmaker_row(id).and_then(|r| r.get("MapList")) {
            Some(Value::Array(list)) if !list.is_empty() => list,
            _ => return Err(format!("matchmaker_hunts_table row '{id}' has no MapList")),
        };
        let usable: Vec<&str> = list.iter().filter_map(usable_map).collect();
        if usable.is_empty() {
            return Err(format!(
                "matchmaker_hunts_table row '{id}' has no valid /Game/ map asset path"
            ));
        }
        let asset = usable[self.random(usable.len())];
        Ok(asset[..asset.rfind('.').unwrap_or(asset.len())].to_owned())
    }

    /// The exact server arguments of a player hunt for these account ids, without starting anything.
    /// `hunt_id` is the request's value as sent (`None` when absent).
    pub fn resolve_manifest(&self, hunt_id: Option<&Value>, expected: Option<&Value>) -> Result<Value, String> {
        let players: Vec<&str> = match expected {
            Some(Value::Array(items)) if (1..=4).contains(&items.len()) => {
                let ids: Vec<&str> = items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .collect();
                if ids.len() != items.len() {
                    return Err(INVALID_PLAYERS.to_owned());
                }
                ids
            }
            _ => return Err(INVALID_PLAYERS.to_owned()),
        };
        let key = js::string(hunt_id);
        let matchmaker_hunt_id = self.matchmaker_hunt_id_for(&key)?;
        let behemoth = self
            .behemoth_path_for(&matchmaker_hunt_id)?
            .unwrap_or_else(|| json!("NO_BEHEMOTH"));
        let map_path = self.map_path_for(&matchmaker_hunt_id)?;
        let expected_player_string = expected_player_args(players.iter().map(|uid| (*uid, key.as_str())));
        let mut manifest = Map::new();
        if let Some(hunt_id) = hunt_id {
            manifest.insert("huntId".into(), hunt_id.clone());
        }
        manifest.insert("matchmakerHuntId".into(), json!(matchmaker_hunt_id));
        manifest.insert("mapPath".into(), json!(map_path));
        manifest.insert("behemothPath".into(), behemoth);
        manifest.insert("expectedPlayerString".into(), json!(expected_player_string));
        Ok(Value::Object(manifest))
    }

    /// How many player hunt rows resolve to a manifest; logs nothing per hunt.
    pub fn count_resolvable_player_hunts(&self) -> usize {
        let was = self.quiet.swap(true, Ordering::Relaxed);
        let probe = json!(["startup-count"]);
        let count = self
            .tables
            .player_rows
            .keys()
            .filter(|id| self.resolve_manifest(Some(&json!(id)), Some(&probe)).is_ok())
            .count();
        self.quiet.store(was, Ordering::Relaxed);
        count
    }

    /// Silences the per-hunt log lines (a test that resolves every hunt).
    pub fn set_quiet(&self, quiet: bool) {
        self.quiet.store(quiet, Ordering::Relaxed);
    }
}

const INVALID_PLAYERS: &str = "ExpectedPlayers must contain between one and four account IDs";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionMode {
    /// The first Behemoth defeat ends the hunt (single-Behemoth pursuits, Trials).
    FirstDefeat,
    /// Only the mode's EndOfHuntStarted signal ends it (Hunting Grounds, Escalations, Gauntlet, events,
    /// story missions).
    EndOfHunt,
    /// Never completes and never rewards (the Training Grounds).
    NoCompletion,
}

impl CompletionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FirstDefeat => "FIRST_DEFEAT",
            Self::EndOfHunt => "END_OF_HUNT",
            Self::NoCompletion => "NO_COMPLETION",
        }
    }
}

/// The Hunt Guard completion mode of a matchmaker row (`shared/hunt-manifest/p2p/completionMode.js`).
pub fn completion_mode_for(
    resolver: &HuntResolver,
    matchmaker_hunt_id: &str,
    behemoth_path: Option<&Value>,
) -> Result<CompletionMode, String> {
    let row = resolver
        .find_matchmaker_row(matchmaker_hunt_id)
        .ok_or_else(|| format!("matchmaker_hunts_table has no row for MatchmakerHuntId '{matchmaker_hunt_id}'"))?;
    let game_mode = match row.get("GameModeOverride") {
        None | Some(Value::Null) => String::new(),
        other => js::string(other),
    };
    // The dojo row names a fixed Behemoth, so without this it would be FIRST_DEFEAT.
    if matchmaker_hunt_id.to_lowercase().contains("trainingdojo") || game_mode.to_lowercase().contains("trainingground")
    {
        return Ok(CompletionMode::NoCompletion);
    }
    // Several encounters (or none fixed): a kill is encounter evidence, not hunt finality. Story
    // missions are named because none names a SpecificBehemoth today (2026-10-11).
    let specific = row
        .get("GameModeSpecificData")
        .filter(|v| !v.is_null())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let material = format!("{matchmaker_hunt_id}\n{game_mode}\n{}", jsjson::stringify(&specific)).to_lowercase();
    let multi = ["escalation", "gauntlet", "archonevent", "eventgamemode", "storymission"]
        .iter()
        .any(|word| material.contains(word));
    Ok(if behemoth_path.is_none() || multi {
        CompletionMode::EndOfHunt
    } else {
        CompletionMode::FirstDefeat
    })
}

/// A player-hosted hunt's manifest: the plain one plus its frozen completion mode. A hosted Training
/// Grounds launches like the dedicated hub, without a forced Behemoth (the dojo places its dummies).
pub fn resolve_player_hosted_manifest(
    resolver: &HuntResolver,
    hunt_id: Option<&Value>,
    expected: Option<&Value>,
) -> Result<Value, String> {
    let mut manifest = resolver.resolve_manifest(hunt_id, expected)?;
    let matchmaker_hunt_id = manifest["matchmakerHuntId"].as_str().unwrap_or("").to_owned();
    let behemoth = manifest
        .get("behemothPath")
        .filter(|b| b.as_str() != Some("NO_BEHEMOTH"))
        .cloned();
    let mode = completion_mode_for(resolver, &matchmaker_hunt_id, behemoth.as_ref())?;
    if let Value::Object(map) = &mut manifest {
        if mode == CompletionMode::NoCompletion {
            map.insert("behemothPath".into(), json!("NO_BEHEMOTH"));
        }
        map.insert("guardCompletionMode".into(), json!(mode.as_str()));
    }
    Ok(manifest)
}

// ---- the Director's audits of the tables (src/controllers/gameservers.ts)

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageAudit {
    pub total_player_hunts: usize,
    pub resolvable_player_hunts: usize,
    pub unresolved_player_hunts: Vec<String>,
    pub referenced_matchmaker_rows: usize,
    pub unique_map_assets: Vec<String>,
    pub game_mode_overrides: Vec<String>,
    pub max_player_values: Vec<i64>,
}

/// Every destination a hunt launch resolves through: direct rows and tag-routed Trials through the
/// launch resolver, stale player rows with no destination listed instead of guessed.
pub fn audit_coverage(resolver: &HuntResolver) -> CoverageAudit {
    use std::collections::{BTreeSet, HashSet};
    let rows = &resolver.tables.player_rows;
    let mut unresolved = Vec::new();
    let mut referenced = HashSet::new();
    let mut maps = BTreeSet::new();
    let mut modes = BTreeSet::new();
    let mut max_players = BTreeSet::new();
    for (player_hunt_id, row) in rows {
        let direct: Vec<String> = match row.get("MatchmakerHuntIDs") {
            Some(Value::Array(entries)) => entries
                .iter()
                .filter_map(|e| e.get("RowName").and_then(Value::as_str))
                .filter(|n| !n.is_empty() && *n != "None" && resolver.find_matchmaker_row(n).is_some())
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        };
        let candidates = if direct.is_empty() {
            resolver.tag_routed_matchmaker_hunt_ids(player_hunt_id, row)
        } else {
            direct
        };
        if candidates.is_empty() {
            unresolved.push(player_hunt_id.clone());
            continue;
        }
        for id in candidates {
            let Some(mm) = resolver.find_matchmaker_row(&id) else {
                continue;
            };
            referenced.insert(id.clone());
            let mode = mm
                .get("GameModeOverride")
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty())
                .unwrap_or("(map/default)");
            modes.insert(mode.to_owned());
            if let Some(n) = js::safe_integer(mm.get("MaxPlayers")) {
                max_players.insert(n);
            }
            if let Some(Value::Array(list)) = mm.get("MapList") {
                maps.extend(list.iter().filter_map(usable_map).map(str::to_owned));
            }
        }
    }
    unresolved.sort();
    CoverageAudit {
        total_player_hunts: rows.len(),
        resolvable_player_hunts: rows.len() - unresolved.len(),
        unresolved_player_hunts: unresolved,
        referenced_matchmaker_rows: referenced.len(),
        unique_map_assets: maps.into_iter().collect(),
        game_mode_overrides: modes.into_iter().collect(),
        max_player_values: max_players.into_iter().collect(),
    }
}

#[derive(Debug, Clone)]
pub struct TableAudit {
    pub player_rows: usize,
    pub direct_player_rows: usize,
    pub tag_routed_player_rows: usize,
    pub tag_routed_references: usize,
    pub matchmaker_rows: usize,
    pub problems: Vec<String>,
    pub coverage: CoverageAudit,
}

/// An inward consistency check of the tables (it cannot notice a row that was never exported; the
/// missing-hunt registry learns those from requests). Changes nothing.
pub fn validate_tables(resolver: &HuntResolver) -> TableAudit {
    let has_valid_map = |row: Option<&Value>| match row.and_then(|r| r.get("MapList")) {
        Some(Value::Array(list)) => list.iter().any(|m| usable_map(m).is_some()),
        _ => false,
    };
    let mut problems = Vec::new();
    let (mut direct_rows, mut tag_refs, mut tag_rows) = (0, 0, 0);
    for (player_hunt_id, row) in &resolver.tables.player_rows {
        let direct = match row.get("MatchmakerHuntIDs") {
            Some(Value::Array(entries)) if !entries.is_empty() => entries,
            _ => {
                // Trials route by tag: resolve it, so a broken query shows at boot, not as a failed hunt.
                let resolved = resolver.tag_routed_matchmaker_hunt_ids(player_hunt_id, row);
                if resolved.is_empty() {
                    continue;
                }
                tag_rows += 1;
                let unusable = resolved
                    .iter()
                    .filter(|id| !has_valid_map(resolver.find_matchmaker_row(id)))
                    .count();
                if unusable > 0 {
                    problems.push(format!(
                        "{player_hunt_id} -> {unusable}/{} tag-routed rows have no valid map",
                        resolved.len()
                    ));
                }
                continue;
            }
        };
        direct_rows += 1;
        for entry in direct {
            let name = entry.get("RowName");
            // "None" is the 1.12 table's tag-routed sentinel, not a dangling reference.
            if name.and_then(Value::as_str) == Some("None") {
                tag_refs += 1;
                continue;
            }
            let row = name
                .and_then(Value::as_str)
                .and_then(|n| resolver.find_matchmaker_row(n));
            let shown = js::string(name);
            if row.is_none() {
                problems.push(format!("{player_hunt_id} -> missing MatchmakerHunt '{shown}'"));
                continue;
            }
            if !has_valid_map(row) {
                problems.push(format!("{player_hunt_id} -> {shown} has no valid map"));
            }
        }
    }
    TableAudit {
        player_rows: resolver.tables.player_rows.len(),
        direct_player_rows: direct_rows,
        tag_routed_player_rows: tag_rows,
        tag_routed_references: tag_refs,
        matchmaker_rows: resolver.tables.matchmaker_rows().map_or(0, Map::len),
        problems,
        coverage: audit_coverage(resolver),
    }
}

/// Where each lettered hunt family stops ("ShatteredIsles_Island: A-R (18)"): a family that ends at R
/// is the cue to check whether newer content was ever exported.
pub fn family_coverage(player_hunt_ids: &[&str]) -> Vec<String> {
    let mut families: Vec<(String, Vec<char>)> = Vec::new();
    for id in player_hunt_ids {
        let Some(last) = id.chars().last().filter(char::is_ascii_uppercase) else {
            continue;
        };
        let prefix = &id[..id.len() - 1];
        match families.iter_mut().find(|(p, _)| p == prefix) {
            Some((_, suffixes)) => suffixes.push(last),
            None => families.push((prefix.to_owned(), vec![last])),
        }
    }
    // As JavaScript sorts [prefix, suffixes] pairs: by their text "prefix,A,B".
    let text = |(p, s): &(String, Vec<char>)| {
        std::iter::once(p.clone())
            .chain(s.iter().map(char::to_string))
            .collect::<Vec<_>>()
            .join(",")
    };
    families.sort_by_key(text);
    let mut lines = Vec::new();
    for (prefix, mut suffixes) in families {
        // Two-member "families" are usually coincidence, not a lettered series.
        if suffixes.len() < 3 {
            continue;
        }
        suffixes.sort_unstable();
        let (first, last) = (suffixes[0], suffixes[suffixes.len() - 1]);
        let expected = last as usize - first as usize + 1;
        let gap = if expected > suffixes.len() {
            " (NON-CONTIGUOUS)"
        } else {
            ""
        };
        lines.push(format!(
            "[HuntTableCoverage] {prefix}: {first}-{last} ({}){gap}",
            suffixes.len()
        ));
    }
    lines
}
