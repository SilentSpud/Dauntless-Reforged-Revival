//! The runtime manifest: the two files of the game runtime a host builds (`MysticParadox.dll` and
//! `winmm.dll`), their sizes and SHA-256 hashes. The content server serves it at [`RUNTIME_MANIFEST_PATH`],
//! as `legacy/ReforgedContent/src/runtime.ts` writes it, and the launcher reads it before it offers the
//! runtime to the player. Checking a `runtime.json` (RS2) and the files on disk (RA7) stays with each side;
//! this is the shape they agree on, serialised byte for byte as Node's `JSON.stringify` does.

use serde::{Deserialize, Serialize};

pub const RUNTIME_MANIFEST_PATH: &str = "/content/v1/runtime/manifest";
/// The runtime is exactly these two files, each listed once.
pub const RUNTIME_FILE_NAMES: [&str; 2] = ["MysticParadox.dll", "winmm.dll"];
pub const MAX_RUNTIME_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// The build every manifest names, whatever the host's `runtime.json` says: Reforged plays this game only.
pub const RUNTIME_BUILD_NAME: &str = "rel-1.14.7-Archon";
/// The changelist of a `runtime.json` that names none.
pub const DEFAULT_CHANGELIST: u64 = 647_472;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RuntimeFile {
    pub name: String,
    pub size: u64,
    /// 64 lower-case hex
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RuntimeBuild {
    /// Always [`RUNTIME_BUILD_NAME`] in a manifest made by [`RuntimeManifest::new`]
    pub name: String,
    pub changelist: u64,
}

/// Fields in Node's order: `version`, `build { name, changelist }`, `files [{ name, size, sha256 }]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RuntimeManifest {
    pub version: String,
    pub build: RuntimeBuild,
    /// Sorted by name
    pub files: Vec<RuntimeFile>,
}

impl RuntimeManifest {
    /// The manifest as the content server writes it: `build.name` forced to [`RUNTIME_BUILD_NAME`] and
    /// the files sorted by name (`ParseRuntimeIndex`'s last step). The caller has checked the values.
    pub fn new(version: String, changelist: u64, mut files: Vec<RuntimeFile>) -> Self {
        files.sort_by(|a, b| a.name.cmp(&b.name));
        Self {
            version,
            build: RuntimeBuild {
                name: RUNTIME_BUILD_NAME.to_owned(),
                changelist,
            },
            files,
        }
    }

    /// The response body, as `JSON.stringify(index)`.
    pub fn to_json(&self) -> String {
        // Strings, integers and lists only: serde_json cannot fail on them
        serde_json::to_string(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_forces_the_build_name_and_sorts() {
        let file = |name: &str| RuntimeFile {
            name: name.into(),
            size: 1,
            sha256: "a".repeat(64),
        };
        let manifest = RuntimeManifest::new("1".into(), 5, vec![file("winmm.dll"), file("MysticParadox.dll")]);
        assert_eq!(manifest.build.name, RUNTIME_BUILD_NAME);
        assert_eq!(manifest.files[0].name, "MysticParadox.dll");
        assert!(manifest.to_json().starts_with(r#"{"version":"1","build":{"name":"rel-1.14.7-Archon","changelist":5},"files":[{"name":"MysticParadox.dll","size":1,"sha256":"#));
    }
}
