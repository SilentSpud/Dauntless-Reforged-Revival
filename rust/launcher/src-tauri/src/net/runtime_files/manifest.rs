//! The server's runtime manifest, checked before anything is offered to the player (Reforged's
//! `parseRuntimeManifest`, `src/main/hostapi.ts` 177–196). `protocol` holds the shape; the checks are the
//! launcher's: exactly the two runtime names, each once, a lower-case SHA-256 and a size from 1 byte to
//! 64 MiB; anything else rejects the whole manifest. Only `protocol` and `serde_json`, so the fuzz target
//! compiles this file as it is.

use protocol::manifest::{
    DEFAULT_CHANGELIST, MAX_RUNTIME_FILE_BYTES, RUNTIME_FILE_NAMES, RuntimeFile, RuntimeManifest,
};
use protocol::status::clean_text;
use serde_json::Value;

/// `Number.MAX_SAFE_INTEGER`
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// `Number.isSafeInteger` for a non-negative JSON number (`5` and `5.0` alike).
fn safe_integer(value: &Value) -> Option<u64> {
    if let Some(n) = value.as_u64() {
        return (n <= MAX_SAFE_INTEGER).then_some(n);
    }
    let n = value.as_f64()?;
    // The range check first, so the cast below is exact
    (n.fract() == 0.0 && (0.0..=MAX_SAFE_INTEGER as f64).contains(&n)).then_some(n as u64)
}

fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The manifest, files sorted by name, or None when anything in it is off.
pub fn parse_runtime_manifest(raw: &Value) -> Option<RuntimeManifest> {
    let raw = raw.as_object()?;
    let listed = raw.get("files")?.as_array()?;
    let version = clean_text(raw.get("version"), 48)?;
    let mut files: Vec<RuntimeFile> = Vec::with_capacity(RUNTIME_FILE_NAMES.len());
    for file in listed {
        let file = file.as_object()?;
        let name = file.get("name")?.as_str()?;
        if !RUNTIME_FILE_NAMES.contains(&name) || files.iter().any(|seen| seen.name == name) {
            return None;
        }
        let sha256 = file.get("sha256")?.as_str()?;
        if !is_sha256_hex(sha256) {
            return None;
        }
        let size = safe_integer(file.get("size")?)?;
        if !(1..=MAX_RUNTIME_FILE_BYTES).contains(&size) {
            return None;
        }
        files.push(RuntimeFile {
            name: name.to_owned(),
            size,
            sha256: sha256.to_owned(),
        });
    }
    // Both files, or the runtime would not load
    if files.len() != RUNTIME_FILE_NAMES.len() {
        return None;
    }
    // Reforged keeps an unusable changelist as null; the shape here has a number, so the default stands in
    let changelist = raw
        .get("build")
        .and_then(|build| build.get("changelist"))
        .and_then(safe_integer)
        .unwrap_or(DEFAULT_CHANGELIST);
    Some(RuntimeManifest::new(version, changelist, files))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn good() -> Value {
        json!({
            "version": "20261014-abcdef12",
            "build": { "name": "rel-1.14.7-Archon", "changelist": 647472 },
            "files": [
                { "name": "MysticParadox.dll", "size": 200_001, "sha256": "a".repeat(64) },
                { "name": "winmm.dll", "size": 40_001, "sha256": "b".repeat(64) },
            ],
        })
    }

    fn with_file(index: usize, field: &str, value: Value) -> Value {
        let mut manifest = good();
        manifest["files"][index][field] = value;
        manifest
    }

    /// runtime.test.ts: "manifest: only the two known files, each once, with a plain SHA-256 and a sane size"
    #[test]
    fn only_the_two_known_files_each_once_with_a_plain_sha256_and_a_sane_size() {
        assert!(parse_runtime_manifest(&good()).is_some());
        let mut one = good();
        one["files"].as_array_mut().unwrap().truncate(1);
        assert_eq!(parse_runtime_manifest(&one), None, "both files are needed");
        let mut twice = good();
        let first = twice["files"][0].clone();
        twice["files"].as_array_mut().unwrap().push(first);
        assert_eq!(parse_runtime_manifest(&twice), None, "no duplicates");
        assert_eq!(
            parse_runtime_manifest(&with_file(0, "name", json!("..\\evil.dll"))),
            None
        );
        assert_eq!(parse_runtime_manifest(&with_file(0, "name", json!("dxgi.dll"))), None);
        assert_eq!(parse_runtime_manifest(&with_file(0, "sha256", json!("ABC"))), None);
        assert_eq!(
            parse_runtime_manifest(&with_file(0, "sha256", json!("A".repeat(64)))),
            None
        );
        assert_eq!(parse_runtime_manifest(&with_file(0, "size", json!(0))), None);
        assert_eq!(parse_runtime_manifest(&with_file(0, "size", json!(1e12))), None);
        assert_eq!(parse_runtime_manifest(&with_file(0, "size", json!(1.5))), None);
        assert_eq!(parse_runtime_manifest(&with_file(0, "size", json!(-1))), None);
        assert_eq!(parse_runtime_manifest(&with_file(0, "size", json!("5"))), None);
        let mut empty_version = good();
        empty_version["version"] = json!("");
        assert_eq!(parse_runtime_manifest(&empty_version), None);
        assert_eq!(parse_runtime_manifest(&json!([])), None);
        let mut reversed = good();
        reversed["files"].as_array_mut().unwrap().reverse();
        let names: Vec<_> = parse_runtime_manifest(&reversed)
            .unwrap()
            .files
            .into_iter()
            .map(|file| file.name)
            .collect();
        assert_eq!(names, ["MysticParadox.dll", "winmm.dll"]);
    }

    #[test]
    fn the_build_name_is_forced_and_a_bad_changelist_falls_back() {
        let mut other = good();
        other["build"] = json!({ "name": "rel-1.4.4", "changelist": 5.0 });
        let manifest = parse_runtime_manifest(&other).unwrap();
        assert_eq!(manifest.build.name, protocol::manifest::RUNTIME_BUILD_NAME);
        assert_eq!(manifest.build.changelist, 5);
        other["build"] = json!({ "changelist": "x" });
        assert_eq!(
            parse_runtime_manifest(&other).unwrap().build.changelist,
            DEFAULT_CHANGELIST
        );
        other.as_object_mut().unwrap().remove("build");
        assert!(parse_runtime_manifest(&other).is_some());
        assert!(parse_runtime_manifest(&with_file(0, "size", json!(200_001.0))).is_some());
    }

    #[test]
    fn the_version_is_cleaned_and_cut_to_48() {
        let mut long = good();
        long["version"] = json!(format!("  v\u{202e}{}\n", "9".repeat(60)));
        let version = parse_runtime_manifest(&long).unwrap().version;
        assert_eq!(version, format!("v{}", "9".repeat(47)));
    }
}
