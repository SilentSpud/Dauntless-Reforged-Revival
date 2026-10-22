//! The files generated from the bridge (rows L6, L8, L9), kept in the repository and checked here:
//!
//! - `src/shared/generated/types.ts`: the TypeScript declarations of every type in `types.rs` and the
//!   command names, for the page;
//! - `isolation/commands.js`: the command list with each argument's shape, for the isolation hook;
//! - `src-tauri/permissions/app-commands.toml`: the permission set that grants exactly those commands.
//!
//! The test fails when a file is stale; `DR_UPDATE_GENERATED=1 cargo test -p launcher generated`
//! rewrites them.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use protocol::invite::ServerMode;
use protocol::status::{InstanceKind, PlayerWhere, RegistrationMode, ServerStatus, StatusInstance, StatusPlayer};
use ts_rs::{Config, TS};

use super::spec::{COMMANDS, SETTINGS_KEYS};
use super::types::*;

const HEADER: &str = "Generated from the launcher backend (src-tauri/src/bridge); do not edit.\n\
Regenerate with: DR_UPDATE_GENERATED=1 cargo test -p launcher generated";

fn declarations() -> String {
    let config = Config::new().with_large_int("number");
    let mut out = String::new();
    for line in HEADER.lines() {
        let _ = writeln!(out, "// {line}");
    }
    out.push('\n');
    let mut declare = |text: String| {
        out.push_str("export ");
        out.push_str(&text);
        out.push('\n');
    };
    macro_rules! each {
        ($($ty:ty),* $(,)?) => { $( declare(<$ty as TS>::decl(&config)); )* };
    }
    each!(
        ServerMode,
        PlayerWhere,
        InstanceKind,
        RegistrationMode,
        StatusPlayer,
        StatusInstance,
        ServerStatus,
        Language,
        ExposureMode,
        GameLanguage,
        HuntRegion,
        Phase,
        ConnectProblem,
        ErrorCode,
        NoticeCode,
        LauncherError,
        OfflinePhase,
        OfflineStatus,
        TaskKind,
        TaskProgress,
        ServerInfo,
        SavedServerInfo,
        Settings,
        SnapshotConnect,
        SnapshotAccount,
        SnapshotInstall,
        SnapshotGame,
        SnapshotApp,
        Snapshot,
        NewsItem,
        Background,
        Branding,
        ExternalTarget,
    );
    // The types serde writes by hand (types.rs)
    declare("type GraphicsPreset = -1 | 0 | 1 | 2 | 3 | 4;".into());
    declare("type ActionResult = { ok: true } | { ok: false; error: LauncherError };".into());
    declare(
        "type InviteCheck = { ok: true; certificateChanged: boolean; previousFingerprint: string | null } | { ok: false; error: LauncherError };"
            .into(),
    );
    declare("type RegisterOutcome = { ok: true; username: string } | { ok: false; error: LauncherError };".into());
    let names: Vec<String> = COMMANDS.iter().map(|command| format!("\"{}\"", command.name)).collect();
    declare(format!("type CommandName = {};", names.join(" | ")));
    out
}

fn hook_commands() -> String {
    let mut out = String::new();
    for line in HEADER.lines() {
        let _ = writeln!(out, "// {line}");
    }
    // Written by hand, in the list's order: a serde_json map's order depends on whether another crate in
    // the build turns on its `preserve_order` feature
    let quote = |text: &str| serde_json::to_string(text).unwrap_or_default();
    out.push_str("window.__DR_COMMANDS__ = {\n");
    for command in COMMANDS {
        let args: Vec<String> = command
            .args
            .iter()
            .map(|(name, arg)| format!("{}: {}", quote(name), serde_json::to_string(arg).unwrap_or_default()))
            .collect();
        let _ = writeln!(out, "  {}: {{ {} }},", quote(command.name), args.join(", "));
    }
    out.push_str("};\n");
    let keys: Vec<String> = SETTINGS_KEYS.iter().map(|key| quote(key)).collect();
    let _ = writeln!(out, "window.__DR_SETTINGS_KEYS__ = [{}];", keys.join(", "));
    out
}

fn permission_set() -> String {
    let mut out = String::new();
    for line in HEADER.lines() {
        let _ = writeln!(out, "# {line}");
    }
    out.push_str("\n[[set]]\nidentifier = \"app-commands\"\ndescription = \"Every command in src/bridge/spec.rs, and nothing else.\"\npermissions = [\n");
    for command in COMMANDS {
        let _ = writeln!(out, "  \"allow-{}\",", command.name.replace('_', "-"));
    }
    out.push_str("]\n");
    out
}

fn launcher_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

#[test]
fn generated_files_are_current() {
    let files = [
        (launcher_dir().join("src/shared/generated/types.ts"), declarations()),
        (launcher_dir().join("isolation/commands.js"), hook_commands()),
        (
            launcher_dir().join("src-tauri/permissions/app-commands.toml"),
            permission_set(),
        ),
    ];
    let update = std::env::var("DR_UPDATE_GENERATED").is_ok_and(|value| value == "1");
    let mut stale = Vec::new();
    for (path, content) in files {
        let current = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
        if current == content {
            continue;
        }
        if update {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).unwrap();
            }
            std::fs::write(&path, content).unwrap();
        } else {
            stale.push(path.display().to_string());
        }
    }
    assert!(
        stale.is_empty(),
        "stale generated files (run DR_UPDATE_GENERATED=1 cargo test -p launcher generated): {stale:?}"
    );
}
