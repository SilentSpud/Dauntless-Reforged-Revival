//! The fixed list of calls the page may make (rows L6, L7, L9): one entry per `LauncherApi` method that
//! the backend serves, with the shape of its arguments. Three layers read it: the isolation hook in the
//! page (`isolation/commands.js`, generated from this list), the capability that grants exactly these
//! commands (`build.rs`), and the commands themselves, which check the same limits again.
#![cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "build.rs and the generator read the list; the commands use its limits"
    )
)]

use serde::Serialize;

/// One argument, as the page sends it (JSON, camelCase names).
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Arg {
    /// A string of at most `max` UTF-16 code units (JavaScript's `length`)
    Text {
        max: usize,
    },
    /// The same, or absent
    OptionalText {
        max: usize,
    },
    Flag,
    /// An object whose keys are settings names and whose values are strings, numbers or booleans
    SettingsPatch,
    /// A `Channel` the backend pushes values into
    Channel,
}

#[derive(Debug, Clone, Copy, Serialize)]
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
pub const EXTERNAL_TARGET_MAX: usize = 32;

/// The settings `setSettings` may name.
pub const SETTINGS_KEYS: &[&str] = &[
    "huntRegion",
    "graphics",
    "exposure",
    "windowed",
    "language",
    "gameLanguage",
    "showConsole",
    "streamerMode",
];

pub const COMMANDS: &[Command] = &[
    command("get_snapshot", &[]),
    command("check_invite", &[("text", Arg::Text { max: INVITE_MAX })]),
    command(
        "submit_invite",
        &[
            ("text", Arg::Text { max: INVITE_MAX }),
            ("acceptNewCertificate", Arg::Flag),
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
    command("choose_install_dir", &[]),
    command("use_existing_game_folder", &[]),
    command(
        "use_existing_game_path",
        &[("gamePath", Arg::Text { max: GAME_PATH_MAX })],
    ),
    command("start_install", &[]),
    command("pause_task", &[]),
    command("resume_task", &[]),
    command("cancel_task", &[]),
    command("repair", &[]),
    command("play", &[]),
    command(
        "start_offline",
        &[("username", Arg::OptionalText { max: USERNAME_MAX })],
    ),
    command("stop_offline", &[]),
    command("offline_status", &[]),
    command("set_settings", &[("patch", Arg::SettingsPatch)]),
    command("set_status_polling", &[("on", Arg::Flag)]),
    command("refresh_status", &[]),
    command("get_news", &[]),
    command("get_branding", &[]),
    command(
        "open_external",
        &[(
            "target",
            Arg::Text {
                max: EXTERNAL_TARGET_MAX,
            },
        )],
    ),
    command("open_game_folder", &[]),
    command("logout", &[]),
    command("install_update", &[]),
    command("take_invite_link", &[]),
    command("dismiss_notice", &[]),
    command("dismiss_error", &[]),
    command("offline_export_save", &[]),
    command("offline_import_save", &[]),
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

    #[test]
    fn every_command_is_served_and_nothing_else() {
        let lib = include_str!("../lib.rs");
        let handler = lib
            .split_once("generate_handler![")
            .map_or("", |(_, rest)| rest)
            .split_once("])")
            .map_or("", |(list, _)| list);
        let served: Vec<&str> = handler
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(|item| item.trim_start_matches("commands::"))
            .collect();
        let listed: Vec<&str> = COMMANDS.iter().map(|command| command.name).collect();
        assert_eq!(served, listed);
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = COMMANDS.iter().map(|command| command.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), COMMANDS.len());
    }
}
