//! What the launcher reads while it handles the server's game runtime (MIGRATION_PLAN.md RA7, RX3): the
//! runtime manifest a content server sends, which decides which files are written next to the game, and
//! the hash cache file (`file-hashes.json`) read back at start. The launcher's own files, compiled in
//! here, since the launcher crate needs a webview to build. See docs/security/runtime-download.md.
#![no_main]

use libfuzzer_sys::fuzz_target;
use protocol::manifest::{MAX_RUNTIME_FILE_BYTES, RUNTIME_FILE_NAMES};
use serde_json::Value;

#[path = "../../launcher/src-tauri/src/net/runtime_files/manifest.rs"]
mod manifest;

#[path = "../../launcher/src-tauri/src/net/verify/cache_file.rs"]
mod cache_file;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Ok(value) = serde_json::from_str::<Value>(&text) {
        if let Some(parsed) = manifest::parse_runtime_manifest(&value) {
            // Only the two runtime names, each once and sorted, so nothing else is ever written next to
            // the game; a plain hash and a size within the cap
            let names: Vec<&str> = parsed.files.iter().map(|file| file.name.as_str()).collect();
            assert_eq!(names, RUNTIME_FILE_NAMES);
            for file in &parsed.files {
                assert!((1..=MAX_RUNTIME_FILE_BYTES).contains(&file.size));
                assert!(file.sha256.len() == 64 && file.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
            }
            assert!(!parsed.version.is_empty() && parsed.version.chars().count() <= 48);
            // And it writes back as a manifest with the same files and build (the version is cleaned
            // text, which a second cleaning may trim further)
            let again = serde_json::from_str::<Value>(&parsed.to_json())
                .ok()
                .as_ref()
                .and_then(manifest::parse_runtime_manifest);
            assert_eq!(again.map(|m| (m.files, m.build)), Some((parsed.files, parsed.build)));
        }
    }
    let entries = cache_file::parse_cache_file(&text);
    assert!(entries.len() <= cache_file::MAX_ENTRIES);
    assert_eq!(cache_file::parse_cache_file(&cache_file::write_cache_file(&entries)).len(), entries.len());
});
