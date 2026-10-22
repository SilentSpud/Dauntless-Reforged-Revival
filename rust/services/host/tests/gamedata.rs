//! The Node kit's tests of the game-data commands (`legacy/ReforgedHost/test/captures.test.ts`,
//! `gamepack.test.ts`, `preset.test.ts`, `progression.test.ts`, and the "runtime publishing" and
//! "game data" cases of `kit.test.ts`), and the files those commands write compared byte for byte with
//! the Node kit's (`tests/fixtures/node/gamedata/`, made by `tests/fixtures/node/gen-gamedata.mjs`
//! with the Node kit's own code). The launcher's import of a pack (`gamepack.test.ts`'s last case) is
//! RA12's.

#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

use std::cell::RefCell;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{DateTime, Utc};
use host::captures::{CaptureFetch, CapturePin, ensure_captures, missing_captures, read_capture_pin};
use host::gamedata::{
    BACKEND_GAME_DATA, GameDataKind, GenerateOptions, Generators, SUPPLIED_SOURCES, game_data_state, game_data_steps,
    generate_game_data, generate_game_data_with, use_examples,
};
use host::gamepack::{
    PACK_FORMAT, export_game_data_pack, import_game_data_pack, pack_json, read_game_data_pack, sha256_hex,
};
use host::paths::Paths;
use host::preset::{CreateOptions, apply_preset, apply_preset_at, create_preset, read_preset_manifest};
use host::progression::{
    ACTIVE_HUNT_PASS, GENERATED_BY, PROGRESSION_EXPORT_DIR, SourceWritten, build_progression_config, progression_text,
    write_progression_source,
};
use host::publish::{
    SERVER_RUNTIME_FLAGS, default_sources, ensure_server_flags, install_server_runtime, publish_runtime, sha256_file,
};
use host::run::RunResult;
use serde_json::{Value, json};
use tempfile::TempDir;

fn write(file: &Path, data: impl AsRef<[u8]>) {
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, data).unwrap();
}

fn read(file: &Path) -> String {
    std::fs::read_to_string(file).unwrap_or_else(|e| panic!("{}: {e}", file.display()))
}

fn date(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text).unwrap().to_utc()
}

fn host(dir: &TempDir) -> Paths {
    Paths::new(dir.path())
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("node")
        .join("gamedata")
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("..")
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            write(&target, std::fs::read(entry.path()).unwrap());
        }
    }
}

// ------------------------------------------------------------------ store captures

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// A capture repository with one store file, committed with CRLF line endings to check the bytes are
/// the committed ones.
fn make_captures(origin: &TempDir) -> (CapturePin, String) {
    let o = origin.path();
    git(o, &["init", "--quiet"]);
    git(o, &["config", "uploadpack.allowReachableSHA1InWant", "true"]);
    git(o, &["config", "core.autocrlf", "false"]);
    let data = "[\r\n  {\"id\": \"trials_pr_frank\"}\r\n]\r\n".to_owned();
    write(
        &o.join("Store").join("Product").join("Skus").join("ladyluckstore.json"),
        &data,
    );
    git(o, &["add", "-A"]);
    // A throwaway repository: never signed with the machine's signing setup.
    git(
        o,
        &["-c", "commit.gpgsign=false", "commit", "--quiet", "-m", "captures"],
    );
    let pin = CapturePin {
        name: "Captures".into(),
        repository: o.to_string_lossy().into_owned(),
        commit: git(o, &["rev-parse", "HEAD"]),
        files: vec![("Store/Product/Skus/ladyluckstore.json".into(), sha256_hex(&data))],
    };
    (pin, data)
}

fn pin_json(pin: &CapturePin) -> String {
    let files: serde_json::Map<String, Value> = pin.files.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    json!({ "name": pin.name, "repository": pin.repository, "commit": pin.commit, "files": files }).to_string()
}

#[test]
fn the_committed_pin_names_a_commit_and_a_hash_for_every_store_file_the_generators_read() {
    let pin = read_capture_pin(&repo().join("paradox").join("backend").join("store-captures.json")).unwrap();
    assert_eq!(
        pin.repository,
        "https://github.com/EisigesEis/DauntlessEndpointDocumentation"
    );
    let mut names: Vec<&str> = pin.files.iter().map(|(f, _)| f.rsplit('/').next().unwrap()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "awakening_webstore.json",
            "dyes.json",
            "hp_level_skip_rank.json",
            "huntpass_store.json",
            "ladyluckstore.json"
        ]
    );
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("pin.json");
    write(
        &file,
        json!({ "repository": "https://x/y", "commit": "0".repeat(40), "files": { "../evil.json": "0".repeat(64) } })
            .to_string(),
    );
    assert!(read_capture_pin(&file).unwrap_err().to_string().contains("plain paths"));
    if cfg!(windows) {
        // A drive or a backslash would leave the captures folder once joined (stricter than Node).
        for name in ["C:x/evil.json", r"Store\evil.json"] {
            let files = json!({ name: "0".repeat(64) });
            write(
                &file,
                json!({ "repository": "https://x/y", "commit": "0".repeat(40), "files": files }).to_string(),
            );
            assert!(
                read_capture_pin(&file).unwrap_err().to_string().contains("plain paths"),
                "{name}"
            );
        }
    }
    write(
        &file,
        json!({ "repository": "ftp://x/y", "commit": "0".repeat(40) }).to_string(),
    );
    assert!(read_capture_pin(&file).unwrap_err().to_string().contains("https URL"));
    write(
        &file,
        json!({ "repository": "https://x/y", "commit": "abc" }).to_string(),
    );
    assert!(
        read_capture_pin(&file)
            .unwrap_err()
            .to_string()
            .contains("40-character")
    );
    write(
        &file,
        json!({ "repository": "https://x/y", "commit": "0".repeat(40), "files": {} }).to_string(),
    );
    assert!(read_capture_pin(&file).unwrap_err().to_string().ends_with("no files"));
}

#[test]
fn captures_are_fetched_at_the_pinned_commit_into_the_folder_the_generators_read_and_checked() {
    let origin = TempDir::new().unwrap();
    let (pin, data) = make_captures(&origin);
    let dir = TempDir::new().unwrap();
    let p = host(&dir);
    assert_eq!(missing_captures(&p, &pin), ["Store/Product/Skus/ladyluckstore.json"]);
    assert_eq!(ensure_captures(&p, &pin, None).unwrap(), CaptureFetch::Fetched);
    let file = p
        .store_captures
        .join("Store")
        .join("Product")
        .join("Skus")
        .join("ladyluckstore.json");
    assert_eq!(std::fs::read(&file).unwrap(), data.as_bytes(), "byte for byte");
    assert!(missing_captures(&p, &pin).is_empty());
    assert_eq!(ensure_captures(&p, &pin, None).unwrap(), CaptureFetch::AlreadyReady);

    // A file that does not match its pinned hash is never written.
    let bad_dir = TempDir::new().unwrap();
    let q = host(&bad_dir);
    let wrong = CapturePin {
        files: vec![("Store/Product/Skus/ladyluckstore.json".into(), "0".repeat(64))],
        ..pin.clone()
    };
    assert!(
        ensure_captures(&q, &wrong, None)
            .unwrap_err()
            .to_string()
            .contains("does not match")
    );
    let absent = CapturePin {
        files: vec![("Store/Product/Skus/nothing.json".into(), "0".repeat(64))],
        ..pin
    };
    let error = ensure_captures(&q, &absent, None).unwrap_err().to_string();
    assert!(error.contains("has no Store/Product/Skus/nothing.json"), "{error}");
    assert!(!q.store_captures.exists());
}

#[test]
fn lady_lucks_store_is_built_from_the_live_rotation_unless_the_host_supplies_a_response() {
    let origin = TempDir::new().unwrap();
    let (pin, _) = make_captures(&origin);
    let dir = TempDir::new().unwrap();
    let p = host(&dir);
    write(&p.captures_pin, pin_json(&pin));
    write(&p.exports.join("catalog_1_14_7.jsonl"), "{}\n");
    write(&p.backend_game_data.join("ladyluck_store.example.json"), "[]\n");
    let lady_luck = |p: &Paths| {
        let steps = game_data_steps(p, false);
        let step = steps.into_iter().find(|s| s.title == "Lady Luck store").unwrap();
        (step.args.clone(), step.skip())
    };
    let (_, skip) = lady_luck(&p);
    assert!(skip.unwrap_or_default().contains("store captures are not there"));
    ensure_captures(&p, &pin, None).unwrap();
    assert_eq!(lady_luck(&p), (vec!["run", "ladyluck:enrich"], None));

    // The enrichment runs on an existing store file: the example goes in first.
    let fetched = RefCell::new(Vec::new());
    let lines = RefCell::new(Vec::<String>::new());
    let fetch = |_: &Paths, q: &CapturePin, _: &Path| {
        fetched.borrow_mut().push(q.commit.clone());
        Ok(CaptureFetch::AlreadyReady)
    };
    let run = |p: &Paths, step: &host::gamedata::Step<'_>, _: &Path| {
        assert_eq!(step.args, ["run", "ladyluck:enrich"]);
        let there = p.backend_game_data.join("ladyluck_store.json").exists();
        Ok(RunResult {
            code: if there { 0 } else { 1 },
            last_line: None,
            output: String::new(),
        })
    };
    let opts = GenerateOptions {
        examples_for_missing: false,
        log_file: p.logs.join("gamedata.log"),
        only: Some(vec!["Lady Luck store".into()]),
    };
    let hooks = Generators {
        fetch_captures: &fetch,
        run_step: &run,
    };
    let r = generate_game_data_with(&p, &opts, &mut |l| lines.borrow_mut().push(l.to_owned()), &hooks).unwrap();
    let lines = lines.into_inner();
    assert!(r.ok && r.failed.is_empty(), "{}", lines.join("\n"));
    assert_eq!(fetched.into_inner(), std::slice::from_ref(&pin.commit));
    assert!(
        lines.iter().any(|l| l.starts_with("- Store captures (Captures at ")),
        "{}",
        lines.join("\n")
    );

    write(&p.raw_game_data.join("ladyluck_store.source.json"), "[]\n");
    assert_eq!(
        lady_luck(&p).0,
        ["run", "generate:ladyluck-store", "--", "--apply"],
        "a supplied response wins"
    );
}

// ------------------------------------------------------------------ game data

#[test]
fn tells_real_from_example_data_and_never_overwrites_a_file() {
    let dir = TempDir::new().unwrap();
    let d = dir.path();
    assert_eq!(
        game_data_state(d, &BACKEND_GAME_DATA).unwrap().state,
        GameDataKind::Missing
    );
    for f in BACKEND_GAME_DATA {
        write(
            &d.join(f.replace(".json", ".example.json")),
            format!("{{\"example\":\"{f}\"}}"),
        );
    }
    assert_eq!(
        use_examples(d, &BACKEND_GAME_DATA).unwrap().len(),
        BACKEND_GAME_DATA.len()
    );
    let synthetic = game_data_state(d, &BACKEND_GAME_DATA).unwrap();
    assert_eq!(synthetic.state, GameDataKind::Synthetic);
    assert_eq!(synthetic.synthetic.len(), BACKEND_GAME_DATA.len());
    for f in BACKEND_GAME_DATA {
        write(&d.join(f), format!("{{\"real\":\"{f}\"}}"));
    }
    assert!(use_examples(d, &BACKEND_GAME_DATA).unwrap().is_empty());
    assert_eq!(
        game_data_state(d, &BACKEND_GAME_DATA).unwrap().state,
        GameDataKind::Real
    );
}

#[test]
fn skips_the_steps_whose_export_or_supplied_source_is_not_there() {
    let dir = TempDir::new().unwrap();
    let p = host(&dir);
    let steps = game_data_steps(&p, false);
    assert_eq!(steps.len(), 7);
    assert!(
        steps.iter().all(|s| s.skip().is_some()),
        "nothing exported yet: every step is skipped with a reason"
    );
    assert_eq!(
        steps[0].title, "Progression config",
        "the progression config goes first: Ramsgate needs it"
    );
    write(&p.exports.join("hunts_1_14_7").join("player_hunts.jsonl"), "{}\n");
    let hunts = steps.iter().find(|s| s.title == "Director hunt tables").unwrap();
    assert_eq!(hunts.skip(), None, "the hunt tables run once the hunts are exported");
}

fn failing_progression_host(dir: &TempDir) -> Paths {
    let p = host(dir);
    write(
        &p.backend_dir.join("package.json"),
        json!({ "name": "b", "private": true, "scripts": { "generate:progression-config": "node fail.js", "reward-cache:generate": "node ok.js" } }).to_string(),
    );
    write(
        &p.backend_dir.join("fail.js"),
        "throw new Error('Catalog item not found: x');\n",
    );
    write(
        &p.backend_dir.join("ok.js"),
        "require('fs').writeFileSync('ran.txt', 'yes');\n",
    );
    write(&p.raw_game_data.join(SUPPLIED_SOURCES[0].1), "{}\n");
    write(&p.exports.join("catalog_1_14_7.jsonl"), "{}\n");
    p
}

#[test]
fn runs_the_other_steps_when_one_fails_and_names_the_failure() {
    let dir = TempDir::new().unwrap();
    let p = failing_progression_host(&dir);
    let ran = RefCell::new(Vec::new());
    let run = |_: &Paths, step: &host::gamedata::Step<'_>, _: &Path| {
        ran.borrow_mut().push(step.title);
        let failing = step.title == "Progression config";
        Ok(RunResult {
            code: i32::from(failing),
            last_line: failing.then(|| "Error: Catalog item not found: x".to_owned()),
            output: String::new(),
        })
    };
    let fetch = |_: &Paths, _: &CapturePin, _: &Path| Ok(CaptureFetch::AlreadyReady);
    let hooks = Generators {
        fetch_captures: &fetch,
        run_step: &run,
    };
    let opts = GenerateOptions {
        examples_for_missing: false,
        log_file: p.logs.join("gamedata.log"),
        only: None,
    };
    let mut lines = Vec::new();
    let r = generate_game_data_with(&p, &opts, &mut |l| lines.push(l.to_owned()), &hooks).unwrap();
    assert_eq!((r.ok, r.failed), (false, vec!["Progression config".to_owned()]));
    assert_eq!(
        ran.into_inner(),
        ["Progression config", "Reward Cache store"],
        "the Reward Cache step still ran"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("failed: Error: Catalog item not found: x")),
        "{}",
        lines.join("\n")
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("Failed: Progression config. The other steps ran"))
    );
    assert!(
        lines.iter().any(|l| l.starts_with("- Store captures: not fetched, ")),
        "no pin in this checkout"
    );
}

/// The same with the real generators through npm: needs RH2's `run::run`.
#[test]
#[ignore = "needs RH2's run::run (crate::run is a stub until wave 2's integration pass)"]
fn runs_the_node_generators_and_names_the_error_not_nodes_version_line() {
    let dir = TempDir::new().unwrap();
    let p = failing_progression_host(&dir);
    let opts = GenerateOptions {
        examples_for_missing: false,
        log_file: p.logs.join("gamedata.log"),
        only: None,
    };
    let mut lines = Vec::new();
    let r = generate_game_data(&p, &opts, &mut |l| lines.push(l.to_owned())).unwrap();
    assert_eq!((r.ok, r.failed), (false, vec!["Progression config".to_owned()]));
    assert!(
        p.backend_dir.join("ran.txt").exists(),
        "the Reward Cache step still ran"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("failed: Error: Catalog item not found: x")),
        "{}",
        lines.join("\n")
    );
}

// ------------------------------------------------------------------ progression

/// Rows as CatalogExporter's ProgressionExporter writes them: the Node test's export, from the fixtures.
fn export(name: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    copy_tree(
        &fixtures().join("progression").join(name).join(PROGRESSION_EXPORT_DIR),
        &dir.path().join(PROGRESSION_EXPORT_DIR),
    );
    let root = dir.path().to_path_buf();
    (dir, root)
}

#[test]
fn converts_the_exported_tracks_to_the_live_services_shape_and_adds_the_active_hunt_pass() {
    let (_dir, root) = export("small");
    let built = build_progression_config(&root, ACTIVE_HUNT_PASS).unwrap();
    let paths = built.config["payload"]["paths"].as_array().unwrap().clone();
    let ids: Vec<&str> = paths.iter().map(|p| p["progression_id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        [
            "ExperienceTrack_PlayerLevel",
            "ExperienceTrack_Weapon_Sword",
            "ExperienceTrack_Weapon_Axe",
            "MasteryTrack_PlayerLevel",
            "PrestigeTrack_Weapon_Sword",
            "PrestigeTrack_Weapon_Axe",
            ACTIVE_HUNT_PASS
        ]
    );
    assert_eq!(
        built.skipped,
        ["ExperienceTrack_Weapon_None"],
        "a track without ranks is left out"
    );
    assert_eq!(
        built.derived,
        ["PrestigeTrack_Weapon_Sword", "PrestigeTrack_Weapon_Axe"],
        "each weapon's prestige track, named by its row or not"
    );
    assert_eq!(
        paths[5]["requirements"],
        json!([{ "rank_id": 0, "xp_required": 0 }, { "rank_id": 1, "xp_required": 70 }])
    );
    assert_eq!(
        paths[4]["requirements"],
        json!([{ "rank_id": 0, "xp_required": 0 }, { "rank_id": 1, "xp_required": 50 }]),
        "with the sword's ranks"
    );
    assert_eq!(
        (&paths[4]["free_rewards"], &paths[4]["premium_rewards"]),
        (&json!([]), &json!([]))
    );

    let level = &paths[0];
    assert_eq!(
        level["requirements"],
        json!([{ "rank_id": 0, "xp_required": 0 }, { "rank_id": 1, "xp_required": 100 }, { "rank_id": 2, "xp_required": 250 }])
    );
    assert_eq!(level["start_date"], "2019-01-01T14:00:00+00:00");
    assert_eq!(level["end_date"], "2099-01-01T14:00:00+00:00");
    assert_eq!(
        level["free_rewards"][0],
        json!({
            "rank_id": 1,
            "custom_reward_string": { "culture_invariant_string": null },
            "stacked_items": [{ "catalog_id": "CURRENCY_RAMS", "hidden": false, "priority": 0, "quantity": 500 }],
            "instanced_items": ["WP_SWORD_00"],
            "ordered_instanced_items": [],
            "entitlements": [{ "duration": 0, "entitlement": "ent_title_00" }],
            "buffs": [],
        })
    );
    let pass = &paths[6];
    assert_eq!(pass["premium_gating_entitlement"], "season43_premium");
    assert_eq!(pass["requirements"].as_array().unwrap().len(), 51);
    assert_eq!(
        pass["requirements"].as_array().unwrap()[..2],
        [
            json!({ "rank_id": 0, "xp_required": 0 }),
            json!({ "rank_id": 1, "xp_required": 100 })
        ]
    );
    assert_eq!(pass["free_rewards"].as_array().unwrap().len(), 51);
    assert_eq!(built.config["code"], Value::Null);
    assert_eq!(built.config["message"], "OK");
}

#[test]
fn refuses_an_export_without_the_player_level_track() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let rows = |file: &str, text: &str| write(&root.join(PROGRESSION_EXPORT_DIR).join(file), text);
    rows(
        "weapon-experience-tracks.jsonl",
        r#"{"rowName":"Axe","base":{"progressionTrack":"ExperienceTrack_Weapon_Axe","requirements":[{"rankId":0,"requiredXP":0}]}}"#,
    );
    let error = build_progression_config(root, ACTIVE_HUNT_PASS)
        .unwrap_err()
        .to_string();
    assert!(error.contains("ExperienceTrack_PlayerLevel"), "{error}");
    let none = TempDir::new().unwrap();
    let error = build_progression_config(none.path(), ACTIVE_HUNT_PASS)
        .unwrap_err()
        .to_string();
    assert!(error.contains("EXPORT_PROGRESSION=1"), "{error}");
}

#[test]
fn writes_the_source_from_the_export_replaces_its_own_next_time_and_never_a_supplied_response() {
    let (_dir, exports) = export("small");
    let raw = TempDir::new().unwrap();
    let source = raw.path().join("progression_config.source.json");
    let none = TempDir::new().unwrap();
    assert_eq!(
        write_progression_source(none.path(), &source).unwrap(),
        SourceWritten::NoExport
    );
    let written = SourceWritten::Written { written: 7, derived: 2 };
    assert_eq!(write_progression_source(&exports, &source).unwrap(), written);
    let text: Value = serde_json::from_str(&read(&source)).unwrap();
    assert_eq!(text["generated_by"], GENERATED_BY);
    assert_eq!(
        write_progression_source(&exports, &source).unwrap(),
        written,
        "a source it wrote is rebuilt"
    );
    write(&source, r#"{"code":null,"message":"OK","payload":{"paths":[]}}"#);
    assert_eq!(
        write_progression_source(&exports, &source).unwrap(),
        SourceWritten::Supplied
    );
    let kept: Value = serde_json::from_str(&read(&source)).unwrap();
    assert_eq!(kept["payload"]["paths"], json!([]), "a captured response is kept");
    write(&source, "not json");
    assert_eq!(
        write_progression_source(&exports, &source).unwrap(),
        SourceWritten::Supplied
    );
}

#[test]
fn writes_the_progression_source_byte_for_byte_as_the_node_kit() {
    for name in ["small", "odd"] {
        let (_dir, exports) = export(name);
        let source = exports.join("progression_config.source.json");
        assert!(matches!(
            write_progression_source(&exports, &source).unwrap(),
            SourceWritten::Written { .. }
        ));
        let expected = fixtures().join("progression").join(name);
        assert_eq!(
            std::fs::read(&source).unwrap(),
            std::fs::read(expected.join("progression_config.source.json")).unwrap(),
            "{name}"
        );
        let built = build_progression_config(&exports, ACTIVE_HUNT_PASS).unwrap();
        assert_eq!(read(&source), progression_text(&built));
        let node: Value = serde_json::from_str(&read(&expected.join("build.json"))).unwrap();
        assert_eq!(
            json!({ "tracks": built.tracks, "skipped": built.skipped, "derived": built.derived }),
            node,
            "{name}"
        );
    }
}

// ------------------------------------------------------------------ packs

fn with_examples(dir: &TempDir) -> Paths {
    let p = host(dir);
    // The examples upstream ships beside the real files.
    write(&p.backend_game_data.join("ladyluck_store.example.json"), "[]\n");
    write(
        &p.backend_game_data.join("progression_config.example.json"),
        "{\"example\":true}\n",
    );
    p
}

#[test]
fn packs_carry_the_generated_files_to_another_setup_leaving_out_the_examples() {
    let from_dir = TempDir::new().unwrap();
    let from = with_examples(&from_dir);
    write(
        &from.backend_game_data.join("progression_config.json"),
        "{\"paths\":1}\n",
    );
    write(&from.backend_game_data.join("ladyluck_store.json"), "[]\n"); // still the example
    write(
        &from.director_game_data.join("player_hunts_table.json"),
        "{\"hunts\":2}\n",
    );
    write(
        &from.raw_game_data.join("progression_config.source.json"),
        "{\"source\":3}\n",
    );
    write(&from.backend_game_data.join("unrelated.json"), "{}\n");
    let files = TempDir::new().unwrap();
    let pack = files.path().join("p.json.gz");
    let r = export_game_data_pack(&from, &pack, Utc::now()).unwrap();
    let mut included = r.included.clone();
    included.sort();
    assert_eq!(
        included,
        [
            "backend/progression_config.json",
            "backend/raw/progression_config.source.json",
            "director/player_hunts_table.json"
        ]
    );
    assert!(
        r.left.contains(&"backend/ladyluck_store.json".to_owned()),
        "the example is not packed"
    );

    let to_dir = TempDir::new().unwrap();
    let to = with_examples(&to_dir);
    write(
        &to.backend_game_data.join("progression_config.json"),
        "{\"old\":true}\n",
    );
    let imported = import_game_data_pack(&to, &pack, Utc::now()).unwrap();
    assert_eq!(imported.written.len(), 3);
    assert_eq!(
        read(&to.backend_game_data.join("progression_config.json")),
        "{\"paths\":1}\n"
    );
    assert_eq!(
        read(&to.director_game_data.join("player_hunts_table.json")),
        "{\"hunts\":2}\n"
    );
    assert_eq!(
        read(&to.raw_game_data.join("progression_config.source.json")),
        "{\"source\":3}\n"
    );
    assert_eq!(imported.backups.len(), 1, "the replaced file is kept");
    assert_eq!(read(&imported.backups[0]), "{\"old\":true}\n");
    assert_eq!(
        import_game_data_pack(&to, &pack, Utc::now()).unwrap().unchanged.len(),
        3,
        "importing again changes nothing"
    );
}

fn gzip(text: &str) -> Vec<u8> {
    use std::io::Write;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(text.as_bytes()).unwrap();
    gz.finish().unwrap()
}

#[test]
fn packs_with_an_unknown_path_a_bad_hash_another_game_build_or_no_pack_at_all_are_refused() {
    let dir = TempDir::new().unwrap();
    let p = with_examples(&dir);
    let files = TempDir::new().unwrap();
    let n = std::cell::Cell::new(0);
    let make = |list: Value, game: &str| {
        n.set(n.get() + 1);
        let f = files.path().join(format!("{}.json.gz", n.get()));
        let pack = json!({ "format": PACK_FORMAT, "version": 1, "game": game, "created": "", "files": list });
        std::fs::write(&f, gzip(&pack.to_string())).unwrap();
        f
    };
    let hash = "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"; // sha256("{}")
    let game = "1.14.7 (647472)";
    let good = make(
        json!([{ "path": "backend/progression_config.json", "sha256": hash, "data": "{}" }]),
        game,
    );
    assert_eq!(read_game_data_pack(&p, &good).unwrap().files.len(), 1);
    let error = |f: PathBuf| read_game_data_pack(&p, &f).unwrap_err().to_string();
    assert!(
        error(make(
            json!([{ "path": "../../evil.json", "sha256": hash, "data": "{}" }]),
            game
        ))
        .contains("unknown file")
    );
    assert!(
        error(make(
            json!([{ "path": "backend/progression_config.json", "sha256": "00", "data": "{}" }]),
            game
        ))
        .contains("damaged")
    );
    assert!(
        error(make(
            json!([{ "path": "backend/progression_config.json", "sha256": sha256_hex("x"), "data": "x" }]),
            game
        ))
        .contains("is not JSON")
    );
    assert!(error(make(json!([]), "1.12.0 (392819)")).contains("1.12.0"));
    let not_pack = files.path().join("x.json.gz");
    std::fs::write(&not_pack, "hello").unwrap();
    assert!(error(not_pack).contains("not a game data pack"));
    let empty = TempDir::new().unwrap();
    let error = export_game_data_pack(&with_examples(&empty), &files.path().join("e.json.gz"), Utc::now())
        .unwrap_err()
        .to_string();
    assert!(error.contains("reforged-host gamedata first"), "{error}");
}

/// A host's game data laid out as the fixtures hold it (`pack/gamedata/{backend,director}`).
fn fixture_host(dir: &TempDir) -> Paths {
    let mut p = host(dir);
    let data = fixtures().join("pack").join("gamedata");
    copy_tree(&data.join("backend"), &dir.path().join("gd").join("backend"));
    copy_tree(&data.join("director"), &dir.path().join("gd").join("director"));
    p.backend_game_data = dir.path().join("gd").join("backend");
    p.director_game_data = dir.path().join("gd").join("director");
    p.raw_game_data = p.backend_game_data.join("raw");
    p
}

#[test]
fn writes_and_reads_packs_as_the_node_kit() {
    let dir = TempDir::new().unwrap();
    let p = fixture_host(&dir);
    let out = dir.path().join("pack.json.gz");
    let r = export_game_data_pack(&p, &out, date("2026-10-15T14:52:05.055Z")).unwrap();
    let node: Value = serde_json::from_str(&read(&fixtures().join("pack").join("result.json"))).unwrap();
    assert_eq!(json!({ "included": r.included, "left": r.left }), node);
    let mut text = String::new();
    flate2::read::GzDecoder::new(std::fs::File::open(&out).unwrap())
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(
        text,
        read(&fixtures().join("pack").join("pack.json")),
        "the pack's JSON, byte for byte"
    );

    // The Node kit's own pack goes back in.
    let to_dir = TempDir::new().unwrap();
    let to = host(&to_dir);
    let pack = read_game_data_pack(&to, &fixtures().join("pack").join("pack.json.gz")).unwrap();
    assert_eq!(pack_json(&pack), text);
    let imported = import_game_data_pack(&to, &fixtures().join("pack").join("pack.json.gz"), Utc::now()).unwrap();
    assert_eq!(imported.written, r.included);
    for name in &r.included {
        let (side, file) = name.split_once('/').unwrap();
        let (from, into) = match side {
            "backend" => (&p.backend_game_data, &to.backend_game_data),
            _ => (&p.director_game_data, &to.director_game_data),
        };
        let file: PathBuf = file.split('/').collect();
        assert_eq!(
            std::fs::read(from.join(&file)).unwrap(),
            std::fs::read(into.join(&file)).unwrap(),
            "{name}"
        );
    }
}

// ------------------------------------------------------------------ preset

fn preset_host(dir: &TempDir) -> Paths {
    let p = host(dir);
    write(&p.backend_game_data.join("ladyluck_store.example.json"), "[]\n");
    write(
        &p.backend_game_data.join("progression_config.example.json"),
        "{\"example\":true}\n",
    );
    write(&p.backend_game_data.join("reward_cache_store.example.json"), "[]\n");
    p
}

#[test]
fn the_preset_is_extracted_with_its_provenance_and_fills_a_new_setup() {
    let from_dir = TempDir::new().unwrap();
    let from = preset_host(&from_dir);
    write(
        &from.backend_game_data.join("progression_config.json"),
        "{\"paths\":1}\n",
    );
    write(&from.backend_game_data.join("reward_cache_store.json"), "[]\n"); // the example: not in the preset
    write(&from.backend_game_data.join("ladyluck_store.json"), "[{\"live\":1}]\n"); // from the store captures: never in one
    write(&from.backend_game_data.join("platinum_store.json"), "[{\"live\":2}]\n");
    write(
        &from.director_game_data.join("player_hunts_table.json"),
        "{\"hunts\":2}\n",
    );
    let game = TempDir::new().unwrap();
    let exe = game.path().join("Dauntless-Win64-Shipping.exe");
    write(&exe, "not really a game");
    write(&from.preset.join("backend").join("stale.json"), "{}\n"); // left from an older preset
    write(&from.preset.join("README.md"), "# preset\n");

    let opts = CreateOptions {
        name: "Friday Hunts".into(),
        executable: Some(exe),
        executable_sha256: None,
    };
    let r = create_preset(&from, &opts, date("2026-10-15T00:00:00Z")).unwrap();
    let paths: Vec<&str> = r.manifest.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        ["backend/progression_config.json", "director/player_hunts_table.json"]
    );
    assert!(r.left.contains(&"backend/reward_cache_store.json".to_owned()));
    assert_eq!(
        r.excluded,
        ["backend/ladyluck_store.json", "backend/platinum_store.json"]
    );
    assert_eq!(
        r.manifest.executable_sha256.as_deref(),
        Some("d787d550340c5880ee32ca9e3fc7ffe14d25bae533e0c3f37e88b9026dbc1370"),
        "the executable it was read from"
    );
    assert!(r.manifest.comment.join(" ").contains("interoperability"));
    assert!(
        !from.preset.join("backend").join("stale.json").exists(),
        "the folder is exactly the manifest"
    );
    assert!(from.preset.join("README.md").exists(), "its README stays");
    assert_eq!(read_preset_manifest(&from).unwrap().unwrap().name, "Friday Hunts");

    // Another checkout of the project with the same preset/ (a new setup): the examples are replaced,
    // a host's own data is kept unless --force.
    let to_dir = TempDir::new().unwrap();
    let to = preset_host(&to_dir);
    copy_tree(&from.preset, &to.preset);
    write(
        &to.backend_game_data.join("progression_config.json"),
        "{\"example\":true}\n",
    ); // still the example
    write(
        &to.director_game_data.join("player_hunts_table.json"),
        "{\"mine\":true}\n",
    ); // the host's own
    let applied = apply_preset(&to, false).unwrap().unwrap();
    assert_eq!(applied.written, ["backend/progression_config.json"]);
    assert_eq!(applied.kept, ["director/player_hunts_table.json"]);
    assert_eq!(
        read(&to.backend_game_data.join("progression_config.json")),
        "{\"paths\":1}\n"
    );
    assert_eq!(
        read(&to.director_game_data.join("player_hunts_table.json")),
        "{\"mine\":true}\n"
    );

    let forced = apply_preset_at(&to, true, date("2026-10-16T01:02:03.004Z"))
        .unwrap()
        .unwrap();
    assert_eq!(forced.written, ["director/player_hunts_table.json"]);
    assert_eq!(forced.backups.len(), 1);
    assert!(forced.backups[0].ends_with("player_hunts_table.json.2026-10-16T01-02-03-004Z.bak"));
    assert_eq!(read(Path::new(&forced.backups[0])), "{\"mine\":true}\n");
    assert_eq!(apply_preset(&to, false).unwrap().unwrap().unchanged.len(), 2);
}

#[test]
fn the_preset_is_not_applied_when_a_file_was_edited_by_hand_and_is_absent_when_there_is_none() {
    let dir = TempDir::new().unwrap();
    let p = preset_host(&dir);
    assert_eq!(apply_preset(&p, false).unwrap(), None);
    write(&p.backend_game_data.join("progression_config.json"), "{\"paths\":1}\n");
    let opts = |hash: Option<String>| CreateOptions {
        name: "x".into(),
        executable: None,
        executable_sha256: hash,
    };
    create_preset(&p, &opts(None), Utc::now()).unwrap();
    let file = p.preset.join("backend").join("progression_config.json");
    write(&file, "{\"paths\":2}\n");
    assert!(
        apply_preset(&p, false)
            .unwrap_err()
            .to_string()
            .contains("does not match its hash")
    );
    // A Git for Windows checkout (CRLF line endings) is the same file.
    write(&file, "{\"paths\":1}\r\n");
    assert_eq!(
        apply_preset(&p, false).unwrap().unwrap().unchanged,
        ["backend/progression_config.json"]
    );
    let empty = TempDir::new().unwrap();
    let error = create_preset(&preset_host(&empty), &opts(None), Utc::now())
        .unwrap_err()
        .to_string();
    assert!(error.contains("reforged-host gamedata first"), "{error}");

    // Made on another machine than the game's: the executable's hash is given.
    let elsewhere_dir = TempDir::new().unwrap();
    let elsewhere = preset_host(&elsewhere_dir);
    write(
        &elsewhere.backend_game_data.join("progression_config.json"),
        "{\"paths\":1}\n",
    );
    let made = create_preset(&elsewhere, &opts(Some("ab".repeat(32))), Utc::now()).unwrap();
    assert_eq!(made.manifest.executable_sha256, Some("ab".repeat(32)));
    // A store built from the captures is refused in a preset made by hand.
    let manifest_file = elsewhere.preset.join("preset.json");
    let mut m: Value = serde_json::from_str(&read(&manifest_file)).unwrap();
    write(&elsewhere.preset.join("backend").join("platinum_store.json"), "[]\n");
    m["files"].as_array_mut().unwrap().push(json!({
        "path": "backend/platinum_store.json",
        "sha256": "37517e5f3dc66819f61f5a7bb8ace1921282415f10551d2defa5c3eb0985b570",
        "bytes": 3
    }));
    write(&manifest_file, m.to_string());
    let error = apply_preset(&elsewhere, false).unwrap_err().to_string();
    assert!(error.contains("builds from the store captures"), "{error}");
}

#[test]
fn writes_the_preset_byte_for_byte_as_the_node_kit() {
    for (case, name, hash) in [
        (
            "with-exe",
            "Friday \"Hunts\" \u{2014} P\u{fa}blico",
            Some("ab".repeat(32)),
        ),
        ("no-exe", "x", None),
    ] {
        let dir = TempDir::new().unwrap();
        let mut p = fixture_host(&dir);
        p.preset = dir.path().join("preset");
        let opts = CreateOptions {
            name: name.into(),
            executable: None,
            executable_sha256: hash,
        };
        create_preset(&p, &opts, date("2026-10-15T14:52:05.055Z")).unwrap();
        let expected = fixtures().join("preset").join(case);
        let mut theirs = Vec::new();
        let mut ours = Vec::new();
        collect(&expected, &expected, &mut theirs);
        collect(&p.preset, &p.preset, &mut ours);
        assert_eq!(ours, theirs, "{case}");
    }
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(root, &path, out);
        } else {
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            out.push((rel, std::fs::read(&path).unwrap()));
        }
    }
}

/// The project's own preset/ is the Node kit's output: made again from its files, it is the same.
#[test]
fn makes_the_projects_preset_again_byte_for_byte() {
    let preset = repo().join("preset");
    let committed = read(&preset.join("preset.json"));
    let manifest: Value = serde_json::from_str(&committed).unwrap();
    let dir = TempDir::new().unwrap();
    let mut p = host(&dir);
    p.preset = dir.path().join("made");
    // The committed files are LF (.gitattributes); a CRLF checkout is read back to LF, as apply does.
    let lf = |from: &Path, to: &Path| {
        for entry in std::fs::read_dir(from).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                write(&to.join(path.file_name().unwrap()), read(&path).replace("\r\n", "\n"));
            }
        }
    };
    lf(&preset.join("backend"), &p.backend_game_data);
    lf(&preset.join("backend").join("raw"), &p.raw_game_data);
    lf(&preset.join("director"), &p.director_game_data);
    let opts = CreateOptions {
        name: manifest["name"].as_str().unwrap().into(),
        executable: None,
        executable_sha256: manifest["executableSha256"].as_str().map(str::to_owned),
    };
    create_preset(&p, &opts, date(manifest["created"].as_str().unwrap())).unwrap();
    assert_eq!(read(&p.preset.join("preset.json")), committed.replace("\r\n", "\n"));

    // And it applies to a new setup.
    let to_dir = TempDir::new().unwrap();
    let mut to = host(&to_dir);
    to.preset = preset;
    let applied = apply_preset(&to, false).unwrap().unwrap();
    assert_eq!(applied.written.len(), manifest["files"].as_array().unwrap().len());
}

// ------------------------------------------------------------------ runtime

#[test]
fn publishing_writes_the_files_and_a_runtime_json_then_installs_them_for_the_game_servers() {
    let dir = TempDir::new().unwrap();
    let p = host(&dir);
    write(&p.runtime_dll, vec![3u8; 6000]);
    write(&p.loader_dll, vec![4u8; 2000]);
    let r = publish_runtime(&p, date("2026-10-15T00:00:00Z"), &default_sources(&p)).unwrap();
    assert!(
        r.version.starts_with("20261015-") && r.version.len() == 17,
        "{}",
        r.version
    );
    let json = std::fs::read(p.runtime_publish.join("runtime.json")).unwrap();
    assert_eq!(
        json,
        std::fs::read(fixtures().join("runtime").join("published").join("runtime.json")).unwrap(),
        "the version and runtime.json byte for byte"
    );
    let index: Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(index["files"][1]["name"], "winmm.dll");
    assert_eq!(index["files"][1]["sha256"], sha256_file(&p.loader_dll).unwrap());
    let game = TempDir::new().unwrap();
    let win64 = game.path();
    assert_eq!(
        install_server_runtime(&p, win64).installed,
        ["MysticParadox.dll", "winmm.dll"]
    );
    assert_eq!(
        install_server_runtime(&p, win64).unchanged,
        ["MysticParadox.dll", "winmm.dll"]
    );
    assert_eq!(
        ensure_server_flags(win64).unwrap(),
        ["CALL_HUNTPASS_QUERY.flag"],
        "the hub's progression query is switched on"
    );
    assert!(
        ensure_server_flags(win64).unwrap().is_empty(),
        "and left alone once it is there"
    );
    assert!(SERVER_RUNTIME_FLAGS.iter().all(|f| win64.join(f).exists()));
}

#[test]
fn clears_old_runtime_files_a_previous_install_had_to_move_aside() {
    let dir = TempDir::new().unwrap();
    let p = host(&dir);
    write(&p.runtime_dll, vec![5u8; 6000]);
    write(&p.loader_dll, vec![6u8; 2000]);
    publish_runtime(&p, Utc::now(), &default_sources(&p)).unwrap();
    let game = TempDir::new().unwrap();
    let win64 = game.path();
    write(&win64.join("MysticParadox.dll.rr-old-123"), "old");
    let r = install_server_runtime(&p, win64);
    assert!(r.in_use.is_empty());
    assert!(!win64.join("MysticParadox.dll.rr-old-123").exists());
    assert!(!win64.join("MysticParadox.dll.tmp").exists());
}

#[test]
fn refuses_to_publish_before_anything_is_built() {
    let dir = TempDir::new().unwrap();
    let p = host(&dir);
    let error = publish_runtime(&p, Utc::now(), &default_sources(&p))
        .unwrap_err()
        .to_string();
    assert!(error.contains("not built"), "{error}");
}
