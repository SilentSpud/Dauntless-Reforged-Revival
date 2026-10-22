//! What is known about each manifest file on the host's disk (`src/gamedir.ts`). The startup check
//! compares sizes only (a full hash of 10.9 GB takes a while; `content verify` does that). A file is
//! served only while it is ok, and at open time its size and modification time must still be what the
//! startup check saw.

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use crate::manifest::{Manifest, ManifestFile, on_disk_path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileState {
    Ok { size: u64, modified: Option<SystemTime> },
    Missing,
    NotAFile,
    SizeMismatch { actual: u64 },
    Changed,
}

impl FileState {
    pub fn status(&self) -> &'static str {
        match self {
            Self::Ok { .. } => "ok",
            Self::Missing => "missing",
            Self::NotAFile => "not_a_file",
            Self::SizeMismatch { .. } => "size_mismatch",
            Self::Changed => "changed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub states: HashMap<String, FileState>,
    pub ok: usize,
    pub missing: Vec<String>,
    pub mismatched: Vec<String>,
    pub bytes_ok: u64,
}

/// `CheckGameDir`.
pub fn check(game_dir: &Path, manifest: &Manifest) -> Report {
    let mut report = Report {
        states: HashMap::new(),
        ok: 0,
        missing: Vec::new(),
        mismatched: Vec::new(),
        bytes_ok: 0,
    };
    for file in &manifest.files {
        let state = stat_file(game_dir, file);
        match state {
            FileState::Ok { .. } => {
                report.ok += 1;
                report.bytes_ok += file.size;
            }
            FileState::Missing => report.missing.push(file.path.clone()),
            _ => report.mismatched.push(file.path.clone()),
        }
        report.states.insert(file.path.clone(), state);
    }
    report
}

fn stat_file(game_dir: &Path, file: &ManifestFile) -> FileState {
    // lstat: a symlink or junction planted in the game folder is not served
    let Ok(meta) = std::fs::symlink_metadata(on_disk_path(game_dir, file)) else {
        return FileState::Missing;
    };
    if !meta.is_file() {
        return FileState::NotAFile;
    }
    if meta.len() != file.size {
        return FileState::SizeMismatch { actual: meta.len() };
    }
    FileState::Ok {
        size: meta.len(),
        modified: meta.modified().ok(),
    }
}
