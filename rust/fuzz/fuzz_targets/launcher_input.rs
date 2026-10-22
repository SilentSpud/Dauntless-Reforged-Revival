//! What the launcher reads from outside (MIGRATION_PLAN.md RX3; DR's L11, L12, X9): invite links, which
//! decide where the account key goes; server status answers; usernames and pasted account keys. The
//! launcher's own files, compiled in here, since the launcher crate needs a webview to build. DR's 1.4.4
//! manifest checks are not brought in (RD5).
#![no_main]
#![allow(dead_code, unused_imports)]

use libfuzzer_sys::fuzz_target;
use serde_json::Value;

#[path = "../../launcher/src-tauri/src/core/text.rs"]
mod text;

#[path = "../../launcher/src-tauri/src/core/invite.rs"]
mod invite;

#[path = "../../launcher/src-tauri/src/core/status.rs"]
mod status;

#[path = "../../launcher/src-tauri/src/core/username.rs"]
mod username;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Ok(parsed) = invite::parse_invite(&text) {
        // An invite that parses names a host the key may go to: never empty, and a public one is pinned
        assert!(!parsed.host.is_empty() && parsed.port != 0);
        assert!(parsed.mode != protocol::invite::ServerMode::Public || parsed.fp.is_some());
    }
    let _ = username::check_username(&text);
    if let Some(key) = username::extract_account_key(&text) {
        assert!(username::is_plausible_account_key(key));
    }
    if let Ok(value) = serde_json::from_str::<Value>(&text) {
        let _ = status::parse_server_status(&value);
    }
});
