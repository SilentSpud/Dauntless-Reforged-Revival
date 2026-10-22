//! The files generated from the bridge (rows L6, L8, L9; RA2), kept in the repository and checked here:
//!
//! - `page/generated/types.ts`: the TypeScript declarations of every type in `types.rs` and the command
//!   names, for the page's bridge (`page/api.ts`);
//! - `isolation/commands.js`: the command list with each argument's shape, for the isolation hook;
//! - `src-tauri/permissions/app-commands.toml`: the permission set that grants exactly those commands.
//!
//! The test fails when a file is stale; `RR_UPDATE_GENERATED=1 cargo test -p launcher generated`
//! rewrites them.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use protocol::invite::ServerMode;
use protocol::status::{InstanceKind, PlayerWhere, RegistrationMode, ServerStatus, StatusInstance, StatusPlayer};
use ts_rs::{Config, TS};

use super::spec::{Arg, COMMANDS, SETTINGS_KEYS};
use super::types::*;

const HEADER: &str = "Generated from the launcher backend (rust/launcher/src-tauri/src/bridge); do not edit.\n\
Regenerate with: RR_UPDATE_GENERATED=1 cargo test -p launcher generated";

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
        GameLanguage,
        RuntimeProfile,
        Phase,
        ConnectProblem,
        GameProblem,
        RuntimeState,
        ErrorCode,
        NoticeCode,
        LauncherError,
        TaskKind,
        TaskProgress,
        ServerInfo,
        SavedServerInfo,
        GameSource,
        DetectedGame,
        GameInfo,
        RuntimeFileInfo,
        RuntimeInfo,
        Settings,
        SnapshotConnect,
        SnapshotAccount,
        SnapshotRunning,
        SnapshotApp,
        Snapshot,
        NewsItem,
        Background,
        Branding,
        GameDataState,
        HostUpstream,
        HostSdk,
        HostDump,
        HostBuilds,
        HostToolchain,
        HostGameData,
        HostExports,
        HostServerMode,
        HostServerConfig,
        HostKit,
        HostMultipliers,
        HostUpdateState,
        HostUpdateView,
        RequirementId,
        RequirementState,
        Requirement,
        HostServerState,
        HostInvite,
        HostServerView,
        HostJobKind,
        HostJobState,
        HostJob,
        ExternalTarget,
        HostAction,
        HostFolder,
        HostSetupRequest,
    );
    // The types serde writes by hand (types.rs)
    declare("type GraphicsPreset = -1 | 0 | 1 | 2 | 3 | 4;".into());
    declare("type ActionResult = { ok: true } | { ok: false; error: LauncherError };".into());
    declare(
        "type InviteCheck = { ok: true; certificateChanged: boolean; previousFingerprint: string | null; movedFrom: string | null } | { ok: false; error: LauncherError };"
            .into(),
    );
    declare("type RegisterOutcome = { ok: true; username: string } | { ok: false; error: LauncherError };".into());
    let names: Vec<String> = COMMANDS.iter().map(|command| format!("\"{}\"", command.name)).collect();
    declare(format!("type CommandName = {};", names.join(" | ")));
    out
}

fn quote(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

/// A number as JavaScript writes it: `1`, not `1.0`.
fn number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

/// One argument's shape for the hook, written by hand so the order is the list's: a serde_json map's
/// order depends on whether another crate in the build turns on its `preserve_order` feature.
fn arg_js(arg: &Arg) -> String {
    match arg {
        Arg::Text { max } => format!("{{ kind: \"text\", max: {max} }}"),
        Arg::OptionalText { max } => format!("{{ kind: \"optionalText\", max: {max} }}"),
        Arg::Flag => "{ kind: \"flag\" }".into(),
        Arg::Index { max } => format!("{{ kind: \"index\", max: {max} }}"),
        Arg::Name { names } => {
            let names: Vec<String> = names.iter().map(|name| quote(name)).collect();
            format!("{{ kind: \"name\", names: [{}] }}", names.join(", "))
        }
        Arg::Number { min, max } => format!("{{ kind: \"number\", min: {}, max: {} }}", number(*min), number(*max)),
        Arg::Object { fields, nullable } => format!(
            "{{ kind: \"object\", nullable: {nullable}, fields: {} }}",
            args_js(fields)
        ),
        Arg::SettingsPatch => "{ kind: \"settingsPatch\" }".into(),
        Arg::Channel => "{ kind: \"channel\" }".into(),
    }
}

fn args_js(args: &[(&str, Arg)]) -> String {
    if args.is_empty() {
        return "{}".into();
    }
    let args: Vec<String> = args
        .iter()
        .map(|(name, arg)| format!("{}: {}", quote(name), arg_js(arg)))
        .collect();
    format!("{{ {} }}", args.join(", "))
}

fn hook_commands() -> String {
    let mut out = String::new();
    for line in HEADER.lines() {
        let _ = writeln!(out, "// {line}");
    }
    out.push_str("window.__RR_COMMANDS__ = {\n");
    for command in COMMANDS {
        let _ = writeln!(out, "  {}: {},", quote(command.name), args_js(command.args));
    }
    out.push_str("};\n");
    let keys: Vec<String> = SETTINGS_KEYS.iter().map(|key| quote(key)).collect();
    let _ = writeln!(out, "window.__RR_SETTINGS_KEYS__ = [{}];", keys.join(", "));
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
        (launcher_dir().join("page/generated/types.ts"), declarations()),
        (launcher_dir().join("isolation/commands.js"), hook_commands()),
        (
            launcher_dir().join("src-tauri/permissions/app-commands.toml"),
            permission_set(),
        ),
    ];
    let update = std::env::var("RR_UPDATE_GENERATED").is_ok_and(|value| value == "1");
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
        "stale generated files (run RR_UPDATE_GENERATED=1 cargo test -p launcher generated): {stale:?}"
    );
}

#[test]
fn numbers_read_as_javascript_writes_them() {
    assert_eq!(number(1.0), "1");
    assert_eq!(number(100.0), "100");
    assert_eq!(number(1.5), "1.5");
}
