//! The game runtime this server hands to players (`src/runtime.ts`): `MysticParadox.dll` and
//! `winmm.dll`, built by the host from their own SDK and copied into `CONTENT_RUNTIME_DIR` with a
//! `runtime.json` next to them (the host kit's `publish-runtime`, or the launcher's Host page):
//!
//! ```json
//! { "version": "20261014-1a2b3c4d", "build": { "changelist": 647472 },
//!   "files": [ { "name": "MysticParadox.dll", "size": 4718592, "sha256": "<64 hex>" },
//!              { "name": "winmm.dll", "size": 233472, "sha256": "<64 hex>" } ] }
//! ```
//!
//! The list is served only after every file on disk matched its size and SHA-256, so a player is
//! never offered a list the server cannot keep. The folder is checked again at most every 10 s, and a
//! new runtime.json (or a file that changed under it) is picked up without a restart. The launcher
//! checks every byte against the list again, and asks the player before installing it.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use protocol::manifest::{RUNTIME_FILE_NAMES, RuntimeFile, RuntimeManifest};
use serde_json::json;
use sha2::{Digest, Sha256};

pub use crate::input::{parse_runtime_index, parse_runtime_json};

use crate::auth::Clock;
use crate::log;

pub const MAX_INDEX_BYTES: u64 = 64 * 1024;
const RECHECK_MS: i64 = 10 * 1000;
const INDEX_FILE: &str = "runtime.json";

/// `RuntimeStatus`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeStatus {
    /// The checked list, its body as served and its ETag
    Ok {
        index: RuntimeManifest,
        body: String,
        etag: String,
    },
    /// Nothing published (404 `no_runtime`)
    None(String),
    /// Published but it does not check out (503 `runtime_invalid`)
    Invalid(String),
}

impl RuntimeStatus {
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Ok { .. } => None,
            Self::None(reason) | Self::Invalid(reason) => Some(reason),
        }
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn hash_file(file: &Path) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    let mut input = std::fs::File::open(file)?;
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The status of a folder, read and hashed now (`RuntimeStore.Load`).
pub fn load(dir: &Path) -> RuntimeStatus {
    let index_file = dir.join(INDEX_FILE);
    let bytes = match std::fs::metadata(&index_file) {
        Ok(meta) if !meta.is_file() || meta.len() > MAX_INDEX_BYTES => {
            return RuntimeStatus::Invalid("runtime.json is not a regular file under 64 KB".into());
        }
        Ok(_) => std::fs::read(&index_file),
        Err(error) => Err(error),
    };
    let Ok(bytes) = bytes else {
        return RuntimeStatus::None("no runtime.json yet (the host has not published a runtime)".into());
    };
    let index = match parse_runtime_json(&bytes) {
        Ok(index) => index,
        Err(reason) => return RuntimeStatus::Invalid(reason),
    };
    for file in &index.files {
        let full = dir.join(&file.name);
        let checked = std::fs::symlink_metadata(&full).and_then(|meta| {
            if !meta.is_file() {
                return Ok(Err(format!("{} is not a regular file", file.name)));
            }
            if meta.len() != file.size {
                return Ok(Err(format!(
                    "{} is {} bytes, runtime.json says {}",
                    file.name,
                    meta.len(),
                    file.size
                )));
            }
            Ok(if hash_file(&full)? == file.sha256 {
                Ok(())
            } else {
                Err(format!("{} does not match its sha256 in runtime.json", file.name))
            })
        });
        match checked {
            Ok(Ok(())) => {}
            Ok(Err(reason)) => return RuntimeStatus::Invalid(reason),
            Err(_) => return RuntimeStatus::Invalid(format!("{} is missing", file.name)),
        }
    }
    let body = index.to_json();
    let etag = format!("\"{}\"", sha256_hex(body.as_bytes()));
    RuntimeStatus::Ok { index, body, etag }
}

struct State {
    current: RuntimeStatus,
    signature: String,
    checked_at: Option<i64>,
}

/// `RuntimeStore`: the folder's status, checked again at most every 10 s, and loaded and hashed again
/// only when a file's size or modification time changed.
pub struct RuntimeStore {
    dir: Option<PathBuf>,
    now: Clock,
    state: Mutex<State>,
}

impl RuntimeStore {
    pub fn new(dir: Option<PathBuf>, now: Clock) -> Self {
        let reason = if dir.is_none() {
            "CONTENT_RUNTIME_DIR is not set"
        } else {
            "not checked yet"
        };
        Self {
            dir,
            now,
            state: Mutex::new(State {
                current: RuntimeStatus::None(reason.into()),
                signature: String::new(),
                checked_at: None,
            }),
        }
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// The status now. It may read and hash the folder: call it off the async threads.
    pub fn get(&self) -> RuntimeStatus {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        self.refresh(&mut state);
        state.current.clone()
    }

    /// A listed file and its path. The name must be exactly one in the checked list.
    pub fn file(&self, name: &str) -> Option<(RuntimeFile, PathBuf)> {
        let RuntimeStatus::Ok { index, .. } = self.get() else {
            return None;
        };
        let dir = self.dir.as_ref()?;
        let file = index.files.into_iter().find(|f| f.name == name)?;
        let full = dir.join(&file.name);
        Some((file, full))
    }

    /// Forget the last check (a file failed while being served).
    pub fn invalidate(&self) {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.checked_at = None;
        state.signature.clear();
    }

    fn signature_now(dir: &Path) -> String {
        std::iter::once(INDEX_FILE)
            .chain(RUNTIME_FILE_NAMES)
            .map(|name| match std::fs::metadata(dir.join(name)) {
                Ok(meta) => {
                    let modified = meta
                        .modified()
                        .ok()
                        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_nanos());
                    format!("{name}:{}:{modified}", meta.len())
                }
                Err(_) => format!("{name}:missing"),
            })
            .collect::<Vec<_>>()
            .join("|")
    }

    fn refresh(&self, state: &mut State) {
        let Some(dir) = &self.dir else {
            return;
        };
        let now = (self.now)();
        if state.checked_at.is_some_and(|at| now - at < RECHECK_MS) {
            return;
        }
        state.checked_at = Some(now);
        let signature = Self::signature_now(dir);
        if signature == state.signature {
            return;
        }
        state.signature = signature;
        let previous = std::mem::replace(&mut state.current, load(dir));
        match (&state.current, &previous) {
            (RuntimeStatus::Ok { index, .. }, RuntimeStatus::Ok { index: before, .. })
                if before.version == index.version => {}
            (RuntimeStatus::Ok { index, .. }, _) => log::info(
                "runtime ready",
                json!({"version": index.version, "files": index.files.iter().map(|f| json!({"name": f.name, "size": f.size, "sha256": f.sha256})).collect::<Vec<_>>()}),
            ),
            (current, _) if previous.reason().is_none() || previous.reason() != current.reason() => {
                let fields = json!({"dir": dir.display().to_string(), "reason": current.reason()});
                if matches!(current, RuntimeStatus::Invalid(_)) {
                    log::warn("no runtime is offered to players", fields);
                } else {
                    log::info("no runtime is offered to players", fields);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::manifest::DEFAULT_CHANGELIST;
    use serde_json::Value;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};

    fn good() -> Value {
        json!({"version": "1.0", "build": {"changelist": 647472}, "files": [
            {"name": "MysticParadox.dll", "size": 10, "sha256": "a".repeat(64)},
            {"name": "winmm.dll", "size": 5, "sha256": "b".repeat(64)},
        ]})
    }

    fn with(change: impl FnOnce(&mut Value)) -> Value {
        let mut value = good();
        change(&mut value);
        value
    }

    /// test/helpers.ts `Publish`: a runtime the way the host kit's publish-runtime writes it.
    pub fn publish(dir: &Path, seed: u8, version: &str) -> (Vec<u8>, Vec<u8>) {
        let dll = vec![seed; 5000 + seed as usize];
        let loader = vec![seed + 1; 3000 + seed as usize];
        std::fs::write(dir.join("MysticParadox.dll"), &dll).unwrap();
        std::fs::write(dir.join("winmm.dll"), &loader).unwrap();
        let index = json!({
            "version": version,
            "build": {"changelist": 647472},
            "files": [
                {"name": "winmm.dll", "size": loader.len(), "sha256": sha256_hex(&loader)},
                {"name": "MysticParadox.dll", "size": dll.len(), "sha256": sha256_hex(&dll)},
            ],
        });
        std::fs::write(dir.join("runtime.json"), index.to_string()).unwrap();
        (dll, loader)
    }

    // test/runtime.test.ts: ParseRuntimeIndex

    #[test]
    fn accepts_the_two_runtime_files_sorted_by_name() {
        let index = parse_runtime_index(&with(|v| {
            let files = v["files"].as_array_mut().unwrap();
            files.reverse();
        }))
        .unwrap();
        assert_eq!(
            index.files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["MysticParadox.dll", "winmm.dll"]
        );
        assert_eq!(index.build.name, "rel-1.14.7-Archon");
        assert_eq!(index.build.changelist, 647472);
    }

    #[test]
    fn refuses_anything_else() {
        let file0 = good()["files"][0].clone();
        let file1 = good()["files"][1].clone();
        let changed = |key: &str, value: Value| {
            let mut file = file0.clone();
            file[key] = value;
            with(|v| v["files"] = json!([file, file1.clone()]))
        };
        let bad = [
            Value::Null,
            json!([]),
            with(|v| v["version"] = json!("")),
            with(|v| v["version"] = json!("a b")),
            with(|v| v["files"] = json!("x")),
            with(|v| v["files"] = json!([file0.clone()])),
            with(|v| v["files"] = json!([file0.clone(), file1.clone(), file0.clone()])),
            changed("name", json!("..\\MysticParadox.dll")),
            changed("name", json!("dxgi.dll")),
            changed("sha256", json!("A".repeat(64))),
            changed("size", json!(0)),
            changed("size", json!(1e9)),
            with(|v| v["build"] = json!({"changelist": -1})),
        ];
        for raw in bad {
            assert!(parse_runtime_index(&raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn reads_as_javascript_does() {
        // Whole doubles are integers, a missing or non-object build takes the default, the version
        // is trimmed, a BOM is dropped, and the host's build name is never kept
        let index = parse_runtime_json(
            "\u{feff}{\"version\":\" v1 \",\"build\":[],\"files\":[{\"name\":\"winmm.dll\",\"size\":5.0,\"sha256\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\"},{\"name\":\"MysticParadox.dll\",\"size\":1e1,\"sha256\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}]}".as_bytes(),
        )
        .unwrap();
        assert_eq!(index.version, "v1");
        assert_eq!(index.build.changelist, DEFAULT_CHANGELIST);
        assert_eq!(index.files[0].size, 10);
        assert!(index.to_json().starts_with(r#"{"version":"v1","build":{"name":"rel-1.14.7-Archon","changelist":647472},"files":[{"name":"MysticParadox.dll","size":10,"#));
        for bad in [
            json!({"build": {"changelist": null}}),
            json!({"build": {"changelist": 1.5}}),
            json!({"build": {"changelist": "1"}}),
            json!({"build": {"changelist": 9_007_199_254_740_992u64}}),
        ] {
            let raw = with(|v| v["build"] = bad["build"].clone());
            assert!(parse_runtime_index(&raw).is_err(), "{raw}");
        }
        assert_eq!(
            parse_runtime_index(&with(|v| v["files"][0]["name"] = json!(null))).unwrap_err(),
            "unknown file null (only MysticParadox.dll and winmm.dll)"
        );
        assert_eq!(
            parse_runtime_index(&with(|v| v["files"][0] = json!(5))).unwrap_err(),
            "unknown file undefined (only MysticParadox.dll and winmm.dll)"
        );
    }

    #[test]
    fn the_body_is_the_one_node_served() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/protocol/tests/fixtures");
        for name in ["runtime-manifest", "runtime-manifest-defaults"] {
            let input = std::fs::read(fixtures.join(format!("{name}.input.json"))).unwrap();
            let body = std::fs::read_to_string(fixtures.join(format!("{name}.json"))).unwrap();
            assert_eq!(parse_runtime_json(&input).unwrap().to_json(), body, "{name}");
        }
    }

    // test/runtime.test.ts: RuntimeStore

    fn clock() -> (Clock, Arc<AtomicI64>) {
        let now = Arc::new(AtomicI64::new(0));
        let read = now.clone();
        (Arc::new(move || read.load(Ordering::SeqCst)), now)
    }

    #[test]
    fn offers_nothing_without_a_folder_or_before_the_host_publishes() {
        let (now, _) = clock();
        assert!(matches!(
            RuntimeStore::new(None, now.clone()).get(),
            RuntimeStatus::None(_)
        ));
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            RuntimeStore::new(Some(dir.path().into()), now).get(),
            RuntimeStatus::None(_)
        ));
    }

    #[test]
    fn offers_a_runtime_only_when_every_file_matches_and_picks_up_a_new_one() {
        let (now, time) = clock();
        let dir = tempfile::tempdir().unwrap();
        publish(dir.path(), 1, "v1");
        let store = RuntimeStore::new(Some(dir.path().into()), now);
        let RuntimeStatus::Ok { index, .. } = store.get() else {
            panic!("not ok")
        };
        assert_eq!(index.version, "v1");
        assert!(store.file("winmm.dll").is_some());
        assert!(store.file("WINMM.DLL").is_none());
        assert!(store.file("runtime.json").is_none());

        // A file that does not match its hash: nothing is offered
        std::fs::write(dir.path().join("winmm.dll"), vec![9u8; 3001]).unwrap();
        time.fetch_add(11_000, Ordering::SeqCst);
        let broken = store.get();
        assert!(
            matches!(&broken, RuntimeStatus::Invalid(reason) if reason.contains("winmm.dll")),
            "{broken:?}"
        );
        assert!(store.file("winmm.dll").is_none());

        publish(dir.path(), 2, "v2");
        time.fetch_add(11_000, Ordering::SeqCst);
        assert!(matches!(store.get(), RuntimeStatus::Ok { index, .. } if index.version == "v2"));
    }

    #[test]
    fn checks_again_only_after_ten_seconds_or_when_invalidated() {
        let (now, time) = clock();
        let dir = tempfile::tempdir().unwrap();
        let store = RuntimeStore::new(Some(dir.path().into()), now);
        assert!(matches!(store.get(), RuntimeStatus::None(_)));
        publish(dir.path(), 1, "v1");
        time.fetch_add(9_999, Ordering::SeqCst);
        assert!(matches!(store.get(), RuntimeStatus::None(_)), "cached");
        store.invalidate();
        assert!(matches!(store.get(), RuntimeStatus::Ok { .. }));
    }

    #[test]
    fn a_symlink_or_an_oversized_index_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        publish(dir.path(), 1, "v1");
        std::fs::write(dir.path().join("runtime.json"), vec![b' '; 64 * 1024 + 1]).unwrap();
        assert!(matches!(load(dir.path()), RuntimeStatus::Invalid(reason) if reason.contains("64 KB")));
        std::fs::create_dir(dir.path().join("x")).unwrap();
        publish(&dir.path().join("x"), 1, "v1");
        std::fs::remove_file(dir.path().join("x/winmm.dll")).unwrap();
        std::fs::create_dir(dir.path().join("x/winmm.dll")).unwrap();
        assert!(
            matches!(load(&dir.path().join("x")), RuntimeStatus::Invalid(reason) if reason == "winmm.dll is not a regular file")
        );
    }
}
