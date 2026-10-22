//! The game manifest (shared contract 4): the exact list of Dauntless 1.4.4 files, their sizes and SHA-256
//! hashes. The content server serves it at `/content/v1/manifest`, and the launcher compiles in the same list
//! and refuses a server whose list differs. The checks (safe paths, the build, the totals) stay with each
//! side, which must give the same reasons; this is the shape they agree on.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFile {
    /// Relative, with forward slashes
    pub path: String,
    pub size: u64,
    /// 64 lower-case hex
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameManifest {
    pub build: String,
    pub total_bytes: u64,
    /// Sorted by path
    pub files: Vec<ManifestFile>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_json_form_the_launchers_read() {
        let manifest = GameManifest {
            build: "b".into(),
            total_bytes: 3,
            files: vec![ManifestFile {
                path: "a/b.pak".into(),
                size: 3,
                sha256: "f".repeat(64),
            }],
        };
        let text = serde_json::to_string(&manifest).unwrap_or_default();
        assert_eq!(
            text,
            format!(
                r#"{{"build":"b","totalBytes":3,"files":[{{"path":"a/b.pak","size":3,"sha256":"{}"}}]}}"#,
                "f".repeat(64)
            )
        );
    }
}
