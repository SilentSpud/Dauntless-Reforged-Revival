//! Launcher settings in `userData/settings.json` (row L28): `src/main/settings.ts`. No secrets in here:
//! the key lives in the key store. Every value is checked when read, an unknown or wrong one falls back
//! to its default, and the file is written as the Electron launcher writes it (same key order, two-space
//! indent), so an upgraded profile reads and saves to the same bytes.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::core::i18n::Language;
use crate::core::invite::{
    clean_server_name, is_private_mode_host, is_valid_host, is_valid_invite_code, is_valid_share_url,
};
use crate::core::status::clean_text;
use crate::core::text::js_length;

pub const MAX_SAVED_SERVERS: usize = 20;

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

fn is_slot(key: &str) -> bool {
    key.len() == 24 && key.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn valid_dir(value: Option<&Value>, platform: Platform) -> Value {
    let Some(text) = value.and_then(Value::as_str) else {
        return Value::Null;
    };
    let length = js_length(text);
    if length <= 3 || length >= 200 || text.contains('\0') {
        return Value::Null;
    }
    let bytes = text.as_bytes();
    let ok = match platform {
        Platform::Windows => {
            bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/')
        }
        Platform::Linux => text.starts_with('/'),
    };
    if ok {
        Value::String(text.to_owned())
    } else {
        Value::Null
    }
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
        .filter(|fp| protocol::invite::is_valid_fingerprint(fp));
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
    let text = |value: Option<&str>| value.map_or(Value::Null, |text| Value::String(text.to_owned()));
    let mut out = Map::new();
    out.insert(
        "mode".into(),
        Value::String(if public { "public" } else { "private" }.into()),
    );
    out.insert("host".into(), Value::String(host.to_owned()));
    out.insert("port".into(), Value::from(port as u64));
    out.insert("name".into(), Value::String(name));
    out.insert("share".into(), text(share));
    out.insert("fp".into(), if public { text(fp) } else { Value::Null });
    out.insert("code".into(), text(code));
    Some(out)
}

/// The same server (and the same account key slot): mode, host and port.
pub fn same_server(a: &Map<String, Value>, b: &Map<String, Value>) -> bool {
    a.get("mode") == b.get("mode") && a.get("host") == b.get("host") && a.get("port") == b.get("port")
}

pub fn default_settings(language: Language) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("version".into(), Value::from(1));
    out.insert("server".into(), Value::Null);
    out.insert("savedServers".into(), Value::Array(Vec::new()));
    out.insert("installDir".into(), Value::Null);
    out.insert("verifiedDir".into(), Value::Null);
    out.insert("graphics".into(), Value::from(4));
    out.insert("exposure".into(), Value::from("game"));
    out.insert("windowed".into(), Value::Bool(false));
    out.insert("streamerMode".into(), Value::Bool(false));
    out.insert("language".into(), Value::from(language.code()));
    out.insert("gameLanguage".into(), Value::from("auto"));
    out.insert("showConsole".into(), Value::Bool(false));
    out.insert("usernames".into(), Value::Object(Map::new()));
    out.insert("backupOffered".into(), Value::Object(Map::new()));
    out
}

const GAME_LANGUAGES: [&str; 9] = [
    "auto", "en-US", "de-DE", "es-ES", "fr-FR", "it-IT", "ja-JP", "pt-BR", "ru-RU",
];

/// `sanitizeSettings`: the settings as they will be used and written, in the file's key order.
pub fn sanitize_settings(raw: &Value, language: Language, platform: Platform) -> Map<String, Value> {
    let mut out = default_settings(language);
    let Some(raw) = raw.as_object() else { return out };
    let server = sanitize_server(raw.get("server"));
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
    out.insert("server".into(), server.map_or(Value::Null, Value::Object));
    out.insert(
        "savedServers".into(),
        Value::Array(saved.into_iter().map(Value::Object).collect()),
    );
    out.insert("installDir".into(), valid_dir(raw.get("installDir"), platform));
    out.insert("verifiedDir".into(), valid_dir(raw.get("verifiedDir"), platform));
    if let Some(graphics) = raw
        .get("graphics")
        .and_then(Value::as_i64)
        .filter(|g| (-1..=4).contains(g))
        && raw
            .get("graphics")
            .is_some_and(|g| g.as_f64().is_some_and(|f| f.fract() == 0.0))
    {
        out.insert("graphics".into(), Value::from(graphics));
    }
    if let Some(exposure) = raw
        .get("exposure")
        .and_then(Value::as_str)
        .filter(|e| *e == "game" || *e == "basic")
    {
        out.insert("exposure".into(), Value::from(exposure));
    }
    for flag in ["windowed", "streamerMode", "showConsole"] {
        out.insert(flag.into(), Value::Bool(raw.get(flag) == Some(&Value::Bool(true))));
    }
    match raw.get("huntRegion").and_then(Value::as_str) {
        // The retired region, once, when loading old settings
        Some("aus") => {
            out.insert("huntRegion".into(), Value::from("us"));
        }
        Some(region @ ("auto" | "main" | "ger" | "us")) => {
            out.insert("huntRegion".into(), Value::from(region));
        }
        _ => {}
    }
    if let Some(chosen) = raw
        .get("language")
        .and_then(Value::as_str)
        .and_then(Language::from_code)
    {
        out.insert("language".into(), Value::from(chosen.code()));
    }
    if let Some(game) = raw
        .get("gameLanguage")
        .and_then(Value::as_str)
        .filter(|g| GAME_LANGUAGES.contains(g))
    {
        out.insert("gameLanguage".into(), Value::from(game));
    }
    // Authenticated display names may predate registration rules or be renamed by an admin: the safe
    // display-text rule of GetUserInfo, not the sign-up form's
    let mut usernames = Map::new();
    for (slot, name) in raw.get("usernames").and_then(Value::as_object).into_iter().flatten() {
        if is_slot(slot)
            && let Some(clean) = clean_text(Some(name), 32).filter(|clean| Some(clean.as_str()) == name.as_str())
        {
            usernames.insert(slot.clone(), Value::String(clean));
        }
    }
    out.insert("usernames".into(), Value::Object(usernames));
    let mut offered = Map::new();
    for (slot, value) in raw
        .get("backupOffered")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        if is_slot(slot) && value == &Value::Bool(true) {
            offered.insert(slot.clone(), Value::Bool(true));
        }
    }
    out.insert("backupOffered".into(), Value::Object(offered));
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

    /// Changes a copy, checks it again, and writes it atomically (temporary file, then rename).
    pub async fn update(
        &mut self,
        change: impl FnOnce(&mut Map<String, Value>),
    ) -> std::io::Result<&Map<String, Value>> {
        let mut copy = self.data.clone();
        change(&mut copy);
        let language = copy
            .get("language")
            .and_then(Value::as_str)
            .and_then(Language::from_code)
            .unwrap_or(Language::En);
        self.data = sanitize_settings(&Value::Object(copy), language, self.platform);
        if let Some(parent) = self.file.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut temporary = self.file.clone().into_os_string();
        temporary.push(".tmp");
        tokio::fs::write(&temporary, settings_text(&self.data)).await?;
        tokio::fs::rename(&temporary, &self.file).await?;
        Ok(&self.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Case {
        platform: String,
        #[serde(default)]
        input: Value,
        language: String,
        text: String,
    }

    #[test]
    fn the_shared_table_byte_for_byte() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../test/cases/settings.json");
        let cases: Vec<Case> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(cases.len() > 60);
        for case in cases {
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
        }
    }

    #[tokio::test]
    async fn a_saved_file_reads_back_the_same() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SettingsStore::load(dir.path(), Language::En, Platform::Windows);
        store
            .update(|settings| {
                settings.insert("huntRegion".into(), Value::from("ger"));
                settings.insert("installDir".into(), Value::from("D:\\Games\\DR"));
            })
            .await
            .unwrap();
        let text = std::fs::read_to_string(dir.path().join("settings.json")).unwrap();
        let again = SettingsStore::load(dir.path(), Language::Fi, Platform::Windows);
        assert_eq!(settings_text(again.get()), text);
        assert_eq!(again.get()["huntRegion"], "ger");
        assert_eq!(again.get()["language"], "en", "a saved language wins over the system's");
    }
}
