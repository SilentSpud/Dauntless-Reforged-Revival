//! The fixed list of calls the page may make (rows L6, L7, L9; RA2): one entry per `LauncherApi` method
//! of Reforged's `preload.ts` that the backend serves, with the shape of its arguments. Three layers read
//! it: the isolation hook in the page (`isolation/commands.js`, generated from this list), the capability
//! that grants exactly these commands (`build.rs`), and the commands themselves, which check the same
//! limits again. The `revived*` methods (1.4.4, RD5) are not served; the window controls are Tauri's own.
#![cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "build.rs and the generator read the list; the commands use its limits"
    )
)]

/// One argument, as the page sends it (JSON, camelCase names). The generator writes each as the
/// hook's `{ kind, ... }` object.
#[derive(Debug, Clone, Copy)]
pub enum Arg {
    /// A string of at most `max` UTF-16 code units (JavaScript's `length`)
    Text {
        max: usize,
    },
    /// The same, or absent, or null
    OptionalText {
        max: usize,
    },
    Flag,
    /// An integer from 0 to `max`
    Index {
        max: u32,
    },
    /// One of a fixed list of names
    Name {
        names: &'static [&'static str],
    },
    /// A finite number from `min` to `max`
    Number {
        min: f64,
        max: f64,
    },
    /// An object with exactly these fields (each may be absent only if its kind allows it), or null when
    /// `nullable`
    Object {
        fields: &'static [(&'static str, Arg)],
        nullable: bool,
    },
    /// An object whose keys are settings names and whose values are strings, numbers or booleans
    SettingsPatch,
    /// A `Channel` the backend pushes values into
    Channel,
}

#[derive(Debug, Clone, Copy)]
pub struct Command {
    pub name: &'static str,
    pub args: &'static [(&'static str, Arg)],
}

const fn command(name: &'static str, args: &'static [(&'static str, Arg)]) -> Command {
    Command { name, args }
}

pub const INVITE_MAX: usize = 2048;
pub const USERNAME_MAX: usize = 32;
pub const KEY_TEXT_MAX: usize = 8192;
pub const GAME_PATH_MAX: usize = 260;
/// A saved server's id: 24 lower-case hex characters
pub const SERVER_ID_LEN: usize = 24;
/// `useDetectedGame`: an index into `game.detected`, below 16
pub const DETECTED_GAME_MAX: u32 = 15;
/// The Host page's setup form: the server's name and its public host (a DNS name or IPv4)
pub const SERVER_NAME_MAX: usize = 60;
pub const PUBLIC_HOST_MAX: usize = 253;
/// Who an invite is for
pub const INVITE_FOR_MAX: usize = 40;
/// `copyText`: an invite link, with room to spare
pub const COPY_TEXT_MAX: usize = 4096;

/// The settings `setSettings` may name.
pub const SETTINGS_KEYS: &[&str] = &[
    "graphics",
    "windowed",
    "streamerMode",
    "hostTools",
    "language",
    "gameLanguage",
    "runtimeProfile",
];

/// `ExternalTarget`'s names (a test checks they are the enum's).
pub const EXTERNAL_TARGETS: &[&str] = &[
    "tailscale_download",
    "tailscale_share",
    "vc_redist",
    "server_source",
    "project_source",
    "project_license",
    "project_issues",
    "project_docs_players",
    "project_docs_hosting",
    "project_docs_sdk",
    "installing_guide",
    "legendary",
    "dumper7",
    "visual_studio",
    "rust",
    "nodejs",
    "mongodb",
    "git",
    "visual_studio_installer",
    "rust_installer",
    "dumper7_zip",
    "revived_source",
    "mystic_source",
    "undaunted_source",
];

/// `HostAction`'s names.
pub const HOST_ACTIONS: &[&str] = &[
    "inject_dumper",
    "import_sdk",
    "build_runtime",
    "build_loader",
    "build_exporter",
    "inject_exporter",
    "copy_exports",
    "synthetic_data",
    "publish_runtime",
    "generate_gamedata",
    "export_gamedata",
];

/// `HostFolder`'s names.
pub const HOST_FOLDERS: &[&str] = &[
    "workspace",
    "runtime_source",
    "dumps",
    "game_data",
    "runtime_publish",
    "host_logs",
    "gamedata_packs",
];

/// `HostServerMode`'s names.
pub const SERVER_MODES: &[&str] = &["private", "public"];

/// The setup form: the hook checks the shape and the raw lengths; `commands::host_setup_request` trims
/// and checks the rest.
const HOST_SETUP: Arg = Arg::Object {
    fields: &[
        ("name", Arg::Text { max: SERVER_NAME_MAX }),
        ("mode", Arg::Name { names: SERVER_MODES }),
        ("publicHost", Arg::OptionalText { max: PUBLIC_HOST_MAX }),
    ],
    nullable: false,
};

const MULTIPLIER: Arg = Arg::Number {
    min: 1.0,
    max: super::types::MAX_MULTIPLIER,
};

/// The test multipliers, or null to turn them off.
const HOST_MULTIPLIERS: Arg = Arg::Object {
    fields: &[("xp", MULTIPLIER), ("rewards", MULTIPLIER)],
    nullable: true,
};

pub const COMMANDS: &[Command] = &[
    command("get_snapshot", &[]),
    command("check_invite", &[("text", Arg::Text { max: INVITE_MAX })]),
    command(
        "submit_invite",
        &[
            ("text", Arg::Text { max: INVITE_MAX }),
            ("acceptNewCertificate", Arg::Flag),
            ("carryAccount", Arg::Flag),
        ],
    ),
    command("retry_connect", &[]),
    command("forget_server", &[]),
    command("switch_server", &[("id", Arg::Text { max: SERVER_ID_LEN })]),
    command("remove_saved_server", &[("id", Arg::Text { max: SERVER_ID_LEN })]),
    command("register", &[("username", Arg::Text { max: USERNAME_MAX })]),
    command("use_existing_key", &[("text", Arg::Text { max: KEY_TEXT_MAX })]),
    command("import_key_file", &[]),
    command("save_key_backup", &[]),
    command("dismiss_backup_offer", &[]),
    command("choose_game_folder", &[]),
    command("use_game_path", &[("gamePath", Arg::Text { max: GAME_PATH_MAX })]),
    command("use_detected_game", &[("index", Arg::Index { max: DETECTED_GAME_MAX })]),
    command("verify_game", &[]),
    command("trust_runtime", &[]),
    command("install_runtime", &[]),
    command("choose_local_runtime", &[]),
    command("clear_local_runtime", &[]),
    command("cancel_task", &[]),
    command("play", &[]),
    command("set_settings", &[("patch", Arg::SettingsPatch)]),
    command("set_status_polling", &[("on", Arg::Flag)]),
    command("refresh_status", &[]),
    command("get_news", &[]),
    command("get_branding", &[]),
    command(
        "open_external",
        &[(
            "target",
            Arg::Name {
                names: EXTERNAL_TARGETS,
            },
        )],
    ),
    command("open_game_folder", &[]),
    command("open_logs_folder", &[]),
    command("logout", &[]),
    command("install_update", &[]),
    command("host_get", &[]),
    command("host_choose_workspace", &[]),
    command("host_choose_dumper", &[]),
    command("host_import_gamedata", &[]),
    command("host_import_sdk_folder", &[]),
    command("host_download_project", &[]),
    command("host_action", &[("action", Arg::Name { names: HOST_ACTIONS })]),
    command("host_open", &[("folder", Arg::Name { names: HOST_FOLDERS })]),
    command("host_server_get", &[]),
    command("host_server_start", &[]),
    command("host_server_stop", &[]),
    command("host_setup", &[("request", HOST_SETUP)]),
    command(
        "host_create_invite",
        &[("forName", Arg::OptionalText { max: INVITE_FOR_MAX })],
    ),
    command("host_update", &[]),
    command("host_set_multipliers", &[("multipliers", HOST_MULTIPLIERS)]),
    command("copy_text", &[("text", Arg::Text { max: COPY_TEXT_MAX })]),
    command("take_invite_link", &[]),
    command("dismiss_notice", &[]),
    command("dismiss_error", &[]),
    command("subscribe_progress", &[("onProgress", Arg::Channel)]),
];

/// Whether a string fits a limit counted as JavaScript counts it, so the page, the hook and the
/// backend agree on every edge.
pub fn fits(text: &str, max: usize) -> bool {
    text.len() <= max || text.encode_utf16().count() <= max
}

pub fn is_server_id(id: &str) -> bool {
    id.len() == SERVER_ID_LEN && id.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use serde::de::DeserializeOwned;
    use ts_rs::TS;

    use super::super::types::{ExternalTarget, HostAction, HostFolder, HostServerMode};
    use super::*;

    #[test]
    fn limits_count_like_javascript() {
        assert!(fits("ab", 2));
        assert!(!fits("abc", 2));
        // One code point outside the BMP is two UTF-16 units, four UTF-8 bytes
        assert!(fits("\u{1F600}", 2));
        assert!(!fits("\u{1F600}a", 2));
        assert!(fits("ää", 2));
    }

    #[test]
    fn server_ids_are_24_hex() {
        assert!(is_server_id("0123456789abcdef01234567"));
        assert!(!is_server_id("0123456789ABCDEF01234567"));
        assert!(!is_server_id("0123456789abcdef0123456"));
    }

    /// The command names `generate_handler!` lists in lib.rs, in order.
    fn served() -> Vec<&'static str> {
        let lib = include_str!("../lib.rs");
        lib.split_once("generate_handler![")
            .map_or("", |(_, rest)| rest)
            .split_once("])")
            .map_or("", |(list, _)| list)
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(|item| item.trim_start_matches("commands::"))
            .collect()
    }

    #[test]
    fn every_command_is_served_and_nothing_else() {
        let listed: Vec<&str> = COMMANDS.iter().map(|command| command.name).collect();
        assert_eq!(served(), listed);
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = COMMANDS.iter().map(|command| command.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), COMMANDS.len());
    }

    /// A name list is exactly the enum's names: each one parses, and the TypeScript union ts-rs writes
    /// for the enum has the same names in the same order.
    fn same_names<T: TS + DeserializeOwned>(names: &[&str]) {
        for name in names {
            assert!(
                serde_json::from_value::<T>(serde_json::Value::from(*name)).is_ok(),
                "{name}"
            );
        }
        let union = T::inline(&ts_rs::Config::new());
        let declared: Vec<String> = union
            .split('|')
            .map(|item| item.trim().trim_matches('"').to_owned())
            .collect();
        assert_eq!(declared, names, "{}", T::name(&ts_rs::Config::new()));
    }

    #[test]
    fn name_lists_are_the_enums() {
        same_names::<ExternalTarget>(EXTERNAL_TARGETS);
        same_names::<HostAction>(HOST_ACTIONS);
        same_names::<HostFolder>(HOST_FOLDERS);
        same_names::<HostServerMode>(SERVER_MODES);
    }

    /// Reforged's `preload.ts` today: every `LauncherApi` method maps to a command here, to one of
    /// Tauri's window calls, or to nothing because RD5 dropped it. A method added to the page without a
    /// command fails here.
    #[test]
    fn every_page_call_is_in_the_spec() {
        let preload = include_str!("../../../../../legacy/ReforgedLauncher/src/preload.ts");
        let methods: Vec<&str> = preload
            .split_once("const api = {")
            .map_or("", |(_, rest)| rest)
            .lines()
            .filter_map(|line| {
                let line = line.strip_prefix("  ")?;
                let (name, _) = line.split_once(": ")?;
                name.bytes().all(|byte| byte.is_ascii_alphanumeric()).then_some(name)
            })
            .collect();
        assert!(methods.len() > 50, "{methods:?}");
        let dropped = ["revivedInstall", "revivedChooseFolder", "revivedUseDetected"];
        let window = ["minimize", "toggleMaximize", "close"];
        let events = [
            ("onSnapshot", None),
            ("onProgress", Some("subscribe_progress")),
            ("onHostKit", None),
            ("onHostServer", None),
            ("onInviteLink", None),
            ("onWindowState", None),
        ];
        let listed: Vec<&str> = COMMANDS.iter().map(|command| command.name).collect();
        let mut used = Vec::new();
        for method in &methods {
            if dropped.contains(method) || window.contains(method) {
                continue;
            }
            if let Some((_, command)) = events.iter().find(|(name, _)| name == method) {
                used.extend(*command);
                continue;
            }
            let command = snake_case(method);
            assert!(listed.contains(&command.as_str()), "{method} has no command {command}");
            used.push(Box::leak(command.into_boxed_str()));
        }
        used.sort_unstable();
        let mut all = listed.clone();
        all.sort_unstable();
        assert_eq!(used, all, "a command the page does not call");
    }

    /// `useDetectedGame` -> `use_detected_game`, `hostImportGameData` -> `host_import_gamedata` (the
    /// channel `rr:host-import-gamedata` spells it as one word).
    fn snake_case(method: &str) -> String {
        let mut out = String::new();
        for char in method.chars() {
            if char.is_ascii_uppercase() {
                out.push('_');
            }
            out.push(char.to_ascii_lowercase());
        }
        out.replace("game_data", "gamedata")
    }
}
