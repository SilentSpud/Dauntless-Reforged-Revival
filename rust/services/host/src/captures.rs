//! The pinned store captures the stores are built from (`legacy/ReforgedHost/src/captures.ts`): Lady
//! Luck's rotation (`scripts/enrich_ladyluck_store.ts`) and the platinum store's prices, Hunt Pass
//! rank skips and passes (`scripts/generate_platinum_store.ts`), both from
//! `paradox/DauntlessEndpointDocumentation/Store/Product/Skus/`. Without them Lady Luck's Store is the
//! shipped example and the platinum store has no rank skips, no passes and no captured prices.
//!
//! They come from EisigesEis/DauntlessEndpointDocumentation, which carries no license, so they are
//! never copied into this repository: `paradox/backend/store-captures.json` pins the commit and each
//! file's SHA-256, and `gamedata` fetches that commit on the host's own machine (a shallow fetch into
//! `.workspace/captures/`) and writes the files where the generators look (git-ignored). A file whose
//! bytes do not match its pinned hash is refused.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::error::HostError;
use crate::gamepack::{sha256_hex, write_file};
use crate::paths::Paths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturePin {
    pub name: String,
    pub repository: String,
    pub commit: String,
    /// Path in the repository and its SHA-256, in the pin's order.
    pub files: Vec<(String, String)>,
}

fn is_hex(text: &str, len: usize) -> bool {
    text.len() == len && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `/^(https:\/\/|file:\/\/|\/|[A-Za-z]:\\)/`.
fn is_repository(text: &str) -> bool {
    let b = text.as_bytes();
    text.starts_with("https://")
        || text.starts_with("file://")
        || text.starts_with('/')
        || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\')
}

/// Node's `path.isAbsolute` on this platform.
fn is_absolute(name: &str) -> bool {
    if cfg!(windows) {
        let b = name.as_bytes();
        name.starts_with(['/', '\\'])
            || (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'/' || b[2] == b'\\'))
    } else {
        name.starts_with('/')
    }
}

pub fn read_capture_pin(file: &Path) -> Result<CapturePin, HostError> {
    let text = std::fs::read_to_string(file).map_err(|e| HostError::io(file, e))?;
    parse_capture_pin(file, &text)
}

/// Each `/`-separated part one plain name on this OS: a drive (`C:x` on Windows), a root or a `\`
/// separator would take the file out of the captures folder when joined. Stricter than the Node kit,
/// whose `path.join` kept such a part inside.
fn is_plain(name: &str) -> bool {
    name.split('/').all(|part| {
        let mut parts = Path::new(part).components();
        matches!(parts.next(), Some(std::path::Component::Normal(n)) if n == part) && parts.next().is_none()
    })
}

/// [`read_capture_pin`] on the file's text; `file` only names it in the errors.
pub fn parse_capture_pin(file: &Path, text: &str) -> Result<CapturePin, HostError> {
    let raw: Value = serde_json::from_str(text).map_err(|e| HostError::message(format!("{}: {e}", file.display())))?;
    let shown = file.display();
    let repository = raw
        .get("repository")
        .and_then(Value::as_str)
        .filter(|r| is_repository(r));
    let Some(repository) = repository else {
        return Err(HostError::message(format!("{shown}: repository must be an https URL")));
    };
    let Some(commit) = raw.get("commit").and_then(Value::as_str).filter(|c| is_hex(c, 40)) else {
        return Err(HostError::message(format!(
            "{shown}: commit must be a full 40-character commit id"
        )));
    };
    let mut files = Vec::new();
    if let Some(map) = raw.get("files").and_then(Value::as_object) {
        for (name, hash) in map {
            let hash = hash.as_str().filter(|h| is_hex(h, 64));
            let Some(hash) = hash.filter(|_| !name.contains("..") && !is_absolute(name) && is_plain(name)) else {
                return Err(HostError::message(format!(
                    "{shown}: files must map plain paths to SHA-256 hashes"
                )));
            };
            files.push((name.clone(), hash.to_owned()));
        }
    }
    if files.is_empty() {
        return Err(HostError::message(format!("{shown}: no files")));
    }
    Ok(CapturePin {
        name: raw
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("DauntlessEndpointDocumentation")
            .to_owned(),
        repository: repository.to_owned(),
        commit: commit.to_owned(),
        files,
    })
}

fn target(p: &Paths, name: &str) -> PathBuf {
    name.split('/')
        .fold(p.store_captures.clone(), |dir, part| dir.join(part))
}

/// The files that are not in place with their pinned bytes yet.
pub fn missing_captures(p: &Paths, pin: &CapturePin) -> Vec<String> {
    pin.files
        .iter()
        .filter(|(name, hash)| std::fs::read(target(p, name)).map_or(true, |bytes| sha256_hex(bytes) != *hash))
        .map(|(name, _)| name.clone())
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFetch {
    AlreadyReady,
    Fetched,
}

fn git_command(args: &[&str], cwd: &Path) -> Command {
    let mut command = Command::new("git");
    command.args(args).current_dir(cwd).stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// A git step, its output appended to the log.
fn git(args: &[&str], cwd: &Path, log_file: Option<&Path>) -> Result<(), HostError> {
    let first = args.first().copied().unwrap_or_default();
    let output = git_command(args, cwd)
        .output()
        .map_err(|e| HostError::message(format!("git {first} failed: {e}")))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if let Some(log) = log_file {
        // The log is for reading afterwards; a log that cannot be written does not stop the fetch.
        if let Some(dir) = log.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let line = format!("\n> git {}\n{text}", args.join(" "));
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
    }
    if output.status.success() {
        return Ok(());
    }
    let last = text.lines().map(str::trim).rfind(|l| !l.is_empty()).map(str::to_owned);
    let code = output.status.code().unwrap_or(-1);
    Err(HostError::message(format!(
        "git {first} failed: {}",
        last.unwrap_or_else(|| format!("exit {code}"))
    )))
}

/// Puts the pinned files in place, fetching the commit when one is missing or changed. The files are
/// read out of git's objects (`git show <commit>:<path>`), so the bytes are the committed ones
/// whatever the host's line-ending settings are.
pub fn ensure_captures(p: &Paths, pin: &CapturePin, log_file: Option<&Path>) -> Result<CaptureFetch, HostError> {
    if missing_captures(p, pin).is_empty() {
        return Ok(CaptureFetch::AlreadyReady);
    }
    let repo = p.workspace.join("captures").join(format!("{}.git", pin.name));
    if !repo.join("HEAD").exists() {
        std::fs::create_dir_all(&repo).map_err(|e| HostError::io(&repo, e))?;
        git(&["init", "--quiet", "--bare"], &repo, log_file)?;
    }
    git(
        &[
            "-c",
            "protocol.version=2",
            "fetch",
            "--quiet",
            "--depth",
            "1",
            &pin.repository,
            &pin.commit,
        ],
        &repo,
        log_file,
    )?;
    let short = &pin.commit[..12];
    let mut data = Vec::new();
    for (name, hash) in &pin.files {
        let object = format!("{}:{name}", pin.commit);
        let output = git_command(&["show", &object], &repo).stderr(Stdio::null()).output();
        let bytes = match output {
            Ok(out) if out.status.success() => out.stdout,
            _ => {
                return Err(HostError::message(format!("{} has no {name} at {short}", pin.name)));
            }
        };
        if sha256_hex(&bytes) != *hash {
            return Err(HostError::message(format!(
                "{}'s {name} does not match its pinned SHA-256",
                pin.name
            )));
        }
        data.push((name, bytes));
    }
    for (name, bytes) in data {
        write_file(&target(p, name), bytes)?;
    }
    Ok(CaptureFetch::Fetched)
}
