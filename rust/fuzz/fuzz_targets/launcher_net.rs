//! What the launcher's host API reads from a server (MIGRATION_PLAN.md RA5, RX3; DR's L21, X9): Register
//! and GetUserInfo answers, which decide what is stored as the account key and what goes on the game's
//! command line; runtime manifests, which name the files the launcher writes next to the game; news and
//! the art pack's index and images. The launcher's own parser file, compiled in here, since the launcher
//! crate needs a webview to build; the bridge types it names are copied below.
#![no_main]
#![allow(dead_code)]

use libfuzzer_sys::fuzz_target;
use serde_json::Value;

/// The few bridge types `parse.rs` names (`launcher/src-tauri/src/bridge/types.rs`), with the same fields
mod bridge {
    pub mod types {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum ErrorCode {
            AccountBanned,
            InviteInvalid,
            UsernameInvalid,
            UsernameTaken,
            RegistrationClosed,
            BadRequest,
            ServerError,
            KeyRejected,
            RateLimited,
        }

        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct NewsItem {
            pub date: Option<String>,
            pub title: String,
            pub body: String,
        }

        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct RuntimeFileInfo {
            pub name: String,
            pub size: u64,
            pub sha256: String,
        }
    }
}

#[path = "../../launcher/src-tauri/src/net/hostapi/parse.rs"]
mod parse;

const STATUSES: [u16; 10] = [200, 400, 401, 403, 404, 409, 413, 429, 500, 503];

fuzz_target!(|data: &[u8]| {
    let _ = parse::sniff_image(data);
    let text = String::from_utf8_lossy(data);
    if parse::is_branding_path(&text) {
        assert!(!text.contains("..") && text.is_ascii());
    }
    let _ = parse::clean_news_body(Some(&Value::String(text.to_string())));

    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    for status in STATUSES {
        if let parse::RegisterResult::Ok { key } = parse::read_register(status, Some(&value)) {
            // Only a key that may go on the command line is ever stored
            assert!(status == 200 && protocol::account::is_plausible_account_key(&key));
        }
        if let Ok(info) = parse::read_user_info(status, Some(&value)) {
            assert!(status == 200 && parse::is_valid_user_id(&info.user_id));
            assert!(!info.username.is_empty() && info.username.chars().count() <= 32);
        }
    }
    if let Some(manifest) = parse::parse_runtime_manifest(Some(&value)) {
        // Exactly the two runtime files, sorted, each in range with a plain hash
        let names: Vec<&str> = manifest.files.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(names, protocol::manifest::RUNTIME_FILE_NAMES);
        for file in &manifest.files {
            assert!((1..=protocol::manifest::MAX_RUNTIME_FILE_BYTES).contains(&file.size));
            assert!(file.sha256.len() == 64 && file.sha256.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
        }
        assert!(!manifest.version.is_empty() && manifest.version.chars().count() <= 48);
    }
    let news = parse::parse_news(Some(&value));
    assert!(news.len() <= 30);
    for item in news {
        assert!(!item.title.is_empty() && !item.body.is_empty());
    }
    let branding = parse::parse_branding(Some(&value));
    assert!(branding.backgrounds.len() <= 8);
    assert!(branding.backgrounds.iter().all(|(path, _)| parse::is_branding_path(path)));
});
