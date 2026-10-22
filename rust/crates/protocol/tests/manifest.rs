//! The runtime manifest against bodies recorded from `legacy/ReforgedContent` (`RuntimeStore.get().body`),
//! by `tests/fixtures/record.js`: each `*.input.json` is the `runtime.json` the host wrote, the matching
//! `*.json` what Node served for it.

#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

use protocol::manifest::{DEFAULT_CHANGELIST, RUNTIME_BUILD_NAME, RuntimeFile, RuntimeManifest};
use protocol::text::js_trim;
use serde_json::Value;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// What `ParseRuntimeIndex` keeps of a `runtime.json` that passes its checks (RS2 ports the checks).
fn from_runtime_json(text: &str) -> RuntimeManifest {
    let raw: Value = serde_json::from_str(text).unwrap();
    let changelist = raw["build"]["changelist"].as_u64().unwrap_or(DEFAULT_CHANGELIST);
    let files = raw["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| RuntimeFile {
            name: file["name"].as_str().unwrap().into(),
            size: file["size"].as_u64().unwrap(),
            sha256: file["sha256"].as_str().unwrap().into(),
        })
        .collect();
    RuntimeManifest::new(js_trim(raw["version"].as_str().unwrap()).into(), changelist, files)
}

#[test]
fn byte_for_byte_as_node_serves_it() {
    for name in ["runtime-manifest", "runtime-manifest-defaults"] {
        let body = fixture(&format!("{name}.json"));
        let manifest = from_runtime_json(&fixture(&format!("{name}.input.json")));
        assert_eq!(manifest.build.name, RUNTIME_BUILD_NAME);
        assert_eq!(manifest.to_json(), body, "{name}");
        // And what the launcher reads back serialises the same
        let read: RuntimeManifest = serde_json::from_str(&body).unwrap();
        assert_eq!(read, manifest);
        assert_eq!(serde_json::to_string(&read).unwrap(), body);
    }
}

#[test]
fn the_recording_kept_the_host_s_changelist_and_dropped_its_build_name() {
    let input = fixture("runtime-manifest.input.json");
    assert!(input.contains("ignored by the server") && !fixture("runtime-manifest.json").contains("ignored"));
    assert_eq!(from_runtime_json(&input).build.changelist, 651_234);
    assert_eq!(
        from_runtime_json(&fixture("runtime-manifest-defaults.input.json"))
            .build
            .changelist,
        DEFAULT_CHANGELIST
    );
}
