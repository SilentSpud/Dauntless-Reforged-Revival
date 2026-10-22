//! Launcher settings in `userData/settings.json` (row L28): `src/main/settings.ts`. No secrets in here:
//! the key lives in the key store. Every value is checked when read, an unknown or wrong one falls back
//! to its default, and the file is written as the Electron launcher writes it (same key order, two-space
//! indent), so an upgraded profile reads and saves to the same bytes, but for the 1.4.4 parts (RD5,
//! RD22): `revivedRoot` and `revivedVerified` are read and not written again, and a saved 1.4.4 server
//! (`game: "revived"`) stays in the list, with its key, but is never the current server.
//!
//! Built on `protocol` and `serde_json` only, so the fuzz target (`rust/fuzz`, `launcher_files`) compiles
//! this file as it is.

use std::path::{Path, PathBuf};

use protocol::account::is_valid_username;
use protocol::invite::{
    clean_server_name, is_private_mode_host, is_valid_fingerprint, is_valid_host, is_valid_invite_code,
    is_valid_share_url,
};
use protocol::text::js_length;
use serde_json::{Map, Value};

use crate::core::i18n::Language;

pub const MAX_SAVED_SERVERS: usize = 20;
/// Runtime file sets kept per server, the newest last
const MAX_TRUSTED_PER_SERVER: usize = 8;

/// `game` of a Dauntless Revived server (Dauntless 1.4.4), which 2.0.0 no longer plays (RD5)
pub const REVIVED: &str = "revived";
/// `game` of every other server; older settings have none
pub const REFORGED: &str = "reforged";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) { Self::Windows } else { Self::Linux }
    }
}

/// A server id (`server_id`): 24 lower-case hex.
fn is_slot(key: &str) -> bool {
    key.len() == 24 && key.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// A runtime file set id: 64 lower-case hex.
fn is_runtime_set(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// `validDir(value, platform, max)`: longer than 3 and shorter than `max` (UTF-16 units), no NUL; a
/// drive-letter path on Windows, any absolute POSIX path elsewhere.
fn valid_dir(value: Option<&Value>, platform: Platform, max: usize) -> Option<&str> {
    let text = value?.as_str()?;
    let length = js_length(text);
    if length <= 3 || length >= max || text.contains('\0') {
        return None;
    }
    let bytes = text.as_bytes();
    let ok = match platform {
        Platform::Windows => {
            bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/')
        }
        Platform::Linux => text.starts_with('/'),
    };
    ok.then_some(text)
}

/// `validDll`: a folder-like path under 260 characters that names a `.dll`.
fn valid_dll(value: Option<&Value>, platform: Platform) -> Option<&str> {
    valid_dir(value, platform, 260).filter(|path| {
        path.len() >= 4 && path.is_char_boundary(path.len() - 4) && path[path.len() - 4..].eq_ignore_ascii_case(".dll")
    })
}

fn text_or_null(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |text| Value::String(text.to_owned()))
}

/// A server entry, or None when it could never be reached safely.
fn sanitize_server(value: Option<&Value>) -> Option<Map<String, Value>> {
    let server = value?.as_object()?;
    let name = server.get("name").and_then(Value::as_str).and_then(clean_server_name)?;
    let public = server.get("mode").and_then(Value::as_str) == Some("public");
    let host = server.get("host").and_then(Value::as_str)?;
    // Private mode is plain HTTP: only into the tailnet (or loopback), as parse_invite enforces
    if !is_valid_host(host) || !public && !is_private_mode_host(host) {
        return None;
    }
    let port = server
        .get("port")
        .and_then(Value::as_f64)
        .filter(|port| port.fract() == 0.0 && (1.0..=65535.0).contains(port))?;
    let fp = server
        .get("fp")
        .and_then(Value::as_str)
        .filter(|fp| is_valid_fingerprint(fp));
    // A public server without a valid fingerprint is dropped
    if public && fp.is_none() {
        return None;
    }
    let share = server
        .get("share")
        .and_then(Value::as_str)
        .filter(|share| !public && is_valid_share_url(share));
    let code = server
        .get("code")
        .and_then(Value::as_str)
        .filter(|code| is_valid_invite_code(code));
    let revived = server.get("game").and_then(Value::as_str) == Some(REVIVED);
    let mut out = Map::new();
    out.insert("game".into(), Value::from(if revived { REVIVED } else { REFORGED }));
    out.insert("mode".into(), Value::from(if public { "public" } else { "private" }));
    out.insert("host".into(), Value::String(host.to_owned()));
    out.insert("port".into(), Value::from(port as u64));
    out.insert("name".into(), Value::String(name));
    out.insert("share".into(), text_or_null(share));
    out.insert("fp".into(), if public { text_or_null(fp) } else { Value::Null });
    out.insert("code".into(), text_or_null(code));
    Some(out)
}

/// The same server (and the same account key slot): mode, host and port.
pub fn same_server(a: &Map<String, Value>, b: &Map<String, Value>) -> bool {
    a.get("mode") == b.get("mode") && a.get("host") == b.get("host") && a.get("port") == b.get("port")
}

/// A saved Dauntless Revived (1.4.4) server: listed, not playable (RD5).
pub fn is_revived(server: &Map<String, Value>) -> bool {
    server.get("game").and_then(Value::as_str) == Some(REVIVED)
}

pub fn default_settings(language: Language) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("version".into(), Value::from(1));
    out.insert("server".into(), Value::Null);
    out.insert("savedServers".into(), Value::Array(Vec::new()));
    out.insert("gameRoot".into(), Value::Null);
    out.insert("graphics".into(), Value::from(DEFAULT_GRAPHICS));
    out.insert("windowed".into(), Value::Bool(false));
    out.insert("streamerMode".into(), Value::Bool(false));
    out.insert("language".into(), Value::from(language.code()));
    out.insert("gameLanguage".into(), Value::from("auto"));
    out.insert("runtimeProfile".into(), Value::from("production"));
    out.insert("hostTools".into(), Value::Bool(false));
    out.insert("usernames".into(), Value::Object(Map::new()));
    out.insert("backupOffered".into(), Value::Object(Map::new()));
    out.insert("trustedRuntimes".into(), Value::Object(Map::new()));
    out.insert("localRuntimeDir".into(), Value::Null);
    out.insert("hostWorkspace".into(), Value::Null);
    out.insert("dumperDll".into(), Value::Null);
    out
}

/// `DEFAULT_GRAPHICS`: leave graphics to the in-game menu
const DEFAULT_GRAPHICS: i64 = -1;

const GAME_LANGUAGES: [&str; 9] = [
    "auto", "en-US", "de-DE", "es-ES", "fr-FR", "it-IT", "ja-JP", "pt-BR", "ru-RU",
];

const RUNTIME_PROFILES: [&str; 2] = ["production", "development"];

/// The entries of an object field, or none.
fn entries<'a>(raw: &'a Map<String, Value>, field: &str) -> impl Iterator<Item = (&'a String, &'a Value)> {
    raw.get(field).and_then(Value::as_object).into_iter().flatten()
}

/// `sanitizeSettings`: the settings as they will be used and written, in the file's key order.
pub fn sanitize_settings(raw: &Value, language: Language, platform: Platform) -> Map<String, Value> {
    let mut out = default_settings(language);
    let Some(raw) = raw.as_object() else { return out };
    let mut server = sanitize_server(raw.get("server"));
    let mut saved: Vec<Map<String, Value>> = Vec::new();
    for entry in raw.get("savedServers").and_then(Value::as_array).into_iter().flatten() {
        if let Some(entry) = sanitize_server(Some(entry))
            && !saved.iter().any(|other| same_server(other, &entry))
            && saved.len() < MAX_SAVED_SERVERS
        {
            saved.push(entry);
        }
    }
    // The current server is always in the list, as it is now (a newer invite or name replaces the old entry)
    if let Some(current) = &server {
        match saved.iter().position(|other| same_server(other, current)) {
            Some(index) => saved[index] = current.clone(),
            None => saved.insert(0, current.clone()),
        }
        saved.truncate(MAX_SAVED_SERVERS);
    }
    // A 1.4.4 server stays in the list (shown disabled, its key kept) but is not the current one
    if server.as_ref().is_some_and(is_revived) {
        server = None;
    }
    out.insert("server".into(), server.map_or(Value::Null, Value::Object));
    out.insert(
        "savedServers".into(),
        Value::Array(saved.into_iter().map(Value::Object).collect()),
    );
    out.insert(
        "gameRoot".into(),
        text_or_null(valid_dir(raw.get("gameRoot"), platform, 200)),
    );
    // `GRAPHICS_PRESETS.includes(graphics)`: 4.0 is 4, -0 is 0
    if let Some(graphics) = raw
        .get("graphics")
        .and_then(Value::as_f64)
        .filter(|graphics| graphics.fract() == 0.0 && (-1.0..=4.0).contains(graphics))
    {
        out.insert("graphics".into(), Value::from(graphics as i64));
    }
    for flag in ["windowed", "streamerMode", "hostTools"] {
        out.insert(flag.into(), Value::Bool(raw.get(flag) == Some(&Value::Bool(true))));
    }
    if let Some(chosen) = raw
        .get("language")
        .and_then(Value::as_str)
        .and_then(Language::from_code)
    {
        out.insert("language".into(), Value::from(chosen.code()));
    }
    for (field, allowed) in [
        ("gameLanguage", GAME_LANGUAGES.as_slice()),
        ("runtimeProfile", RUNTIME_PROFILES.as_slice()),
    ] {
        if let Some(value) = raw
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| allowed.contains(value))
        {
            out.insert(field.into(), Value::from(value));
        }
    }
    // The registration rule (RD22): a name an admin changed to something else is not shown
    let mut usernames = Map::new();
    for (slot, name) in entries(raw, "usernames") {
        if is_slot(slot)
            && let Some(name) = name.as_str().filter(|name| is_valid_username(name))
        {
            usernames.insert(slot.clone(), Value::from(name));
        }
    }
    out.insert("usernames".into(), Value::Object(usernames));
    let mut offered = Map::new();
    for (slot, value) in entries(raw, "backupOffered") {
        if is_slot(slot) && value == &Value::Bool(true) {
            offered.insert(slot.clone(), Value::Bool(true));
        }
    }
    out.insert("backupOffered".into(), Value::Object(offered));
    let mut trusted = Map::new();
    for (slot, value) in entries(raw, "trustedRuntimes") {
        let Some(list) = value.as_array().filter(|_| is_slot(slot)) else {
            continue;
        };
        // `[...new Set(ids)].slice(-8)`: the first of each, the last eight
        let mut ids: Vec<&str> = Vec::new();
        for id in list.iter().filter_map(Value::as_str).filter(|id| is_runtime_set(id)) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        if !ids.is_empty() {
            let keep = &ids[ids.len().saturating_sub(MAX_TRUSTED_PER_SERVER)..];
            trusted.insert(
                slot.clone(),
                Value::Array(keep.iter().map(|id| Value::from(*id)).collect()),
            );
        }
    }
    out.insert("trustedRuntimes".into(), Value::Object(trusted));
    out.insert(
        "localRuntimeDir".into(),
        text_or_null(valid_dir(raw.get("localRuntimeDir"), platform, 260)),
    );
    out.insert(
        "hostWorkspace".into(),
        text_or_null(valid_dir(raw.get("hostWorkspace"), platform, 200)),
    );
    out.insert(
        "dumperDll".into(),
        text_or_null(valid_dll(raw.get("dumperDll"), platform)),
    );
    out
}

/// The file text, as `JSON.stringify(settings, null, 2)`.
pub fn settings_text(settings: &Map<String, Value>) -> String {
    serde_json::to_string_pretty(settings).unwrap_or_default()
}

pub struct SettingsStore {
    file: PathBuf,
    data: Map<String, Value>,
    platform: Platform,
}

impl SettingsStore {
    pub fn load(dir: &Path, language: Language, platform: Platform) -> Self {
        let file = dir.join("settings.json");
        let raw = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or(Value::Null);
        let data = sanitize_settings(&raw, language, platform);
        Self { file, data, platform }
    }

    pub fn get(&self) -> &Map<String, Value> {
        &self.data
    }

    /// Changes a copy, checks it again, and writes it atomically (temporary file, then rename). The file is
    /// a few kilobytes, so it is written in place rather than on a blocking pool.
    pub fn update(&mut self, change: impl FnOnce(&mut Map<String, Value>)) -> std::io::Result<&Map<String, Value>> {
        let mut copy = self.data.clone();
        change(&mut copy);
        let language = copy
            .get("language")
            .and_then(Value::as_str)
            .and_then(Language::from_code)
            .unwrap_or(Language::En);
        self.data = sanitize_settings(&Value::Object(copy), language, self.platform);
        if let Some(parent) = self.file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut temporary = self.file.clone().into_os_string();
        temporary.push(".tmp");
        std::fs::write(&temporary, settings_text(&self.data))?;
        std::fs::rename(&temporary, &self.file)?;
        Ok(&self.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Deserialize)]
    struct Case {
        platform: String,
        #[serde(default)]
        input: Value,
        language: String,
        text: String,
    }

    /// Until RA3 makes the languages `en` and `pt-BR`, a case that stores another language is skipped.
    fn languages_are_reforged() -> bool {
        Language::from_code("pt-BR").is_some() && Language::from_code("fi").is_none()
    }

    /// `test/cases/settings.json`, made by Reforged's `sanitizeSettings` (the Node test
    /// `platform-tables.test.ts` checks it there) less the 1.4.4 parts this launcher drops.
    #[test]
    fn the_shared_table_byte_for_byte() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../test/cases/settings.json");
        let cases: Vec<Case> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(cases.len() > 100);
        let mut run = 0;
        for case in cases {
            let stored_language = case.input.get("language").and_then(Value::as_str);
            if !languages_are_reforged() && stored_language.is_some_and(|code| code != "en" && code != "xx") {
                continue;
            }
            let platform = if case.platform == "win32" {
                Platform::Windows
            } else {
                Platform::Linux
            };
            let language = Language::from_code(&case.language).unwrap();
            assert_eq!(
                settings_text(&sanitize_settings(&case.input, language, platform)),
                case.text,
                "{} {}",
                case.platform,
                case.input
            );
            run += 1;
        }
        assert!(run > 100);
    }

    const FP: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";

    #[test]
    fn a_profile_with_a_1_4_4_server_loads_and_keeps_it() {
        let revived = json!({ "game": "revived", "mode": "public", "host": "old.example.org", "port": 443, "name": "Old", "share": null, "fp": FP, "code": null });
        let raw = json!({
            "version": 1,
            "server": revived,
            "savedServers": [revived, { "mode": "private", "host": "100.64.0.1", "port": 61000, "name": "Friends" }],
            "revivedRoot": "D:\\Dauntless144",
            "revivedVerified": "D:\\Dauntless144",
        });
        let settings = sanitize_settings(&raw, Language::En, Platform::Windows);
        assert_eq!(settings["server"], Value::Null, "never the current server");
        let saved = settings["savedServers"].as_array().unwrap();
        assert_eq!(saved.len(), 2);
        assert!(is_revived(saved[0].as_object().unwrap()));
        assert_eq!(saved[1]["game"], "reforged", "no game: a Reforged server");
        assert!(settings.get("revivedRoot").is_none() && settings.get("revivedVerified").is_none());
    }

    #[test]
    fn usernames_follow_the_registration_rule() {
        let slot = "0123456789abcdef01234567";
        let names = |name: Value| {
            sanitize_settings(&json!({ "usernames": { slot: name } }), Language::En, Platform::Linux)["usernames"]
                .get(slot)
                .cloned()
        };
        assert_eq!(names(json!("Slayer_01")), Some(json!("Slayer_01")));
        for bad in [
            json!("Sam Smith"),
            json!("ab"),
            json!("a".repeat(17)),
            json!("Bjørn"),
            json!(7),
        ] {
            assert_eq!(names(bad.clone()), None, "{bad}");
        }
    }

    #[test]
    fn a_saved_file_reads_back_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SettingsStore::load(dir.path(), Language::En, Platform::Windows);
        store
            .update(|settings| {
                settings.insert("gameRoot".into(), Value::from("D:\\Games\\Dauntless"));
                settings.insert("runtimeProfile".into(), Value::from("development"));
                // Dropped from DR: ignored when stored
                settings.insert("huntRegion".into(), Value::from("ger"));
            })
            .unwrap();
        let text = std::fs::read_to_string(dir.path().join("settings.json")).unwrap();
        let other = Language::ALL
            .into_iter()
            .find(|language| *language != Language::En)
            .unwrap();
        let again = SettingsStore::load(dir.path(), other, Platform::Windows);
        assert_eq!(settings_text(again.get()), text);
        assert_eq!(again.get()["gameRoot"], "D:\\Games\\Dauntless");
        assert_eq!(again.get()["runtimeProfile"], "development");
        assert!(again.get().get("huntRegion").is_none());
        assert_eq!(again.get()["language"], "en", "a saved language wins over the system's");
    }
}
