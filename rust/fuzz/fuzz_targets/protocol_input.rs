//! What the launcher reads from outside through the `protocol` crate (MIGRATION_PLAN.md RS1, RX3): invite
//! links, which decide where the account key goes; server status answers; usernames and pasted account
//! keys; and runtime manifests from a content server. None of it may panic.
#![no_main]

use libfuzzer_sys::fuzz_target;
use protocol::account::{check_username, extract_account_key, is_plausible_account_key};
use protocol::invite::{ServerMode, format_invite, parse_invite};
use protocol::manifest::RuntimeManifest;
use protocol::status::parse_server_status;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Ok(invite) = parse_invite(&text) {
        // An invite that parses names a host the key may go to: never empty, and a public one is pinned
        assert!(!invite.host.is_empty() && invite.port != 0);
        assert!(invite.mode != ServerMode::Public || invite.fp.is_some());
        // And it writes back as an invite that reads the same
        let written = format_invite(&invite);
        assert_eq!(written.as_deref().map(parse_invite), Ok(Ok(invite)));
    }
    let _ = check_username(&text);
    if let Some(key) = extract_account_key(&text) {
        assert!(is_plausible_account_key(key));
    }
    if let Ok(value) = serde_json::from_str::<Value>(&text) {
        if let Some(status) = parse_server_status(&value) {
            assert!(!status.limited || status.players.is_empty() && status.instances.is_empty());
        }
    }
    if let Ok(manifest) = serde_json::from_str::<RuntimeManifest>(&text) {
        let _ = manifest.to_json();
    }
});
