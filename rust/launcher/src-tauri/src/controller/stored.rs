//! A typed view of `settings.json` for the controller (`StoredSettings` in `src/main/settings.ts`). The
//! store keeps the file as a JSON map so it writes the Electron launcher's exact bytes; the controller
//! reads it through this view and changes it through the map.

use std::collections::HashMap;

use protocol::invite::{Invite, ServerMode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::bridge::types::{GameLanguage, GraphicsPreset, Language};
use crate::net::http::Endpoint;
use crate::platform::keystore::{KeySlot, server_id};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredServer {
    pub mode: ServerMode,
    pub host: String,
    pub port: u16,
    pub name: String,
    pub share: Option<String>,
    pub fp: Option<String>,
    /// The invite code, until an account exists
    pub code: Option<String>,
}

impl StoredServer {
    pub fn from_invite(invite: &Invite, code: Option<String>) -> Self {
        Self {
            mode: invite.mode,
            host: invite.host.clone(),
            port: invite.port,
            name: invite.name.clone(),
            share: invite.share.clone(),
            fp: invite.fp.clone(),
            code,
        }
    }

    pub fn public(&self) -> bool {
        self.mode == ServerMode::Public
    }

    /// `endpointFor`
    pub fn endpoint(&self) -> Endpoint {
        Endpoint {
            host: self.host.clone(),
            port: self.port,
            pin: if self.public() { self.fp.clone() } else { None },
        }
    }

    pub fn slot(&self) -> KeySlot {
        KeySlot {
            host: self.host.clone(),
            port: self.port,
            public: self.public(),
            fp: self.fp.clone(),
        }
    }

    pub fn id(&self) -> String {
        server_id(&self.host, self.port, self.public())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stored {
    pub server: Option<StoredServer>,
    pub saved_servers: Vec<StoredServer>,
    pub install_dir: Option<String>,
    pub verified_dir: Option<String>,
    pub graphics: GraphicsPreset,
    pub windowed: bool,
    pub streamer_mode: bool,
    pub language: Language,
    pub game_language: GameLanguage,
    pub usernames: HashMap<String, String>,
    pub backup_offered: HashMap<String, bool>,
}

impl Stored {
    /// The view of a checked settings map (`sanitize_settings` output always fits it).
    pub fn of(map: &Map<String, Value>) -> Self {
        serde_json::from_value(Value::Object(map.clone())).unwrap_or_default()
    }
}

impl Default for Stored {
    fn default() -> Self {
        Self {
            server: None,
            saved_servers: Vec::new(),
            install_dir: None,
            verified_dir: None,
            graphics: GraphicsPreset::DEFAULT,
            windowed: false,
            streamer_mode: false,
            language: Language::En,
            game_language: GameLanguage::Auto,
            usernames: HashMap::new(),
            backup_offered: HashMap::new(),
        }
    }
}

/// Map edits, as `settings.update((s) => …)` makes them in TypeScript.
pub fn set_server(map: &mut Map<String, Value>, server: Option<&StoredServer>) {
    let value = server
        .and_then(|server| serde_json::to_value(server).ok())
        .unwrap_or(Value::Null);
    map.insert("server".into(), value);
}

pub fn edit_server(map: &mut Map<String, Value>, field: &str, value: Value) {
    if let Some(server) = map.get_mut("server").and_then(Value::as_object_mut) {
        server.insert(field.into(), value);
    }
}

/// Changes the object at `field`, made first when it is not one.
fn edit_object(map: &mut Map<String, Value>, field: &str, change: impl FnOnce(&mut Map<String, Value>)) {
    if let Some(Value::Object(object)) = map.get_mut(field) {
        change(object);
        return;
    }
    let mut object = Map::new();
    change(&mut object);
    map.insert(field.into(), Value::Object(object));
}

pub fn set_username(map: &mut Map<String, Value>, id: &str, name: Option<&str>) {
    edit_object(map, "usernames", |names| match name {
        Some(name) => {
            names.insert(id.into(), Value::String(name.into()));
        }
        None => {
            names.remove(id);
        }
    });
}

pub fn set_backup_offered(map: &mut Map<String, Value>, id: &str, offered: bool) {
    edit_object(map, "backupOffered", |offers| {
        if offered {
            offers.insert(id.into(), Value::Bool(true));
        } else {
            offers.remove(id);
        }
    });
}

pub fn set_text(map: &mut Map<String, Value>, field: &str, value: Option<&str>) {
    map.insert(
        field.into(),
        value.map_or(Value::Null, |text| Value::String(text.into())),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::settings::{Platform, default_settings, sanitize_settings};

    #[test]
    fn reads_every_checked_settings_file() {
        let defaults = Stored::of(&default_settings(Language::De));
        assert!(defaults.server.is_none());
        assert_eq!(defaults.language, Language::De);
        assert_eq!(defaults.graphics, GraphicsPreset::DEFAULT);

        let mut map = default_settings(Language::En);
        let server = StoredServer {
            mode: ServerMode::Private,
            host: "100.64.0.1".into(),
            port: 61000,
            name: "Friends".into(),
            share: None,
            fp: None,
            code: Some("abcd".into()),
        };
        set_server(&mut map, Some(&server));
        set_username(&mut map, &server.id(), Some("Sam"));
        set_backup_offered(&mut map, &server.id(), true);
        let map = sanitize_settings(&Value::Object(map), Language::En, Platform::current());
        let stored = Stored::of(&map);
        assert_eq!(stored.server.as_ref(), Some(&server));
        assert_eq!(stored.saved_servers, vec![server.clone()]);
        assert_eq!(stored.usernames.get(&server.id()).map(String::as_str), Some("Sam"));
        assert_eq!(stored.backup_offered.get(&server.id()), Some(&true));
    }
}
