//! `file-hashes.json`, the hash cache's file (Reforged's `HashCache`, `src/main/verify.ts` 46–59 and 86–92):
//! `{"files": {"<absolute path>": {"size", "mtimeMs", "sha256"}}}`. Entries without a 64-hex hash or with
//! a size or time that is not a number are dropped on load. Only `serde_json`, so the fuzz target
//! compiles this file as it is.

use serde_json::{Map, Value, json};

/// At most this many files are remembered; the oldest goes first.
pub const MAX_ENTRIES: usize = 32;

#[derive(Debug, Clone, PartialEq)]
pub struct CacheEntry {
    pub size: u64,
    /// Node's `mtimeMs`
    pub mtime_ms: f64,
    pub sha256: String,
}

fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The entries in the file's order (oldest first), at most [`MAX_ENTRIES`] of the newest.
pub fn parse_cache_file(text: &str) -> Vec<(String, CacheEntry)> {
    let Ok(Value::Object(raw)) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let Some(Value::Object(files)) = raw.get("files") else {
        return Vec::new();
    };
    let mut entries: Vec<(String, CacheEntry)> = files
        .iter()
        .filter_map(|(path, entry)| {
            let size = entry.get("size")?.as_f64()?;
            let mtime_ms = entry.get("mtimeMs")?.as_f64()?;
            let sha256 = entry.get("sha256")?.as_str()?;
            // A size that is no file size never matches a file, so it is not kept
            let size = (size.fract() == 0.0 && (0.0..=u64::MAX as f64).contains(&size)).then_some(size as u64)?;
            is_sha256_hex(sha256).then(|| {
                (
                    path.clone(),
                    CacheEntry {
                        size,
                        mtime_ms,
                        sha256: sha256.to_owned(),
                    },
                )
            })
        })
        .collect();
    let extra = entries.len().saturating_sub(MAX_ENTRIES);
    entries.drain(..extra);
    entries
}

/// The file's text, as `JSON.stringify({ files: Object.fromEntries(entries) })`.
pub fn write_cache_file(entries: &[(String, CacheEntry)]) -> String {
    let mut files = Map::new();
    for (path, entry) in entries {
        files.insert(
            path.clone(),
            json!({ "size": entry.size, "mtimeMs": entry.mtime_ms, "sha256": entry.sha256 }),
        );
    }
    json!({ "files": files }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_entries_are_dropped_and_good_ones_read_back() {
        let hash = "c".repeat(64);
        let text = format!(
            r#"{{"files":{{"a":{{"size":5,"mtimeMs":1.5,"sha256":"{hash}"}},"b":{{"size":5,"mtimeMs":1,"sha256":"C{}"}},"c":{{"size":"5","mtimeMs":1,"sha256":"{hash}"}},"d":7}}}}"#,
            "c".repeat(63)
        );
        let entries = parse_cache_file(&text);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "a");
        assert_eq!(parse_cache_file(&write_cache_file(&entries)), entries);
        assert!(parse_cache_file("").is_empty());
        assert!(parse_cache_file("[]").is_empty());
        assert!(parse_cache_file(r#"{"files":[]}"#).is_empty());
    }

    #[test]
    fn at_most_32_and_the_oldest_go() {
        let entries: Vec<_> = (0..40)
            .map(|i| {
                (
                    format!("f{i}"),
                    CacheEntry {
                        size: i,
                        mtime_ms: 0.25,
                        sha256: "0".repeat(64),
                    },
                )
            })
            .collect();
        let read = parse_cache_file(&write_cache_file(&entries));
        assert_eq!(read.len(), MAX_ENTRIES);
        assert_eq!(read[0].0, "f8");
    }
}
