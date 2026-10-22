//! The host's `runtime.json` as the content server reads it (MIGRATION_PLAN.md RS2, RX3;
//! docs/security/content.md). What it accepts becomes the manifest every player's launcher is
//! offered, so whatever the bytes: no panic, and an accepted index lists exactly the two runtime
//! files, sorted, each once, with a size in range and a lower-case SHA-256, under the one build name;
//! its body reads back to the same manifest and parses again to itself.
#![no_main]

#[allow(dead_code)]
#[path = "../../services/content/src/input.rs"]
mod input;

use input::parse_runtime_json;
use libfuzzer_sys::fuzz_target;
use protocol::manifest::{MAX_RUNTIME_FILE_BYTES, RUNTIME_BUILD_NAME, RUNTIME_FILE_NAMES, RuntimeManifest};

fuzz_target!(|data: &[u8]| {
    let Ok(manifest) = parse_runtime_json(data) else {
        return;
    };
    assert_eq!(manifest.build.name, RUNTIME_BUILD_NAME);
    assert!(manifest.build.changelist > 0 && manifest.build.changelist <= 9_007_199_254_740_991);
    assert!((1..=48).contains(&manifest.version.len()));
    let names: Vec<&str> = manifest.files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, RUNTIME_FILE_NAMES);
    for file in &manifest.files {
        assert!((1..=MAX_RUNTIME_FILE_BYTES).contains(&file.size));
        assert!(file.sha256.len() == 64 && file.sha256.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
    }
    let body = manifest.to_json();
    let read: RuntimeManifest = serde_json::from_str(&body).expect("the body reads back");
    assert_eq!(read, manifest);
    let again = parse_runtime_json(body.as_bytes()).expect("the body is a runtime.json too");
    assert_eq!(again, manifest);
});
