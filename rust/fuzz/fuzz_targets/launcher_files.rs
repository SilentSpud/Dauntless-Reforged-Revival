//! What the launcher reads from its own files, which anything running as the player can rewrite
//! (MIGRATION_PLAN.md RX3, RA8): `settings.json`, decrypted key files and their Documents copies, and the
//! Electron launcher's `Local State` and key file framing. The launcher's own files, compiled in here,
//! since the launcher crate needs a webview to build; the AES and DPAPI calls behind the framing are not
//! (see docs/security/keystore.md).
#![no_main]
#![allow(dead_code, unused_imports)]

use libfuzzer_sys::fuzz_target;
use serde_json::Value;

/// The languages the launcher's build script makes from the page's strings, as settings.rs uses them.
mod core {
    pub mod i18n {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Language {
            En,
            PtBr,
        }

        impl Language {
            pub const ALL: [Self; 2] = [Self::En, Self::PtBr];

            pub fn code(self) -> &'static str {
                match self {
                    Self::En => "en",
                    Self::PtBr => "pt-BR",
                }
            }

            pub fn from_code(code: &str) -> Option<Self> {
                Self::ALL.into_iter().find(|language| language.code() == code)
            }
        }
    }
}

#[path = "../../launcher/src-tauri/src/platform/settings.rs"]
mod settings;

#[path = "../../launcher/src-tauri/src/platform/keystore/format.rs"]
mod format;

use core::i18n::Language;
use settings::{Platform, is_revived, sanitize_settings, settings_text};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);

    if let Ok(raw) = serde_json::from_str::<Value>(&text) {
        for platform in [Platform::Windows, Platform::Linux] {
            let settings = sanitize_settings(&raw, Language::En, platform);
            // What is written reads back as itself
            let written = settings_text(&settings);
            let again = sanitize_settings(&serde_json::from_str(&written).unwrap(), Language::En, platform);
            assert_eq!(settings_text(&again), written);
            // A 1.4.4 server is never the one the key and the game go to
            if let Some(server) = settings["server"].as_object() {
                assert!(!is_revived(server));
            }
            // A public server is only ever reached with its pinned certificate
            for server in settings["savedServers"].as_array().unwrap() {
                if server["mode"] == "public" {
                    assert!(
                        server["fp"]
                            .as_str()
                            .is_some_and(protocol::invite::is_valid_fingerprint)
                    );
                }
            }
            for name in settings["usernames"].as_object().unwrap().values() {
                assert!(protocol::account::is_valid_username(name.as_str().unwrap()));
            }
        }
    }

    if let Some(stored) = format::parse_stored(&text) {
        // A key that reads is a key the game's command line can take, and is written back the same
        assert!(protocol::account::is_plausible_account_key(&stored.key));
        assert_eq!(format::parse_stored(&stored.text()), Some(stored));
    }
    let _ = format::local_state_key(&text);
    let _ = format::dpapi_blob(data);
    if let Some((nonce, sealed)) = format::windows_blob(data) {
        assert!(nonce.len() == 12 && sealed.len() >= 16);
    }
    if let Some((_, sealed)) = format::linux_blob(data) {
        assert!(!sealed.is_empty() && sealed.len() % 16 == 0);
    }
});
