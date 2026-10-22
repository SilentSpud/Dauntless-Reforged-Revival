//! The server's game runtime on this PC (Reforged's `src/main/runtime-files.ts`): `MysticParadox.dll` and
//! `winmm.dll` next to the game. The server's content server lists the two files with their sizes and
//! SHA-256 (`manifest`); the launcher
//!   - asks the player once before it installs a server's runtime, and again whenever the set changes
//!     (`runtime_set_id`, kept in `trustedRuntimes[serverId]`),
//!   - downloads each file over the same pinned connection as everything else (`request_to_file`),
//!   - checks size and SHA-256 before a file replaces the old one, and again before every launch
//!     (`check_runtime`).
//!
//! A host can point the launcher at a local folder with a runtime they built themselves instead.
//! `runtime_set_id` must stay byte for byte Reforged's, so runtimes accepted in 1.x stay accepted.

pub mod manifest;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use protocol::manifest::{RUNTIME_FILE_NAMES, RUNTIME_MANIFEST_PATH, RuntimeFile, RuntimeManifest};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::http::to_file::{FileRequestError, FileRequestOptions, request_to_file};
use super::http::{Endpoint, HttpError, RequestOptions, parse_json_body, request};
use super::verify::{Abort, hash_file};
use crate::bridge::types::{RuntimeFileInfo, RuntimeState};
pub use manifest::parse_runtime_manifest;

/// Written next to the game after an install: what was installed and when (informational only).
pub const RUNTIME_MARKER_FILE: &str = ".reforged-runtime.json";
/// A download is written next to its target under this suffix, checked, then renamed over it.
pub const DOWNLOAD_SUFFIX: &str = ".rr-download";
/// An old file moved aside because a running process had it loaded: `<name>.rr-old-<ms>`.
pub const ASIDE_MARK: &str = ".rr-old-";
const DOWNLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const MANIFEST_TIMEOUT: Duration = Duration::from_secs(10);
const MANIFEST_MAX_BYTES: usize = 64 * 1024;

/// What the page shows of a manifest's files.
pub fn file_infos(files: &[RuntimeFile]) -> Vec<RuntimeFileInfo> {
    files
        .iter()
        .map(|file| RuntimeFileInfo {
            name: file.name.clone(),
            size: file.size,
            sha256: file.sha256.clone(),
        })
        .collect()
}

/// One id for a set of runtime files: SHA-256 hex of the lines `name:size:sha256`, sorted by name (as
/// JavaScript compares strings), joined by `\n`.
pub fn runtime_set_id(files: &[RuntimeFile]) -> String {
    let mut sorted: Vec<&RuntimeFile> = files.iter().collect();
    sorted.sort_by(|a, b| a.name.encode_utf16().cmp(b.name.encode_utf16()));
    let lines: Vec<String> = sorted
        .iter()
        .map(|file| format!("{}:{}:{}", file.name, file.size, file.sha256))
        .collect();
    Sha256::digest(lines.join("\n").as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Whether the player accepted this exact set for the server: `trusted` is `trustedRuntimes[serverId]`.
pub fn is_trusted(trusted: &[String], files: &[RuntimeFile]) -> bool {
    trusted.contains(&runtime_set_id(files))
}

/// The player accepted the set: adds its id to the server's list once, and answers it.
pub fn accept(trusted: &mut Vec<String>, files: &[RuntimeFile]) -> String {
    let id = runtime_set_id(files);
    if !trusted.contains(&id) {
        trusted.push(id.clone());
    }
    id
}

// ---------------------------------------------------------------- the manifest

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeManifestResult {
    Ok(RuntimeManifest),
    /// The server has no runtime to hand out yet (404 or 503)
    None,
    BadAnswer,
    CertMismatch,
    Unreachable,
}

/// `GET /content/v1/runtime/manifest`: 10 s, at most 64 KiB, no key header.
pub async fn fetch_runtime_manifest(endpoint: &Endpoint) -> RuntimeManifestResult {
    let response = match request(
        endpoint,
        RUNTIME_MANIFEST_PATH,
        RequestOptions::get(MANIFEST_TIMEOUT, MANIFEST_MAX_BYTES),
    )
    .await
    {
        Ok(response) => response,
        Err(HttpError::Pin { .. }) => return RuntimeManifestResult::CertMismatch,
        Err(_) => return RuntimeManifestResult::Unreachable,
    };
    match response.status {
        404 | 503 => RuntimeManifestResult::None,
        200 => parse_json_body(&response)
            .as_ref()
            .and_then(parse_runtime_manifest)
            .map_or(RuntimeManifestResult::BadAnswer, RuntimeManifestResult::Ok),
        _ => RuntimeManifestResult::BadAnswer,
    }
}

// ---------------------------------------------------------------- the files on disk

async fn sha256_of(file: &Path) -> io::Result<String> {
    hash_file(file.to_path_buf(), Abort::default(), Arc::new(|_| {}))
        .await
        .map_err(|error| match error {
            super::verify::VerifyError::Io(error) => error,
            super::verify::VerifyError::Aborted => io::Error::other("aborted"),
        })
}

/// The hashes of the runtime files in a folder, by name: None when a file is missing, not a file or
/// empty. Hashing two small DLLs is cheap, so it is done afresh every time.
pub async fn installed_runtime(dir: &Path) -> Vec<(&'static str, Option<String>)> {
    let mut out = Vec::with_capacity(RUNTIME_FILE_NAMES.len());
    for name in RUNTIME_FILE_NAMES {
        let file = dir.join(name);
        let hash = match tokio::fs::metadata(&file).await {
            Ok(metadata) if metadata.is_file() && metadata.len() > 0 => sha256_of(&file).await.ok(),
            _ => None,
        };
        out.push((name, hash));
    }
    out
}

/// Both runtime files present and non-empty.
pub async fn is_complete_runtime_dir(dir: &Path) -> bool {
    installed_runtime(dir).await.iter().all(|(_, hash)| hash.is_some())
}

/// The runtime next to the game against the server's list, and whether the files there may be used for
/// a launch as they are.
///  - `offered`: the server's list, or None when the server offers none (its host has not built one)
///  - `trusted`: whether the player accepted this exact list for this server ([`is_trusted`])
///  - `local_dir`: a local runtime folder used instead (playable when complete)
pub async fn check_runtime(
    win64_dir: &Path,
    offered: Option<&RuntimeManifest>,
    trusted: bool,
    local_dir: Option<&Path>,
) -> (RuntimeState, bool) {
    if let Some(local_dir) = local_dir {
        return (RuntimeState::Local, is_complete_runtime_dir(local_dir).await);
    }
    let installed = installed_runtime(win64_dir).await;
    let present = installed.iter().filter(|(_, hash)| hash.is_some()).count();
    let Some(offered) = offered else {
        // A runtime installed earlier still works: one build serves every server (the backend address
        // comes from the launcher at run time)
        return if present == RUNTIME_FILE_NAMES.len() {
            (RuntimeState::Ok, true)
        } else {
            (RuntimeState::Unavailable, false)
        };
    };
    let matches = offered.files.iter().all(|file| {
        installed
            .iter()
            .any(|(name, hash)| *name == file.name && hash.as_deref() == Some(file.sha256.as_str()))
    });
    if matches {
        (RuntimeState::Ok, true)
    } else if !trusted {
        (RuntimeState::Untrusted, false)
    } else if present == 0 {
        (RuntimeState::Missing, false)
    } else {
        (RuntimeState::Outdated, false)
    }
}

/// Clears the "downloaded from the internet" mark (the `Zone.Identifier` stream), like `Unblock-File`.
/// Linux has no alternate streams: nothing to do.
pub async fn unblock(file: &Path) {
    if cfg!(windows) {
        let mut stream = file.as_os_str().to_owned();
        stream.push(":Zone.Identifier");
        // No mark, or not NTFS
        let _ = tokio::fs::remove_file(stream).await;
    }
}

// ---------------------------------------------------------------- the install

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeErrorCode {
    /// 401 or 403: the server does not take the account key
    KeyRejected,
    /// 404: the server lists a file it does not serve
    NotFound,
    /// The file does not match the server's own list (or changed after it was copied)
    Different,
    DownloadFailed,
    /// The certificate is not the invite's
    CertMismatch,
    /// Disk full or access denied
    DiskError,
    /// Neither replaced nor moved aside: something holds the file without sharing it
    InUse,
    Cancelled,
    /// Anything else (Reforged's `runtime_failed`)
    Failed,
}

#[derive(Debug)]
pub struct RuntimeError {
    pub code: RuntimeErrorCode,
    pub file: Option<String>,
    pub message: String,
}

impl RuntimeError {
    fn new(code: RuntimeErrorCode, file: &str, message: impl Into<String>) -> Self {
        Self {
            code,
            file: Some(file.to_owned()),
            message: message.into(),
        }
    }

    fn io(error: &io::Error) -> Self {
        Self {
            code: if is_disk_error(error) {
                RuntimeErrorCode::DiskError
            } else {
                RuntimeErrorCode::Failed
            },
            file: None,
            message: error.to_string(),
        }
    }
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?} {} {}",
            self.code,
            self.file.as_deref().unwrap_or(""),
            self.message
        )
    }
}

impl std::error::Error for RuntimeError {}

/// `ENOSPC`, `EACCES`, `EPERM`
fn is_disk_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::StorageFull | io::ErrorKind::PermissionDenied | io::ErrorKind::QuotaExceeded
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeProgress {
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub files_done: usize,
    pub files_total: usize,
    pub current_file: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeInstallResult {
    /// Files whose old copy was loaded by a running process (on a host's PC: its game servers). The new
    /// file is in place; those processes use it once they restart.
    pub replaced_in_use: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replaced {
    Replaced,
    /// The old file had to be moved aside: something had it loaded
    InUse,
}

/// The file operations [`replace_file`] uses, so a test can act as Windows does with a loaded DLL.
pub trait ReplaceOps {
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    fn remove(&self, file: &Path) -> io::Result<()>;
}

pub struct DiskOps;

impl ReplaceOps for DiskOps {
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    fn remove(&self, file: &Path) -> io::Result<()> {
        std::fs::remove_file(file)
    }
}

fn with_suffix(file: &Path, suffix: &str) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Puts a checked file in place of the old one. Windows refuses to overwrite a DLL that a running process
/// has loaded, and on a host's own PC the Director's game servers (Ramsgate, the Training Grounds, hunts)
/// run the same Dauntless executable from the same folder, so they hold both runtime files for as long as
/// the server is up. Windows does let a loaded file be renamed: when the plain replace fails, the old file
/// is moved aside, the new one takes its name, and the old one is deleted once nothing has it loaded any
/// more (here, or by [`sweep_aside`] on a later install). If the second rename fails the old file is put
/// back, so the game still has a runtime.
pub fn replace_file(tmp: &Path, target: &Path, ops: &dyn ReplaceOps) -> io::Result<Replaced> {
    let Err(first) = ops.rename(tmp, target) else {
        return Ok(Replaced::Replaced);
    };
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let aside = with_suffix(target, &format!("{ASIDE_MARK}{millis}"));
    if ops.rename(target, &aside).is_err() {
        return Err(first);
    }
    if let Err(error) = ops.rename(tmp, target) {
        let _ = ops.rename(&aside, target);
        return Err(error);
    }
    let _ = ops.remove(&aside);
    Ok(Replaced::InUse)
}

/// Deletes old runtime files moved aside by an earlier install, the ones no process holds any more.
pub fn sweep_aside(dir: &Path) {
    let Ok(names) = std::fs::read_dir(dir) else { return };
    for entry in names.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if RUNTIME_FILE_NAMES
            .iter()
            .any(|runtime| name.starts_with(&format!("{runtime}{ASIDE_MARK}")))
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

pub struct InstallRequest<'a> {
    pub endpoint: &'a Endpoint,
    /// The account key's headers, as the host API sends them (`keyHeaders`)
    pub headers: Vec<(&'static str, String)>,
    pub manifest: &'a RuntimeManifest,
    pub win64_dir: &'a Path,
    pub cancel: Option<CancellationToken>,
    pub on_progress: Option<&'a (dyn Fn(RuntimeProgress) + Send + Sync)>,
}

fn download_error(error: FileRequestError, file: &str) -> RuntimeError {
    use RuntimeErrorCode as Code;
    match error {
        FileRequestError::Http(HttpError::Aborted) => RuntimeError::new(Code::Cancelled, file, "cancelled"),
        FileRequestError::Http(error @ HttpError::Pin { .. }) => {
            RuntimeError::new(Code::CertMismatch, file, error.to_string())
        }
        FileRequestError::Http(HttpError::TooLarge) => {
            RuntimeError::new(Code::Different, file, "larger than the server's own list says")
        }
        FileRequestError::Disk(error) if is_disk_error(&error) => {
            RuntimeError::new(Code::DiskError, file, error.to_string())
        }
        error => RuntimeError::new(Code::DownloadFailed, file, error.to_string()),
    }
}

/// Downloads and installs the server's runtime into the game's `Win64` folder. Each file is written next
/// to its target first, checked, and only then renamed over the old one. A file already right is skipped.
pub async fn install_runtime(request: InstallRequest<'_>) -> Result<RuntimeInstallResult, RuntimeError> {
    use RuntimeErrorCode as Code;
    let InstallRequest {
        endpoint,
        headers,
        manifest,
        win64_dir,
        cancel,
        on_progress,
    } = request;
    let total_bytes: u64 = manifest.files.iter().map(|file| file.size).sum();
    let files_total = manifest.files.len();
    let mut done_bytes = 0u64;
    let mut files_done = 0usize;
    let mut result = RuntimeInstallResult::default();
    tokio::fs::create_dir_all(win64_dir)
        .await
        .map_err(|error| RuntimeError::io(&error))?;
    sweep_aside(win64_dir);
    let report = |done_bytes: u64, files_done: usize, current: &str| {
        if let Some(on_progress) = on_progress {
            on_progress(RuntimeProgress {
                done_bytes,
                total_bytes,
                files_done,
                files_total,
                current_file: Some(current.to_owned()),
            });
        }
    };
    for file in &manifest.files {
        let name = file.name.as_str();
        // The manifest's checks already hold this; a name is never a path
        if !RUNTIME_FILE_NAMES.contains(&name) {
            return Err(RuntimeError::new(Code::Different, name, "not a runtime file"));
        }
        if cancel.as_ref().is_some_and(CancellationToken::is_cancelled) {
            return Err(RuntimeError::new(Code::Cancelled, name, "cancelled"));
        }
        let target = win64_dir.join(name);
        if sha256_of(&target).await.ok().as_deref() == Some(file.sha256.as_str()) {
            done_bytes += file.size;
            files_done += 1;
            report(done_bytes, files_done, name);
            continue;
        }
        let tmp = with_suffix(&target, DOWNLOAD_SUFFIX);
        let _ = tokio::fs::remove_file(&tmp).await;
        let before = done_bytes;
        let mut on_bytes = |n: u64| {
            done_bytes += n;
            report(done_bytes, files_done, name);
        };
        let path = format!(
            "/content/v1/runtime/files/{}",
            percent_encoding::utf8_percent_encode(name, URI_COMPONENT)
        );
        let status = request_to_file(
            endpoint,
            &path,
            &tmp,
            FileRequestOptions {
                headers: headers.clone(),
                idle_timeout: DOWNLOAD_IDLE_TIMEOUT,
                max_bytes: file.size,
                cancel: cancel.clone(),
                on_bytes: Some(&mut on_bytes),
            },
        )
        .await;
        let status = match status {
            Ok(status) => status,
            Err(error) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                return Err(download_error(error, name));
            }
        };
        if status != 200 {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(match status {
                401 | 403 => RuntimeError::new(Code::KeyRejected, name, format!("HTTP {status}")),
                404 => RuntimeError::new(Code::NotFound, name, "HTTP 404"),
                _ => RuntimeError::new(Code::DownloadFailed, name, format!("HTTP {status}")),
            });
        }
        let size = tokio::fs::metadata(&tmp).await.map(|metadata| metadata.len()).ok();
        let digest = sha256_of(&tmp).await.ok();
        if size != Some(file.size) || digest.as_deref() != Some(file.sha256.as_str()) {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(RuntimeError::new(
                Code::Different,
                name,
                "the file does not match the server's list",
            ));
        }
        match replace_file(&tmp, &target, &DiskOps) {
            Ok(Replaced::InUse) => result.replaced_in_use.push(name.to_owned()),
            Ok(Replaced::Replaced) => {}
            Err(error) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                return Err(RuntimeError::new(Code::InUse, name, error.to_string()));
            }
        }
        unblock(&target).await;
        if sha256_of(&target).await.ok().as_deref() != Some(file.sha256.as_str()) {
            return Err(RuntimeError::new(
                Code::Different,
                name,
                "changed after copying (antivirus?)",
            ));
        }
        done_bytes = before + file.size;
        files_done += 1;
        report(done_bytes, files_done, name);
    }
    write_marker(win64_dir, manifest).await;
    Ok(result)
}

/// `encodeURIComponent`: everything but `A-Z a-z 0-9 - _ . ! ~ * ' ( )` is escaped.
const URI_COMPONENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// Copies a local runtime (a folder with both files, built by the host) next to the game.
pub async fn install_local_runtime(local_dir: &Path, win64_dir: &Path) -> Result<RuntimeInstallResult, RuntimeError> {
    use RuntimeErrorCode as Code;
    let mut result = RuntimeInstallResult::default();
    sweep_aside(win64_dir);
    for name in RUNTIME_FILE_NAMES {
        let source = local_dir.join(name);
        let target = win64_dir.join(name);
        let source_hash = sha256_of(&source).await.map_err(|error| RuntimeError {
            file: Some(name.to_owned()),
            ..RuntimeError::io(&error)
        })?;
        if sha256_of(&target).await.ok().as_deref() == Some(source_hash.as_str()) {
            continue;
        }
        let tmp = with_suffix(&target, DOWNLOAD_SUFFIX);
        tokio::fs::copy(&source, &tmp).await.map_err(|error| RuntimeError {
            file: Some(name.to_owned()),
            ..RuntimeError::io(&error)
        })?;
        if sha256_of(&tmp).await.ok().as_deref() != Some(source_hash.as_str()) {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(RuntimeError::new(
                Code::Different,
                name,
                "changed while copying (antivirus?)",
            ));
        }
        match replace_file(&tmp, &target, &DiskOps) {
            Ok(Replaced::InUse) => result.replaced_in_use.push(name.to_owned()),
            Ok(Replaced::Replaced) => {}
            Err(error) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                return Err(RuntimeError::new(Code::InUse, name, error.to_string()));
            }
        }
        unblock(&target).await;
    }
    Ok(result)
}

async fn write_marker(win64_dir: &Path, manifest: &RuntimeManifest) {
    let marker = serde_json::json!({
        "note": "Written by the Dauntless Reforged Revival launcher: the game runtime installed next to the game.",
        "version": manifest.version,
        "files": manifest.files,
        "installedAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    });
    if let Ok(text) = serde_json::to_string_pretty(&marker) {
        // Informational only
        let _ = tokio::fs::write(win64_dir.join(RUNTIME_MARKER_FILE), text).await;
    }
}

#[cfg(test)]
mod tests;
