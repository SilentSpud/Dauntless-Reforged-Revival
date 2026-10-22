//! Publishing the game runtime (`legacy/ReforgedHost/src/publish.ts`): the built `MysticParadox.dll`
//! and `winmm.dll` go to `.workspace/runtime/` with `runtime.json` (what the content server lists and
//! the launcher checks), and next to the server's own game executable (the Director's game servers
//! load the same runtime).
//!
//! RD23: this is the one place the version format and `runtime.json` are written; the launcher's
//! Host page calls `reforged-host publish-runtime`. Both are the Node kit's, byte for byte:
//! `<yyyymmdd>-<first 8 hex of the SHA-256 of the files' hashes joined by ":">` and
//! `JSON.stringify({ version, build: { changelist }, files: [{ name, size, sha256 }] }, null, 2)`
//! with a newline.
//!
//! `start` (RH2) uses [`ensure_server_flags`].

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::error::HostError;
use crate::gamepack::sha256_hex;
use crate::hostconfig::stringify_pretty;
use crate::paths::{GAME_CHANGELIST, Paths, RUNTIME_FILE_NAMES};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeFile {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedRuntime {
    pub version: String,
    pub changelist: u32,
    pub files: Vec<RuntimeFile>,
}

impl PublishedRuntime {
    /// `runtime.json`'s text.
    pub fn to_json(&self) -> String {
        let files: Vec<Value> = self
            .files
            .iter()
            .map(|f| json!({ "name": f.name, "size": f.size, "sha256": f.sha256 }))
            .collect();
        let value = json!({ "version": self.version, "build": { "changelist": self.changelist }, "files": files });
        stringify_pretty(&value) + "\n"
    }
}

/// `<yyyymmdd>-<id>`: the UTC date and the first 8 hex of the SHA-256 of the files' hashes.
pub fn runtime_version(now: DateTime<Utc>, hashes: &[&str]) -> String {
    let id = sha256_hex(hashes.join(":"));
    format!("{}-{}", now.format("%Y%m%d"), &id[..8])
}

pub fn sha256_file(file: &Path) -> Result<String, HostError> {
    std::fs::read(file).map(sha256_hex).map_err(|e| HostError::io(file, e))
}

fn suffixed(file: &Path, suffix: &str) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn copy_atomically(from: &Path, to: &Path) -> Result<(), HostError> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir).map_err(|e| HostError::io(dir, e))?;
    }
    let tmp = suffixed(to, ".tmp");
    std::fs::copy(from, &tmp).map_err(|e| HostError::io(from, e))?;
    replace_loaded_file(&tmp, to).map(drop)
}

/// Windows refuses to overwrite a DLL a running game server has loaded, but lets it be renamed: the
/// old file is moved aside, the new one takes its name, and the old one is deleted when it is free
/// (now, or on the next publish). Running game servers keep the old runtime until they restart.
pub const ASIDE_MARK: &str = ".rr-old-";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replaced {
    Replaced,
    InUse,
}

pub fn replace_loaded_file(tmp: &Path, target: &Path) -> Result<Replaced, HostError> {
    let Err(first) = std::fs::rename(tmp, target) else {
        return Ok(Replaced::Replaced);
    };
    let millis = Utc::now().timestamp_millis();
    let aside = suffixed(target, &format!("{ASIDE_MARK}{millis}"));
    if std::fs::rename(target, &aside).is_err() {
        let _ = std::fs::remove_file(tmp);
        return Err(HostError::io(target, first));
    }
    if let Err(e) = std::fs::rename(tmp, target) {
        // The old file stays aside when it cannot go back.
        let _ = std::fs::rename(&aside, target);
        let _ = std::fs::remove_file(tmp);
        return Err(HostError::io(target, e));
    }
    // Still loaded: swept later.
    let _ = std::fs::remove_file(&aside);
    Ok(Replaced::InUse)
}

fn sweep_aside(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if RUNTIME_FILE_NAMES
            .iter()
            .any(|r| name.starts_with(&format!("{r}{ASIDE_MARK}")))
        {
            // Still loaded: left for the next publish.
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The checkout's builds of the two runtime files.
pub fn default_sources(p: &Paths) -> [(&'static str, PathBuf); 2] {
    [
        ("MysticParadox.dll", p.runtime_dll.clone()),
        ("winmm.dll", p.loader_dll.clone()),
    ]
}

/// Publishes the runtime from `sources` (each runtime file's name and where it is).
pub fn publish_runtime(
    p: &Paths,
    now: DateTime<Utc>,
    sources: &[(&str, PathBuf)],
) -> Result<PublishedRuntime, HostError> {
    let source = |name: &str| {
        sources
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, file)| file.clone())
            .unwrap_or_default()
    };
    for name in RUNTIME_FILE_NAMES {
        let file = source(name);
        if !file.exists() {
            return Err(HostError::message(format!(
                "{name} is not built yet ({})",
                file.display()
            )));
        }
    }
    let dir = &p.runtime_publish;
    std::fs::create_dir_all(dir).map_err(|e| HostError::io(dir, e))?;
    let mut files = Vec::new();
    for name in RUNTIME_FILE_NAMES {
        let target = dir.join(name);
        copy_atomically(&source(name), &target)?;
        let size = std::fs::metadata(&target).map_err(|e| HostError::io(&target, e))?.len();
        files.push(RuntimeFile {
            name: name.to_owned(),
            size,
            sha256: sha256_file(&target)?,
        });
    }
    let hashes: Vec<&str> = files.iter().map(|f| f.sha256.as_str()).collect();
    let runtime = PublishedRuntime {
        version: runtime_version(now, &hashes),
        changelist: GAME_CHANGELIST,
        files,
    };
    // runtime.json last: the content server never sees a list whose files are not in place yet.
    let tmp = dir.join("runtime.json.tmp");
    std::fs::write(&tmp, runtime.to_json()).map_err(|e| HostError::io(&tmp, e))?;
    let json = dir.join("runtime.json");
    std::fs::rename(&tmp, &json).map_err(|e| HostError::io(&json, e))?;
    Ok(runtime)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstalledRuntime {
    pub installed: Vec<String>,
    pub unchanged: Vec<String>,
    pub in_use: Vec<String>,
    /// Name and error.
    pub failed: Vec<(String, String)>,
}

fn install_one(from: &Path, to: &Path, win64_dir: &Path) -> Result<Option<Replaced>, HostError> {
    if to.exists() && sha256_file(to)? == sha256_file(from)? {
        return Ok(None);
    }
    std::fs::create_dir_all(win64_dir).map_err(|e| HostError::io(win64_dir, e))?;
    let tmp = suffixed(to, ".tmp");
    std::fs::copy(from, &tmp).map_err(|e| HostError::io(from, e))?;
    replace_loaded_file(&tmp, to).map(Some)
}

/// Copies the published runtime next to the server's game executable. A file a running game server
/// has loaded is moved aside and still replaced (listed in `in_use`: those servers use it once they
/// restart); a file that can't be replaced at all is reported in `failed`.
pub fn install_server_runtime(p: &Paths, win64_dir: &Path) -> InstalledRuntime {
    let mut out = InstalledRuntime::default();
    sweep_aside(win64_dir);
    for name in RUNTIME_FILE_NAMES {
        let from = p.runtime_publish.join(name);
        let to = win64_dir.join(name);
        match install_one(&from, &to, win64_dir) {
            Ok(None) => out.unchanged.push(name.to_owned()),
            Ok(Some(replaced)) => {
                if replaced == Replaced::InUse {
                    out.in_use.push(name.to_owned());
                }
                out.installed.push(name.to_owned());
            }
            Err(e) => out.failed.push((name.to_owned(), e.to_string())),
        }
    }
    out
}

/// Runtime switches the game servers need, as empty files beside the game executable (the runtime
/// checks for them there). Mystic Paradox's runtime ships them off. They act on the server role only,
/// so the host's own game client, which runs from the same folder, is not affected.
///
/// `CALL_HUNTPASS_QUERY.flag`: a 1.14.7 hub never queries the progression configuration on its own,
/// so each player's progression component waits forever and the 60-second player-data timeout drops
/// them. With it the hub queries the current Hunt Pass season, which fetches the configuration
/// (`GET /progression/config`).
pub const SERVER_RUNTIME_FLAGS: [&str; 1] = ["CALL_HUNTPASS_QUERY.flag"];

/// Writes the missing flag files into the `Win64` folder; returns the ones it wrote.
pub fn ensure_server_flags(win64_dir: &Path) -> Result<Vec<String>, HostError> {
    let mut created = Vec::new();
    for name in SERVER_RUNTIME_FLAGS {
        let file = win64_dir.join(name);
        if file.exists() {
            continue;
        }
        std::fs::write(&file, "").map_err(|e| HostError::io(&file, e))?;
        created.push(name.to_owned());
    }
    Ok(created)
}
