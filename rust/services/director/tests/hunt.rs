#![allow(clippy::unwrap_used, clippy::expect_used)] // tests

//! The hunt manifest resolver: `huntManifestResolver.test.ts` and `huntManifestCompletion.test.ts`
//! over small synthetic tables, then the `*.gamedata.test.ts` files over the real tables (skipped,
//! as the Node runner skips them, when `paradox/director/game-data` or PARADOX_GAME_DATA_DIR lacks
//! them), including the golden digest the Metagame's parity test checks too.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use director::hunt::{self, HuntLog, HuntResolver, ResolverOptions, Tables};
use serde_json::{Map, Value, json};
use service_kit::jsjson;

const WEEK_MS: i64 = 7 * 24 * 60 * 60_000;

fn none_behemoth() -> Value {
    json!({ "BehemothAsset": { "AssetPathName": "None" } })
}

fn boss_behemoth() -> Value {
    json!({ "BehemothAsset": { "AssetPathName": "/Game/Monsters/test/boss_bp.boss_bp_C" } })
}

fn map(asset: &str) -> Value {
    json!([{ "MapAssetName": asset }])
}

fn rows(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => unreachable!(),
    }
}

fn main_rows() -> Map<String, Value> {
    rows(json!({
        "MM_Plain": { "MapList": map("/Game/Maps/test/plain.plain"), "GameModeOverride": "", "HuntTags": [], "SpecificBehemoth": none_behemoth() },
        "MM_Boss": { "MapList": map("/Game/Maps/test/boss.boss"), "GameModeOverride": "", "SpecificBehemoth": boss_behemoth() },
        "CR19_MatchmakerHunt_ShatteredIsles_TrainingDojo": {
            "MapList": map("/Game/Maps/islands/dojo/training_dojo_persistent.training_dojo_persistent"),
            "GameModeOverride": "BPGM_ArchonTrainingGrounds",
            "SpecificBehemoth": { "BehemothAsset": { "AssetPathName": "/Game/Monsters/dummy/dummy_fulgar_bp.dummy_fulgar_bp_C" } }
        },
        "MatchmakerHunt_StoryMission_Linnea_01": { "MapList": map("/Game/Maps/test/story.story"), "GameModeOverride": "", "SpecificBehemoth": boss_behemoth() },
        "NoMapRow": { "MapList": map("Engine/Maps/none"), "GameModeOverride": "", "SpecificBehemoth": none_behemoth() }
    }))
}

fn tables() -> Tables {
    let arena_easy = rows(json!({
        "Arena_MatchmakerHunt_Easy_001": { "HuntTags": ["Hunt.Arena.Easy"], "MapList": map("/Game/Maps/arena/a1.a1"), "GameModeOverride": "", "SpecificBehemoth": none_behemoth() },
        "Arena_MatchmakerHunt_Easy_002": { "HuntTags": ["Hunt.Arena.Easy.Week"], "MapList": map("/Game/Maps/arena/a2.a2"), "GameModeOverride": "", "SpecificBehemoth": none_behemoth() },
        "Arena_MatchmakerHunt_Hard_001": { "HuntTags": ["Hunt.Arena.Hard"], "MapList": map("/Game/Maps/arena/h1.h1"), "GameModeOverride": "", "SpecificBehemoth": none_behemoth() }
    }));
    // ANY(Hunt.Arena.Easy) in the raw CatalogExporter shape.
    let easy_query = json!({ "tokenStream": [0, 1, 1, 1, 0], "tagDictionary": ["Hunt.Arena.Easy"] });
    let player_rows = rows(json!({
        "Plain_Hunt": { "MatchmakerHuntIDs": [{ "RowName": "MM_Plain" }] },
        "Boss_Hunt": { "MatchmakerHuntIDs": [{ "RowName": "MM_Boss" }] },
        "Training_Hunt": { "MatchmakerHuntIDs": [{ "RowName": "CR19_MatchmakerHunt_ShatteredIsles_TrainingDojo" }] },
        "Story_Hunt": { "MatchmakerHuntIDs": [{ "RowName": "MatchmakerHunt_StoryMission_Linnea_01" }] },
        "Dangling_Hunt": { "MatchmakerHuntIDs": [{ "RowName": "NoSuchRow" }] },
        "Unmapped_Hunt": { "MatchmakerHuntIDs": [{ "RowName": "NoMapRow" }] },
        "Trials_Hunt": {
            "MatchmakerHuntIDs": [],
            "MatchmakerHuntsByTag": [{ "MatchmakerTable": { "ObjectName": "DataTable'arena_easy_matchmaker_hunts'" }, "HuntTags": [easy_query] }]
        }
    }));
    let mut sources = vec![
        ("matchmaker_hunts_table".to_owned(), main_rows()),
        ("arena_easy_matchmaker_hunts".to_owned(), arena_easy),
    ];
    for name in [
        "arena_hard_matchmaker_hunts",
        "arena_elite_matchmaker_hunts",
        "arena_hard_matchmaker_hunts_new",
        "arena_elite_matchmaker_hunts_new",
    ] {
        sources.push((name.to_owned(), Map::new()));
    }
    Tables {
        player_rows,
        matchmaker_sources: sources,
    }
}

#[derive(Clone, Default)]
struct Capture {
    infos: Arc<Mutex<Vec<String>>>,
    warnings: Arc<Mutex<Vec<String>>>,
}

impl HuntLog for Capture {
    fn info(&self, message: &str) {
        self.infos.lock().unwrap().push(message.to_owned());
    }

    fn warn(&self, message: &str) {
        self.warnings.lock().unwrap().push(message.to_owned());
    }
}

/// The first candidate wins every random pick; the clock is the caller's.
fn resolver_with(tables: Tables, now: i64, log: Capture, missing: Arc<Mutex<Vec<String>>>) -> HuntResolver {
    let mut options = ResolverOptions::new(tables);
    options.random_int = Box::new(|min, _| min);
    options.now = Box::new(move || now);
    options.log = Box::new(log);
    options.on_missing_hunt = Box::new(move |id| missing.lock().unwrap().push(id.to_owned()));
    HuntResolver::new(options)
}

fn resolver(now: i64) -> HuntResolver {
    resolver_with(tables(), now, Capture::default(), Arc::default())
}

fn resolve(r: &HuntResolver, hunt: &str, expected: Value) -> Result<Value, String> {
    r.resolve_manifest(Some(&json!(hunt)), Some(&expected))
}

#[test]
fn tag_queries_any_all_none_nesting_and_prefixes() {
    let any = json!({ "QueryTokenStream": [0, 1, 1, 1, 0], "TagDictionary": [{ "TagName": "Hunt.Arena.Easy" }] });
    let tags = |t: &[&str]| t.iter().map(|s| json!(s)).collect::<Vec<_>>();
    assert!(hunt::evaluate_tag_query(&any, &tags(&["Hunt.Arena.Easy"])));
    assert!(
        hunt::evaluate_tag_query(&any, &tags(&["Hunt.Arena.Easy.Week"])),
        "a child tag satisfies its parent"
    );
    assert!(!hunt::evaluate_tag_query(&any, &tags(&["Hunt.Arena.Hard"])));
    assert!(
        !hunt::evaluate_tag_query(&any, &tags(&["Hunt.Arena.EasyX"])),
        "whole segments only"
    );

    let all = json!({ "tokenStream": [0, 1, 2, 2, 0, 1], "tagDictionary": ["A", "B"] });
    assert!(!hunt::evaluate_tag_query(&all, &tags(&["A"])));
    assert!(hunt::evaluate_tag_query(&all, &tags(&["A", "B"])));

    let none = json!({ "tokenStream": [0, 1, 3, 1, 0], "tagDictionary": ["Hunt.Arena.Easy"] });
    assert!(hunt::evaluate_tag_query(&none, &tags(&["Hunt.Arena.Hard"])));
    assert!(!hunt::evaluate_tag_query(&none, &tags(&["Hunt.Arena.Easy"])));

    // ALL( ANY(A), NONE(B) )
    let nested = json!({ "tokenStream": [0, 1, 5, 2, 1, 1, 0, 3, 1, 1], "tagDictionary": ["A", "B"] });
    assert!(hunt::evaluate_tag_query(&nested, &tags(&["A"])));
    assert!(!hunt::evaluate_tag_query(&nested, &tags(&["A", "B"])));

    for broken in [
        json!({ "tokenStream": [0, 0, 1, 1, 0], "tagDictionary": ["A"] }),
        json!({ "tokenStream": [0, 1, 9, 1, 0], "tagDictionary": ["A"] }),
        json!({ "tokenStream": [0, 1, 1, 1], "tagDictionary": ["A"] }),
        json!({}),
    ] {
        assert!(!hunt::evaluate_tag_query(&broken, &tags(&["A"])), "{broken}");
    }
}

#[test]
fn the_trials_week_is_the_utc_bucket_wrapping_at_181() {
    assert_eq!(
        hunt::current_trials_week(0, hunt::DEFAULT_TRIALS_ROTATION_MINUTES),
        "001"
    );
    assert_eq!(
        hunt::current_trials_week(WEEK_MS, hunt::DEFAULT_TRIALS_ROTATION_MINUTES),
        "002"
    );
    assert_eq!(
        hunt::current_trials_week(WEEK_MS * 181, hunt::DEFAULT_TRIALS_ROTATION_MINUTES),
        "001"
    );
    assert_eq!(hunt::current_trials_week(30 * 60_000, 30), "002");
}

#[test]
fn trials_rotation_minutes_default_and_invalid_values() {
    let log = Capture::default();
    assert_eq!(hunt::parse_trials_rotation_minutes(None, &log), 10_080);
    assert_eq!(hunt::parse_trials_rotation_minutes(Some("30"), &log), 30);
    assert!(log.warnings.lock().unwrap().is_empty());
    for bad in ["0", "600000", "soon"] {
        assert_eq!(hunt::parse_trials_rotation_minutes(Some(bad), &log), 10_080);
    }
    let warnings = log.warnings.lock().unwrap();
    assert_eq!(warnings.len(), 3);
    assert_eq!(
        warnings[0],
        "[TrialsRotation] invalid TRIALS_ROTATION_MINUTES='0'; using 10080"
    );
}

#[test]
fn expected_player_strings_are_uid_hunt_pairs() {
    assert_eq!(hunt::expected_player_args([]), "");
    assert_eq!(hunt::expected_player_args([("a", "H"), ("b", "H")]), "a:H,b:H");
}

#[test]
fn a_plain_manifest_carries_map_behemoth_and_party() {
    let r = resolver(0);
    let manifest = resolve(&r, "Plain_Hunt", json!(["acc-1"])).unwrap();
    assert_eq!(
        jsjson::stringify(&manifest),
        r#"{"huntId":"Plain_Hunt","matchmakerHuntId":"MM_Plain","mapPath":"/Game/Maps/test/plain","behemothPath":"NO_BEHEMOTH","expectedPlayerString":"acc-1:Plain_Hunt"}"#
    );
    let boss = resolve(&r, "Boss_Hunt", json!(["acc-1", "acc-2"])).unwrap();
    assert_eq!(boss["behemothPath"], "/Game/Monsters/test/boss_bp.boss_bp_C");
    assert_eq!(boss["expectedPlayerString"], "acc-1:Boss_Hunt,acc-2:Boss_Hunt");
}

#[test]
fn an_unknown_or_prototype_named_hunt_is_a_missing_row_reported_once() {
    let missing: Arc<Mutex<Vec<String>>> = Arc::default();
    let r = resolver_with(tables(), 0, Capture::default(), missing.clone());
    for id in ["No_Such_Hunt", "constructor", "__proto__"] {
        assert_eq!(
            resolve(&r, id, json!(["acc-1"])).unwrap_err(),
            format!("player_hunts_table has no usable row for HuntId '{id}' (missing row or empty MatchmakerHuntIDs)")
        );
    }
    assert_eq!(*missing.lock().unwrap(), ["No_Such_Hunt", "constructor", "__proto__"]);
}

#[test]
fn refuses_unresolvable_and_unmapped_rows_and_bad_parties_with_the_directors_messages() {
    let r = resolver(0);
    assert_eq!(
        resolve(&r, "Dangling_Hunt", json!(["acc-1"])).unwrap_err(),
        "player_hunts_table row 'Dangling_Hunt' has no resolvable 1.12 MatchmakerHuntIDs"
    );
    assert_eq!(
        resolve(&r, "Unmapped_Hunt", json!(["acc-1"])).unwrap_err(),
        "matchmaker_hunts_table row 'NoMapRow' has no valid /Game/ map asset path"
    );
    for bad in [
        json!([]),
        json!(["a", "b", "c", "d", "e"]),
        json!([""]),
        json!(["a", 7]),
        json!(null),
        json!("acc-1"),
    ] {
        assert_eq!(
            resolve(&r, "Plain_Hunt", bad.clone()).unwrap_err(),
            "ExpectedPlayers must contain between one and four account IDs",
            "{bad}"
        );
    }
}

#[test]
fn a_trials_hunt_takes_the_featured_week_or_a_numbered_candidate() {
    let featured = resolver(WEEK_MS);
    let manifest = resolve(&featured, "Trials_Hunt", json!(["acc-1"])).unwrap();
    assert_eq!(manifest["matchmakerHuntId"], "Arena_MatchmakerHunt_Easy_002");
    assert_eq!(manifest["mapPath"], "/Game/Maps/arena/a2");

    let log = Capture::default();
    let unfeatured = resolver_with(tables(), 2 * WEEK_MS, log.clone(), Arc::default());
    assert_eq!(
        unfeatured.matchmaker_hunt_id_for("Trials_Hunt").unwrap(),
        "Arena_MatchmakerHunt_Easy_001"
    );
    assert!(
        log.warnings
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("no arena row matched featured week _003"))
    );
    assert!(
        log.infos
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("Trials_Hunt resolved to 2 matchmaker row(s) via tag query"))
    );
}

#[test]
fn counts_the_resolvable_rows_without_logging_each() {
    let log = Capture::default();
    let r = resolver_with(tables(), 0, log.clone(), Arc::default());
    // Plain, Boss, Training, Story and Trials resolve; Dangling and Unmapped do not.
    assert_eq!(r.count_resolvable_player_hunts(), 5);
    assert!(log.infos.lock().unwrap().is_empty() && log.warnings.lock().unwrap().is_empty());
}

#[test]
fn loads_the_seven_tables_with_a_bom_and_refuses_a_missing_or_non_datatable_one() {
    let dir = tempfile::tempdir().unwrap();
    let files: Vec<&str> = std::iter::once(hunt::PLAYER_TABLE)
        .chain(hunt::MATCHMAKER_TABLES.iter().map(|(_, f)| *f))
        .collect();
    for file in &files {
        let body = jsjson::stringify(
            &json!([{ "Type": "DataTable", "Rows": { format!("Row_{}", file.replace(['.', '-'], "_")): {} } }]),
        );
        let body = if *file == hunt::PLAYER_TABLE {
            format!("\u{feff}{body}")
        } else {
            body
        };
        std::fs::write(dir.path().join(file), body).unwrap();
    }
    let loaded = hunt::load_tables(dir.path()).unwrap();
    assert_eq!(loaded.player_rows.len(), 1);
    assert_eq!(loaded.matchmaker_sources.len(), 6);
    let third = hunt::MATCHMAKER_TABLES[2].1;
    std::fs::remove_file(dir.path().join(third)).unwrap();
    let error = hunt::load_tables(dir.path()).unwrap_err();
    assert!(
        error.starts_with("[hunt-manifest] Required hunt table is missing: ")
            && error.ends_with("arena_hard_matchmaker_hunts.json"),
        "{error}"
    );
    std::fs::write(dir.path().join(third), r#"{"Rows":{}}"#).unwrap();
    assert!(
        hunt::load_tables(dir.path())
            .unwrap_err()
            .ends_with("is not a DataTable export (no Rows)")
    );
}

#[test]
fn the_player_hosted_manifest_freezes_the_completion_mode() {
    let r = resolver(0);
    let hosted =
        |id: &str| hunt::resolve_player_hosted_manifest(&r, Some(&json!(id)), Some(&json!(["acc-1"]))).unwrap();
    assert_eq!(
        hosted("Plain_Hunt")["guardCompletionMode"],
        "END_OF_HUNT",
        "no fixed Behemoth"
    );
    let boss = hosted("Boss_Hunt");
    assert_eq!(boss["guardCompletionMode"], "FIRST_DEFEAT");
    assert_eq!(boss["behemothPath"], "/Game/Monsters/test/boss_bp.boss_bp_C");
    assert_eq!(
        hosted("Story_Hunt")["guardCompletionMode"],
        "END_OF_HUNT",
        "a story mission is multi-objective"
    );
    let training = hosted("Training_Hunt");
    assert_eq!(training["guardCompletionMode"], "NO_COMPLETION");
    assert_eq!(training["behemothPath"], "NO_BEHEMOTH");
    assert_eq!(
        resolve(&r, "Training_Hunt", json!(["acc-1"])).unwrap()["behemothPath"],
        "/Game/Monsters/dummy/dummy_fulgar_bp.dummy_fulgar_bp_C"
    );
    let keys: Vec<&String> = training.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "huntId",
            "matchmakerHuntId",
            "mapPath",
            "behemothPath",
            "expectedPlayerString",
            "guardCompletionMode"
        ]
    );
}

// ---- the real tables (*.gamedata.test.ts)

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn real_tables() -> Option<Tables> {
    let dir = std::env::var_os("PARADOX_GAME_DATA_DIR")
        .map_or_else(|| repo().join("paradox/director/game-data"), PathBuf::from);
    match hunt::load_tables(&dir) {
        Ok(tables) => Some(tables),
        Err(error) => {
            eprintln!("[tests] skipping a game-data test: {error}");
            None
        }
    }
}

fn real_resolver(choice_last: bool) -> Option<HuntResolver> {
    let mut options = ResolverOptions::new(real_tables()?);
    options.random_int = Box::new(move |min, max| if choice_last { max - 1 } else { min });
    // manifestDigest.js's PINNED_NOW_MS
    options.now = Box::new(|| 1_800_000_000_000);
    Some(HuntResolver::new(options))
}

#[test]
fn gamedata_all_launch_destinations_are_audited_and_stale_rows_explicit() {
    let Some(r) = real_resolver(false) else { return };
    let coverage = hunt::audit_coverage(&r);
    // 1.14.7: 161 exported rows and 7 older vendored ones the export no longer has.
    assert_eq!(coverage.total_player_hunts, 168);
    assert_eq!(coverage.resolvable_player_hunts, 161);
    let stale: Vec<String> = (2..=8)
        .map(|n| format!("CR19_PlayerHunt_Expedition_Island0{n}"))
        .collect();
    assert_eq!(coverage.unresolved_player_hunts, stale);
    assert!(
        coverage.referenced_matchmaker_rows >= 600,
        "Trials auxiliary rows, the _new tables included"
    );
    assert!(coverage.unique_map_assets.len() >= 39);
    assert!(coverage.game_mode_overrides.iter().any(|m| m == "(map/default)"));
    assert_eq!(coverage.max_player_values, [4, 12]);
    assert!(
        hunt::validate_tables(&r).problems.is_empty(),
        "the consistency audit stays clean"
    );
}

#[test]
fn gamedata_representative_families_freeze_valid_manifests() {
    let Some(r) = real_resolver(false) else { return };
    for id in [
        "ShatteredIsles_IslandA",
        "ShatteredIsles_IslandU",
        "CR19_PlayerHunt_Escalation_Glitter_Hard",
        "CR19_PlayerHunt_Arena_Hard",
        "11A_PlayerHunt_StoryMission_01_Terramane",
    ] {
        let manifest = resolve(&r, id, json!(["player-a", "player-b"])).unwrap();
        assert_eq!(manifest["huntId"], id);
        assert!(manifest["mapPath"].as_str().unwrap().starts_with("/Game/Maps/"));
        assert_eq!(manifest["expectedPlayerString"], format!("player-a:{id},player-b:{id}"));
    }
    for (name, row) in &r.tables.player_rows {
        if let Some(kind) = row.get("MatchmakingGameType") {
            let text = service_kit::js::string(Some(kind));
            let ok = text
                .strip_prefix("EMatchmakingGameType::")
                .is_some_and(|k| !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphabetic()));
            assert!(ok, "{name}: MatchmakingGameType is stored by name");
        }
    }
}

#[test]
fn gamedata_completion_modes_of_the_real_rows() {
    let Some(r) = real_resolver(false) else { return };
    let hosted =
        |id: &str| hunt::resolve_player_hosted_manifest(&r, Some(&json!(id)), Some(&json!(["player-a"]))).unwrap();
    assert_eq!(hosted("ShatteredIsles_IslandA")["guardCompletionMode"], "END_OF_HUNT");
    assert_eq!(
        hosted("CR19_PlayerHunt_Escalation_Glitter_Hard")["guardCompletionMode"],
        "END_OF_HUNT"
    );
    assert_eq!(
        hosted("CR19_PlayerHunt_Arena_Hard")["guardCompletionMode"],
        "FIRST_DEFEAT"
    );
    let boss = json!("/Game/Monsters/test/test_bp.test_bp_C");
    for row in [
        "11A_MatchmakerHunt_StoryMission_01_Terramane",
        "11A_MatchmakerHunt_StoryMission_02_Sally_Terrogg",
        "MatchmakerHunt_Event_Dark_Harvest_01",
        "MatchmakerHunt_Event_Dark_Harvest_Hard_01",
        "MatchmakerHunt_Event_Ramsgiving_01",
        "MatchmakerHunt_Event_Ramsgiving_02",
        "MatchmakerHunt_Event_Snowball_Fight_Test_00",
        "MatchmakerHunt_Event_Anniversary_01",
        "MatchmakerHunt_Event_Anniversary_02",
        "MatchmakerHunt_StoryMission_Linnea_01",
    ] {
        for behemoth in [None, Some(&boss)] {
            assert_eq!(
                hunt::completion_mode_for(&r, row, behemoth).unwrap(),
                hunt::CompletionMode::EndOfHunt,
                "{row}"
            );
        }
    }
    let training = hosted("ShatteredIsles_TrainingDojo");
    assert_eq!(training["guardCompletionMode"], "NO_COMPLETION");
    assert_eq!(training["behemothPath"], "NO_BEHEMOTH");
    assert_eq!(training["mapPath"], "/Game/Maps/islands/dojo/training_dojo_persistent");
    let fulgar = json!("/Game/Monsters/fulgar/fulgar_normal_bp.fulgar_normal_bp_C");
    assert_eq!(
        hunt::completion_mode_for(&r, "CR19_MatchmakerHunt_Fulgar", Some(&fulgar)).unwrap(),
        hunt::CompletionMode::FirstDefeat
    );
}

/// `manifestDigest.js`: every manifest of the real tables, plain and player-hosted, with the clock and
/// the random choice pinned, must give the digest the Director's resolver gave before the M1 move.
#[test]
fn gamedata_manifests_reproduce_the_golden_digest() {
    use sha2::Digest;
    let (Some(first), Some(last)) = (real_resolver(false), real_resolver(true)) else {
        return;
    };
    let golden: Value = serde_json::from_str(
        &std::fs::read_to_string(repo().join("paradox/shared/hunt-manifest/golden-digest.json")).unwrap(),
    )
    .unwrap();
    let outcome = |result: Result<Value, String>| match result {
        Ok(manifest) => jsjson::stringify(&manifest),
        Err(error) => format!("ERR {error}"),
    };
    let ids: Vec<String> = first.tables.player_rows.keys().cloned().collect();
    let expected_sets = [json!(["acc-1"]), json!(["acc-1", "acc-2", "acc-3", "acc-4"])];
    let unknown = ["__digest_unknown_hunt__", "", " ", "ShatteredIsles_IslandA "];
    let invalid = [
        json!([]),
        json!(["acc-1", "acc-2", "acc-3", "acc-4", "acc-5"]),
        json!([""]),
        json!(["acc-1", 7]),
        json!(null),
        json!("acc-1"),
    ];
    let mut lines = Vec::new();
    for (choice, r) in [("first", &first), ("last", &last)] {
        let mut both = |key: String, hunt: Value, expected: &Value| {
            lines.push(format!(
                "plain|{key}|{}",
                outcome(r.resolve_manifest(Some(&hunt), Some(expected)))
            ));
            lines.push(format!(
                "hosted|{key}|{}",
                outcome(hunt::resolve_player_hosted_manifest(r, Some(&hunt), Some(expected)))
            ));
        };
        for id in &ids {
            for expected in &expected_sets {
                both(
                    format!("{choice}|{id}|{}", jsjson::stringify(expected)),
                    json!(id),
                    expected,
                );
            }
        }
        let one = json!(["acc-1"]);
        for id in unknown {
            both(
                format!("{choice}|{}|{}", jsjson::stringify(&json!(id)), jsjson::stringify(&one)),
                json!(id),
                &one,
            );
        }
        for expected in &invalid {
            both(
                format!("{choice}|{}|{}", ids[0], jsjson::stringify(expected)),
                json!(ids[0]),
                expected,
            );
        }
    }
    let digest: String = sha2::Sha256::digest(lines.join("\n").as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    // Tables other than the golden digest's: HUNT_MANIFEST_DIGEST is the Node resolver's digest of
    // them (BuildManifestDump and DigestOf in paradox/shared/hunt-manifest/manifestDigest.js).
    if let Ok(expected) = std::env::var("HUNT_MANIFEST_DIGEST") {
        assert_eq!(digest, expected, "{} rows, {} lines", ids.len(), lines.len());
        return;
    }
    assert_eq!(
        ids.len() as u64,
        golden["playerRows"].as_u64().unwrap(),
        "player hunt rows"
    );
    assert_eq!(lines.len() as u64, golden["lines"].as_u64().unwrap(), "manifest lines");
    assert_eq!(digest, golden["digest"].as_str().unwrap());
}
