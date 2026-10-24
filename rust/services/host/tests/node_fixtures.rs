//! Files the Node kit wrote, read back and written out by the Rust kit: the bytes must be the same
//! (MIGRATION_PLAN.md section 6, "host.json and the .env files it writes").
//!
//! `tests/fixtures/node/<flavour>/` comes from `tests/fixtures/node/gen.ts`, run with the Node kit's
//! own `paths.ts`, `envfile.ts` and `hostconfig.ts` (`windows`: as Node runs them on Windows;
//! `posix`: with `path.posix`, as on Linux). Each OS checks the flavour whose paths it writes.

#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

use std::path::{Path, PathBuf};

use host::envfile::{EnvDocument, parse_env};
use host::hostconfig::{
    HostConfig, Multipliers, Platform, Secrets, build_env_files, host_config_text, load_or_create_secrets,
    read_env_file, read_host_config, write_env_files, write_host_config,
};
use host::paths::Paths;

fn fixtures(flavour: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("node")
        .join(flavour)
}

/// The flavour whose paths this OS writes.
fn native() -> (PathBuf, Paths) {
    if cfg!(windows) {
        (fixtures("windows"), Paths::new("C:\\rr"))
    } else {
        (fixtures("posix"), Paths::new("/home/reforged/Reforged"))
    }
}

fn text(file: &Path) -> String {
    std::fs::read_to_string(file).unwrap_or_else(|e| panic!("{}: {e}", file.display()))
}

fn read(file: &Path) -> HostConfig {
    read_host_config(file).unwrap().unwrap()
}

const WRITTEN_BY_SETUP: [&str; 5] = [
    "private.json",
    "public.json",
    "older.json",
    "multipliers.json",
    "multipliers-off.json",
];

#[test]
fn host_json_written_by_the_node_kit_reads_back_and_writes_out_unchanged() {
    for flavour in ["windows", "posix"] {
        let dir = fixtures(flavour).join("hostjson");
        for name in WRITTEN_BY_SETUP {
            let file = dir.join(name);
            assert_eq!(host_config_text(&read(&file)).unwrap(), text(&file), "{flavour}/{name}");
            // And through the file, as the commands do.
            let out = tempfile::tempdir().unwrap();
            let copy = out.path().join("config").join("host.json");
            write_host_config(&copy, &read(&file)).unwrap();
            assert_eq!(text(&copy), text(&file), "{flavour}/{name} written to a file");
        }
    }
}

#[test]
fn a_hand_edited_host_json_is_normalized_as_the_node_kit_does_it() {
    // Another key order, a key the kit does not know, 3.0 and 1e1, CRLF and four spaces; the host
    // and the name in their cleaned forms.
    for flavour in ["windows", "posix"] {
        let dir = fixtures(flavour).join("hostjson");
        let c = read(&dir.join("older-input.json"));
        assert_eq!(c.host, "hostpc.tail1234.ts.net");
        assert_eq!(c.name, "Old Server");
        assert_eq!(
            host_config_text(&c).unwrap(),
            text(&dir.join("older.json")),
            "{flavour}"
        );
    }
}

#[test]
fn changing_one_field_moves_no_other_key() {
    for flavour in ["windows", "posix"] {
        let dir = fixtures(flavour).join("hostjson");
        // `multipliers --xp 5 --rewards 2.5`: the new key goes at the end, as `{ ...config, multipliers }`.
        let mut c = read(&dir.join("older-input.json"));
        c.multipliers = Some(Multipliers { xp: 5.0, rewards: 2.5 });
        assert_eq!(
            host_config_text(&c).unwrap(),
            text(&dir.join("multipliers.json")),
            "{flavour} on"
        );
        // `multipliers --off`
        let mut c = read(&dir.join("multipliers.json"));
        c.multipliers = None;
        assert_eq!(
            host_config_text(&c).unwrap(),
            text(&dir.join("multipliers-off.json")),
            "{flavour} off"
        );
    }
}

#[test]
fn a_new_host_json_has_setups_key_order() {
    for flavour in ["windows", "posix"] {
        let file = fixtures(flavour).join("hostjson").join("public.json");
        let mut fresh = read(&file);
        fresh.file = Default::default();
        assert_eq!(host_config_text(&fresh).unwrap(), text(&file), "{flavour}");
    }
}

/// The secrets the Rust kit adds to a Node kit's file: the admin API's two (docs/admin-api.md).
fn with_admin_api_secrets() -> Result<Secrets, host::error::HostError> {
    Ok(Secrets {
        admin_api_key: "g".repeat(64),
        operator_secret: "h".repeat(64),
        ..Secrets::default()
    })
}

/// The lines the Rust kit writes that the Node kit never did, which the comparisons leave out.
const ADMIN_API_LINES: [&str; 3] = [
    "REFORGED_ADMIN_API_KEY=",
    "MYSTICPARADOX_MANAGER_ADMIN=",
    "MYSTICPARADOX_MANAGER_OPERATOR_SECRET=",
];

fn without_admin_api_lines(text: &str) -> String {
    text.split_inclusive('\n')
        .filter(|line| !ADMIN_API_LINES.iter().any(|start| line.starts_with(start)))
        .collect()
}

#[test]
fn secrets_env_written_by_the_node_kit_is_kept_byte_for_byte() {
    let file = fixtures("windows").join("secrets").join("secrets.env");
    let copy = tempfile::tempdir().unwrap();
    std::fs::copy(&file, copy.path().join("secrets.env")).unwrap();
    // The admin API's two are added; the Node kit's own are kept as they are
    let (secrets, created) = load_or_create_secrets(copy.path(), with_admin_api_secrets).unwrap();
    assert!(created);
    assert_eq!(secrets.admin_key, "c".repeat(64));
    assert_eq!(secrets.admin_api_key, "g".repeat(64));
    let rewritten = text(&copy.path().join("secrets.env"));
    assert_eq!(without_admin_api_lines(&rewritten), text(&file));
    assert!(rewritten.ends_with(&format!("MYSTICPARADOX_MANAGER_OPERATOR_SECRET={}\n", "h".repeat(64))));
    // Made by the Rust kit from the same values: the same file.
    let out = tempfile::tempdir().unwrap();
    let (made, created) = load_or_create_secrets(out.path(), || Ok(secrets.clone())).unwrap();
    assert!(created);
    assert_eq!(made, secrets);
    assert_eq!(text(&out.path().join("secrets.env")), rewritten);
}

const CASES: [(&str, &str, Platform); 5] = [
    ("private-win32", "private.json", Platform::Windows),
    ("private-linux", "private.json", Platform::Linux),
    ("public-win32", "public.json", Platform::Windows),
    ("public-linux", "public.json", Platform::Linux),
    ("public-darwin", "public.json", Platform::Other),
];

fn secrets(dir: &Path) -> Secrets {
    let copy = tempfile::tempdir().unwrap();
    std::fs::copy(dir.join("secrets").join("secrets.env"), copy.path().join("secrets.env")).unwrap();
    load_or_create_secrets(copy.path(), with_admin_api_secrets).unwrap().0
}

#[test]
fn the_service_env_files_are_the_node_kits_bytes() {
    let (dir, p) = native();
    let s = secrets(&dir);
    for (case, json, platform) in CASES {
        let mut c = read(&dir.join("hostjson").join(json));
        match case {
            "private-win32" => c.multipliers = Some(Multipliers { xp: 5.0, rewards: 1.5 }),
            "public-darwin" => {
                c.game_dir = None;
                c.linux = None;
                c.performance = None;
                c.public_port = None;
            }
            _ => {}
        }
        let out = tempfile::tempdir().unwrap();
        let written_to = Paths {
            config: out.path().to_path_buf(),
            ..p.clone()
        };
        let files = build_env_files(&c, &s, &p, "abc123def456", platform);
        let written = write_env_files(&written_to, &files).unwrap();
        let expected_dir = dir.join("env").join(case);
        let mut expected: Vec<String> = std::fs::read_dir(&expected_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        expected.sort();
        let mut names: Vec<String> = written
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        // admin.env is the Rust kit's own: the admin API has no Node build
        names.retain(|n| n != "admin.env");
        assert_eq!(names, expected, "{case}");
        for name in &expected {
            assert_eq!(
                without_admin_api_lines(&text(&out.path().join(name))),
                text(&expected_dir.join(name)),
                "{case}/{name}"
            );
        }
    }
}

#[test]
fn every_env_file_the_node_kit_wrote_reads_back_and_writes_out_unchanged() {
    for flavour in ["windows", "posix"] {
        let root = fixtures(flavour);
        let mut files = vec![root.join("secrets").join("secrets.env")];
        for (case, _, _) in CASES {
            for entry in std::fs::read_dir(root.join("env").join(case)).unwrap() {
                files.push(entry.unwrap().path());
            }
        }
        for file in files {
            let original = text(&file);
            let doc = EnvDocument::parse(&original);
            assert_eq!(doc.to_text(), original, "{}", file.display());
            assert_eq!(doc.values(), parse_env(&original));
            assert_eq!(read_env_file(&file).unwrap(), parse_env(&original));
            // Setting a value to what it is changes nothing. Not for a value the Node kit reads with a
            // newline in it: its reader turns the `\\n` of a quoted Windows path (`...\\news.json`)
            // into one, and no value with a newline can be written.
            let mut same = doc.clone();
            for (key, value) in doc.values().iter().filter(|(_, v)| !v.contains('\n')) {
                same.set(key, value).unwrap();
            }
            assert_eq!(same.to_text(), original, "{}", file.display());
        }
    }
}
