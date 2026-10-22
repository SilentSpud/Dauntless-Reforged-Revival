//! Updating the server (`legacy/ReforgedHost/src/selfupdate.ts`): from a `git` fast-forward of the
//! checkout to downloading and checking the server release (RD8, RH5), which
//! `.github/workflows/server-release.yml` builds.
//!
//! A release is two files per platform on a GitHub release `server-v<version>`: the archive
//! (`ReforgedRevivalServer-<version>-<platform>.zip` or `.tar.gz`) and `SHA256SUMS.txt` for the
//! archives. The archive holds one folder, `reforged-revival-server-<version>/`: the commit's source
//! with `bin/` (the Rust binaries), the built Node services and `release.json`, which lists every
//! file with its SHA-256. Installing copies that folder over the server's folder:
//! - nothing git ignores is touched (`.workspace/`: config, secrets, the database; the SDK, game
//!   data and builds the host made in `paradox/` and `native/`)
//! - a file of the release the host changed stops the update (as `git merge --ff-only` did), and
//!   files an older release had and this one has not are removed
//! - `release.json` is written last: a server whose update stopped half way installs it again
//!
//! Where the release comes from: GitHub's releases of [`DEFAULT_REPOSITORY`] (or the one the
//! installed release names), or `REFORGED_SERVER_RELEASE` (a folder or an `http(s)://` address
//! holding `SHA256SUMS.txt` and the archives: a dry run's artifact, a mirror, the tests).

use std::cmp::Ordering;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::HostError;

pub const RELEASE_FILE: &str = "release.json";
pub const SUMS_FILE: &str = "SHA256SUMS.txt";
pub const SOURCE_ENV: &str = "REFORGED_SERVER_RELEASE";
pub const TAG_PREFIX: &str = "server-v";
pub const ARCHIVE_PREFIX: &str = "ReforgedRevivalServer-";
/// Where the releases are published; the release workflow builds this in from the repository it runs in.
pub const DEFAULT_REPOSITORY: &str = match option_env!("REFORGED_RELEASE_REPOSITORY") {
    Some(repository) => repository,
    None => "SilentSpud/Dauntless-Reforged-Revival",
};

/// The build outputs a release replaces as a whole on a server that never had a release (a Node-kit
/// checkout built them with npm): whatever is in them now is not the release's.
const RELEASE_OWNED_DIRS: [&str; 8] = [
    "paradox/backend/build",
    "paradox/backend/node_modules",
    "paradox/director/dist",
    "paradox/director/node_modules",
    "legacy/ReforgedGateway/dist",
    "legacy/ReforgedGateway/node_modules",
    "legacy/ReforgedContent/dist",
    "legacy/ReforgedContent/node_modules",
];

/// This PC's platform in the release's file names; `None` where there is no release.
pub fn platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("windows-x64"),
        ("linux", "x86_64") => Some("linux-x64"),
        _ => None,
    }
}

pub fn this_platform() -> Result<&'static str, HostError> {
    platform().ok_or_else(|| {
        HostError::message(format!(
            "there is no server release for {} {}: the servers are built for Windows x64 and Linux x86_64",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    })
}

pub fn archive_name(version: &str, platform: &str) -> String {
    let ext = if platform.starts_with("windows") {
        "zip"
    } else {
        "tar.gz"
    };
    format!("{ARCHIVE_PREFIX}{version}-{platform}.{ext}")
}

/// The version in an archive's name for `platform`, if it is one.
pub fn archive_version<'a>(name: &'a str, platform: &str) -> Option<&'a str> {
    let rest = name.strip_prefix(ARCHIVE_PREFIX)?;
    let ext = if platform.starts_with("windows") {
        ".zip"
    } else {
        ".tar.gz"
    };
    let version = rest.strip_suffix(ext)?.strip_suffix(platform)?.strip_suffix('-')?;
    Version::parse(version).map(|_| version)
}

// ------------------------------------------------------------------ problems

/// What can stop an update, with the code the launcher's Host page reads (`{"error": code}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// No release for this platform where we looked.
    NoRelease(String),
    /// A download that did not finish, or an answer that was not the file.
    DownloadFailed(String),
    /// `SHA256SUMS.txt` does not name the archive.
    NotInSums(String),
    /// The archive is not the one `SHA256SUMS.txt` names.
    Checksum {
        file: String,
        expected: String,
        actual: String,
    },
    /// `gh attestation verify` refused the archive.
    Attestation(String),
    /// The archive does not hold one release folder whose files match its `release.json`.
    BadArchive(String),
    /// Files of the release were changed on this PC.
    LocalChanges(Vec<String>),
}

impl Problem {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoRelease(_) => "no_release",
            Self::DownloadFailed(_) => "download_failed",
            Self::NotInSums(_) => "not_in_sums",
            Self::Checksum { .. } => "checksum",
            Self::Attestation(_) => "attestation",
            Self::BadArchive(_) => "bad_archive",
            Self::LocalChanges(_) => "local_changes",
        }
    }

    pub fn detail(&self) -> Option<String> {
        match self {
            Self::NoRelease(d) | Self::DownloadFailed(d) | Self::Attestation(d) | Self::BadArchive(d) => {
                Some(d.clone())
            }
            Self::NotInSums(file) => Some(file.clone()),
            Self::Checksum { file, .. } => Some(file.clone()),
            Self::LocalChanges(files) => Some(files.iter().take(5).cloned().collect::<Vec<_>>().join(", ")),
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::NoRelease(d) => format!("found no server release to install ({d})"),
            Self::DownloadFailed(d) => format!("the download failed: {d}"),
            Self::NotInSums(file) => format!("{SUMS_FILE} of the release does not list {file}: nothing was installed"),
            Self::Checksum { file, expected, actual } => format!(
                "{file} is not the file the release lists (SHA-256 {actual}, {SUMS_FILE} says {expected}): it was deleted and nothing was installed; try again"
            ),
            Self::Attestation(d) => format!(
                "the archive's build provenance did not verify (gh attestation verify): {d}. Nothing was installed"
            ),
            Self::BadArchive(d) => format!("the release archive is damaged or not a server release: {d}"),
            Self::LocalChanges(files) => format!(
                "files of the project were changed on this PC and the update would overwrite them: {}{}. Put them back (or move your changes elsewhere) and update again",
                files.iter().take(5).cloned().collect::<Vec<_>>().join(", "),
                if files.len() > 5 {
                    format!(" and {} more", files.len() - 5)
                } else {
                    String::new()
                }
            ),
        }
    }
}

/// An update's error: a [`Problem`] the launcher can name, or another error.
#[derive(Debug)]
pub enum UpdateError {
    Problem(Problem),
    Host(HostError),
}

impl From<HostError> for UpdateError {
    fn from(e: HostError) -> Self {
        Self::Host(e)
    }
}

impl From<Problem> for UpdateError {
    fn from(p: Problem) -> Self {
        Self::Problem(p)
    }
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Problem(p) => f.write_str(&p.message()),
            Self::Host(e) => write!(f, "{e}"),
        }
    }
}

impl From<UpdateError> for HostError {
    fn from(e: UpdateError) -> Self {
        match e {
            UpdateError::Host(e) => e,
            UpdateError::Problem(p) => HostError::message(p.message()),
        }
    }
}

pub type UpdateResult<T> = Result<T, UpdateError>;

// ------------------------------------------------------------------ versions

/// A semantic version without build metadata: `x.y.z` or `x.y.z-prerelease`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub core: [u64; 3],
    pub pre: Vec<String>,
}

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut numbers = [0u64; 3];
        let parts: Vec<&str> = core.split('.').collect();
        if parts.len() != 3 {
            return None;
        }
        for (slot, part) in numbers.iter_mut().zip(&parts) {
            if part.is_empty() || (part.len() > 1 && part.starts_with('0')) || !part.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            *slot = part.parse().ok()?;
        }
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => {
                let ids: Vec<String> = pre.split('.').map(str::to_owned).collect();
                if ids
                    .iter()
                    .any(|id| id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
                {
                    return None;
                }
                ids
            }
        };
        Some(Self { core: numbers, pre })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core
            .cmp(&other.core)
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    for (a, b) in self.pre.iter().zip(&other.pre) {
                        let order = match (a.parse::<u64>(), b.parse::<u64>()) {
                            (Ok(a), Ok(b)) => a.cmp(&b),
                            (Ok(_), Err(_)) => Ordering::Less,
                            (Err(_), Ok(_)) => Ordering::Greater,
                            (Err(_), Err(_)) => a.cmp(b),
                        };
                        if order != Ordering::Equal {
                            return order;
                        }
                    }
                    self.pre.len().cmp(&other.pre.len())
                }
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Which of the releases offered to install: the newest stable one, or the newest of all when this
/// server runs a prerelease (the canary) or there is no stable release yet (before the cutover).
pub fn pick_newest<'a>(versions: &[&'a str], installed: Option<&str>) -> Option<&'a str> {
    let installed_pre = installed.and_then(Version::parse).is_some_and(|v| v.is_prerelease());
    let parsed: Vec<(&str, Version)> = versions
        .iter()
        .filter_map(|v| Version::parse(v).map(|p| (*v, p)))
        .collect();
    let newest = |stable_only: bool| {
        parsed
            .iter()
            .filter(|(_, v)| !stable_only || !v.is_prerelease())
            .max_by(|a, b| a.1.cmp(&b.1))
            .map(|(text, _)| *text)
    };
    if installed_pre {
        newest(false)
    } else {
        newest(true).or_else(|| newest(false))
    }
}

// ------------------------------------------------------------------ release.json

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// `release.json`: what the release is and every file in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    pub commit: String,
    pub target: String,
    pub platform: String,
    pub repository: Option<String>,
    pub files: Vec<ManifestFile>,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
        let s = |name: &str| v.get(name).and_then(Value::as_str).map(str::to_owned);
        if v.get("name").and_then(Value::as_str) != Some("reforged-revival-server") {
            return Err("not a Dauntless Reforged Revival server release".into());
        }
        let version = s("version").ok_or("no version")?;
        Version::parse(&version).ok_or_else(|| format!("{version} is not a version"))?;
        let mut files = Vec::new();
        for f in v.get("files").and_then(Value::as_array).ok_or("no files")? {
            let path = f.get("path").and_then(Value::as_str).ok_or("a file without a path")?;
            let sha256 = f
                .get("sha256")
                .and_then(Value::as_str)
                .ok_or("a file without a SHA-256")?;
            if !safe_relative(path) {
                return Err(format!("a file outside the release: {path}"));
            }
            if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!("{path}: {sha256} is not a SHA-256"));
            }
            files.push(ManifestFile {
                path: path.to_owned(),
                sha256: sha256.to_ascii_lowercase(),
                size: f.get("size").and_then(Value::as_u64).unwrap_or(0),
            });
        }
        Ok(Self {
            version,
            commit: s("commit").unwrap_or_default(),
            target: s("target").unwrap_or_default(),
            platform: s("platform").unwrap_or_default(),
            repository: s("repository").filter(|r| !r.is_empty()),
            files,
        })
    }

    pub fn file(&self, path: &str) -> Option<&ManifestFile> {
        self.files.iter().find(|f| f.path == path)
    }

    pub fn short_commit(&self) -> &str {
        self.commit.get(..12).unwrap_or(&self.commit)
    }
}

/// A path inside the release: relative, `/`-separated, no `..`.
fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains(':'))
}

fn join_relative(root: &Path, path: &str) -> PathBuf {
    path.split('/').fold(root.to_path_buf(), |p, part| p.join(part))
}

/// The installed release's `release.json`; `None` on a server that has none (a git checkout).
pub fn read_manifest(root: &Path) -> Result<Option<Manifest>, HostError> {
    let file = root.join(RELEASE_FILE);
    match fs::read_to_string(&file) {
        Ok(text) => Manifest::parse(&text)
            .map(Some)
            .map_err(|e| HostError::message(format!("{} is damaged: {e}", file.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(HostError::io(&file, e)),
    }
}

// ------------------------------------------------------------------ SHA-256

pub fn sha256_file(file: &Path) -> Result<String, HostError> {
    let mut f = fs::File::open(file).map_err(|e| HostError::io(file, e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).map_err(|e| HostError::io(file, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `SHA256SUMS.txt` (`sha256sum` output): `(hash, file name)` per line.
pub fn parse_sums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            let (hash, name) = line.split_once(' ')?;
            let name = name.trim_start_matches(' ').trim_start_matches('*');
            (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) && !name.is_empty())
                .then(|| (hash.to_ascii_lowercase(), name.to_owned()))
        })
        .collect()
}

// ------------------------------------------------------------------ where the release is

/// Where a file of the release is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    File(PathBuf),
    Url(String),
}

/// Where releases are looked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// GitHub's releases `server-v*` of `owner/name`.
    GitHub(String),
    /// A folder with `SHA256SUMS.txt` and the archives.
    Folder(PathBuf),
    /// An `http(s)://` address with `SHA256SUMS.txt` and the archives.
    Url(String),
}

impl Source {
    /// `REFORGED_SERVER_RELEASE` when set; otherwise GitHub, from the repository the installed release
    /// was built in.
    pub fn from_env(installed: Option<&Manifest>) -> Self {
        match std::env::var(SOURCE_ENV) {
            Ok(value) if !value.trim().is_empty() => Self::parse(value.trim()),
            _ => Self::GitHub(
                installed
                    .and_then(|m| m.repository.clone())
                    .unwrap_or_else(|| DEFAULT_REPOSITORY.to_owned()),
            ),
        }
    }

    pub fn parse(value: &str) -> Self {
        if value.starts_with("http://") || value.starts_with("https://") {
            Self::Url(value.trim_end_matches('/').to_owned())
        } else if let Some(repo) = value.strip_prefix("github:") {
            Self::GitHub(repo.to_owned())
        } else {
            Self::Folder(PathBuf::from(value))
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::GitHub(repo) => format!("https://github.com/{repo}/releases"),
            Self::Folder(dir) => dir.display().to_string(),
            Self::Url(url) => url.clone(),
        }
    }
}

/// One release for this platform, ready to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub version: String,
    pub archive: String,
    pub archive_at: Location,
    pub sums_at: Location,
    /// The GitHub repository it was published in: its attestation can be checked.
    pub repository: Option<String>,
}

/// Which release [`find_release`] looks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wanted<'a> {
    /// The newest, as [`pick_newest`] picks it for a server on this version (`None`: no release yet).
    Newest(Option<&'a str>),
    /// This version (setup putting back the installed release).
    Exactly(&'a str),
}

impl Wanted<'_> {
    fn pick<'v>(self, versions: &[&'v str]) -> Option<&'v str> {
        match self {
            Self::Newest(installed) => pick_newest(versions, installed),
            Self::Exactly(version) => versions.iter().copied().find(|v| *v == version),
        }
    }
}

/// The release for `platform` at `source`.
pub fn find_release(source: &Source, platform: &str, wanted: Wanted<'_>) -> UpdateResult<Candidate> {
    match source {
        Source::GitHub(repo) => {
            let url = format!("https://api.github.com/repos/{repo}/releases?per_page=100");
            let body = http::get_text(&url, true).map_err(Problem::DownloadFailed)?;
            let releases: Value = serde_json::from_str(&body)
                .map_err(|e| Problem::DownloadFailed(format!("GitHub's answer was not JSON: {e}")))?;
            candidates_from_github(&releases, repo, platform, wanted)
        }
        Source::Folder(dir) => {
            let sums = dir.join(SUMS_FILE);
            let text = fs::read_to_string(&sums).map_err(|e| Problem::NoRelease(format!("{}: {e}", sums.display())))?;
            let lines = parse_sums(&text);
            let names: Vec<&str> = lines.iter().map(|(_, n)| n.as_str()).collect();
            let version = named(&names, platform, wanted).ok_or_else(|| {
                Problem::NoRelease(format!(
                    "no {} archive in {}",
                    describe(platform, wanted),
                    sums.display()
                ))
            })?;
            let archive = archive_name(&version, platform);
            Ok(Candidate {
                archive_at: Location::File(dir.join(&archive)),
                sums_at: Location::File(sums),
                archive,
                version,
                repository: None,
            })
        }
        Source::Url(base) => {
            let sums = format!("{base}/{SUMS_FILE}");
            let text = http::get_text(&sums, false).map_err(Problem::DownloadFailed)?;
            let lines = parse_sums(&text);
            let names: Vec<&str> = lines.iter().map(|(_, n)| n.as_str()).collect();
            let version = named(&names, platform, wanted)
                .ok_or_else(|| Problem::NoRelease(format!("no {} archive in {sums}", describe(platform, wanted))))?;
            let archive = archive_name(&version, platform);
            Ok(Candidate {
                archive_at: Location::Url(format!("{base}/{archive}")),
                sums_at: Location::Url(sums),
                archive,
                version,
                repository: None,
            })
        }
    }
}

fn named(names: &[&str], platform: &str, wanted: Wanted<'_>) -> Option<String> {
    let versions: Vec<&str> = names.iter().filter_map(|n| archive_version(n, platform)).collect();
    wanted.pick(&versions).map(str::to_owned)
}

fn describe(platform: &str, wanted: Wanted<'_>) -> String {
    match wanted {
        Wanted::Newest(_) => platform.to_owned(),
        Wanted::Exactly(version) => format!("{version} {platform}"),
    }
}

/// The newest of GitHub's releases (the API's list) with this platform's archive and `SHA256SUMS.txt`.
pub fn candidates_from_github(
    releases: &Value,
    repo: &str,
    platform: &str,
    wanted: Wanted<'_>,
) -> UpdateResult<Candidate> {
    let mut found: Vec<(String, String, String)> = Vec::new();
    for release in releases.as_array().into_iter().flatten() {
        if release.get("draft").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(version) = release
            .get("tag_name")
            .and_then(Value::as_str)
            .and_then(|t| t.strip_prefix(TAG_PREFIX))
            .filter(|v| Version::parse(v).is_some())
        else {
            continue;
        };
        let asset = |name: &str| {
            release
                .get("assets")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|a| a.get("name").and_then(Value::as_str) == Some(name))
                .and_then(|a| a.get("browser_download_url").and_then(Value::as_str))
                .map(str::to_owned)
        };
        if let (Some(archive), Some(sums)) = (asset(&archive_name(version, platform)), asset(SUMS_FILE)) {
            found.push((version.to_owned(), archive, sums));
        }
    }
    let versions: Vec<&str> = found.iter().map(|(v, _, _)| v.as_str()).collect();
    let newest = wanted
        .pick(&versions)
        .ok_or_else(|| {
            Problem::NoRelease(format!(
                "no server release for {} at https://github.com/{repo}/releases",
                describe(platform, wanted)
            ))
        })?
        .to_owned();
    let Some((version, archive_url, sums_url)) = found.into_iter().find(|(v, _, _)| *v == newest) else {
        return Err(Problem::NoRelease(newest).into());
    };
    Ok(Candidate {
        archive: archive_name(&version, platform),
        archive_at: Location::Url(archive_url),
        sums_at: Location::Url(sums_url),
        version,
        repository: Some(repo.to_owned()),
    })
}

// ------------------------------------------------------------------ download and check

/// Puts the release's `SHA256SUMS.txt` and archive in `dir` and checks the archive against it;
/// returns the archive. A file that does not check is deleted.
pub fn fetch(candidate: &Candidate, dir: &Path) -> UpdateResult<PathBuf> {
    fs::create_dir_all(dir).map_err(|e| HostError::io(dir, e))?;
    let sums_text = match &candidate.sums_at {
        Location::File(file) => {
            fs::read_to_string(file).map_err(|e| Problem::DownloadFailed(format!("{}: {e}", file.display())))?
        }
        Location::Url(url) => http::get_text(url, false).map_err(Problem::DownloadFailed)?,
    };
    let expected = parse_sums(&sums_text)
        .into_iter()
        .find(|(_, name)| *name == candidate.archive)
        .map(|(hash, _)| hash)
        .ok_or_else(|| Problem::NotInSums(candidate.archive.clone()))?;
    let target = dir.join(&candidate.archive);
    let part = dir.join(format!("{}.part", candidate.archive));
    let _ = fs::remove_file(&part);
    let copied = match &candidate.archive_at {
        Location::File(file) => fs::copy(file, &part)
            .map(|_| ())
            .map_err(|e| Problem::DownloadFailed(format!("{}: {e}", file.display()))),
        Location::Url(url) => http::download(url, &part).map_err(Problem::DownloadFailed),
    };
    if let Err(e) = copied {
        let _ = fs::remove_file(&part);
        return Err(e.into());
    }
    let actual = sha256_file(&part)?;
    if actual != expected {
        let _ = fs::remove_file(&part);
        return Err(Problem::Checksum {
            file: candidate.archive.clone(),
            expected,
            actual,
        }
        .into());
    }
    fs::rename(&part, &target).map_err(|e| HostError::io(&target, e))?;
    Ok(target)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attestation {
    Verified,
    /// Not checked, and why.
    Skipped(String),
}

/// Checks the archive's build provenance with the GitHub CLI where it can: a release from GitHub,
/// with `gh` installed and signed in. A refusal stops the update; no `gh` only skips the check.
pub fn verify_attestation(archive: &Path, candidate: &Candidate, log_file: &Path) -> UpdateResult<Attestation> {
    let Some(repo) = &candidate.repository else {
        return Ok(Attestation::Skipped(
            "not a GitHub release, so it has no attestation to check".into(),
        ));
    };
    let Some(gh) = crate::wine::find_on_path(&["gh"])? else {
        return Ok(Attestation::Skipped(
            "the GitHub CLI (gh) is not installed; SHA256SUMS.txt checked it".into(),
        ));
    };
    let cwd = archive.parent().map(Path::to_path_buf).unwrap_or_default();
    let options = crate::run::RunOptions {
        cwd,
        env: None,
        log_file: Some(log_file.to_path_buf()),
        quiet: true,
    };
    if crate::run::run(&gh, &["auth", "status"], &options)?.code != 0 {
        return Ok(Attestation::Skipped(
            "the GitHub CLI is not signed in (gh auth login); SHA256SUMS.txt checked it".into(),
        ));
    }
    let file = archive.to_string_lossy();
    let r = crate::run::run(&gh, &["attestation", "verify", &file, "--repo", repo], &options)?;
    if r.code != 0 {
        let _ = fs::remove_file(archive);
        return Err(Problem::Attestation(r.last_line.unwrap_or_else(|| format!("exit {}", r.code))).into());
    }
    Ok(Attestation::Verified)
}

// ------------------------------------------------------------------ unpacking

/// Unpacks the archive into `dest` (emptied first); returns the release folder in it.
pub fn extract(archive: &Path, dest: &Path) -> UpdateResult<PathBuf> {
    if dest.exists() {
        fs::remove_dir_all(dest).map_err(|e| HostError::io(dest, e))?;
    }
    fs::create_dir_all(dest).map_err(|e| HostError::io(dest, e))?;
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let bad = |e: String| UpdateError::Problem(Problem::BadArchive(format!("{name}: {e}")));
    let file = fs::File::open(archive).map_err(|e| HostError::io(archive, e))?;
    if name.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(file).map_err(|e| bad(e.to_string()))?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| bad(e.to_string()))?;
            let entry_name = String::from_utf8_lossy(entry.name_raw()).into_owned();
            let Some(relative) = entry.enclosed_name() else {
                return Err(bad(format!("{entry_name} is outside the archive's folder")));
            };
            let out = dest.join(relative);
            if entry.is_dir() {
                fs::create_dir_all(&out).map_err(|e| HostError::io(&out, e))?;
                continue;
            }
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent).map_err(|e| HostError::io(parent, e))?;
            }
            let mut w = fs::File::create(&out).map_err(|e| HostError::io(&out, e))?;
            std::io::copy(&mut entry, &mut w).map_err(|e| bad(format!("{entry_name}: {e}")))?;
            #[cfg(unix)]
            if let Some(mode) = entry.unix_mode() {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&out, fs::Permissions::from_mode(mode & 0o777));
            }
        }
    } else if name.ends_with(".tar.gz") {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
        tar.set_preserve_permissions(true);
        for entry in tar.entries().map_err(|e| bad(e.to_string()))? {
            let mut entry = entry.map_err(|e| bad(e.to_string()))?;
            let kind = entry.header().entry_type();
            if !(kind.is_file() || kind.is_dir()) {
                let path = entry.path().map(|p| p.display().to_string()).unwrap_or_default();
                return Err(bad(format!("{path} is not a file or a folder")));
            }
            // unpack_in refuses paths that leave dest
            if !entry.unpack_in(dest).map_err(|e| bad(e.to_string()))? {
                return Err(bad("a file outside the archive's folder".into()));
            }
        }
    } else {
        return Err(bad("neither a .zip nor a .tar.gz".into()));
    }
    let mut folders = Vec::new();
    for entry in fs::read_dir(dest).map_err(|e| HostError::io(dest, e))? {
        let entry = entry.map_err(|e| HostError::io(dest, e))?;
        folders.push(entry.path());
    }
    match folders.as_slice() {
        [one] if one.join(RELEASE_FILE).is_file() => Ok(one.clone()),
        _ => Err(bad(format!("it should hold one folder with {RELEASE_FILE}"))),
    }
}

/// The files under `root` that are missing or differ from `manifest`.
pub fn damaged_files(root: &Path, manifest: &Manifest) -> Result<Vec<String>, HostError> {
    let mut damaged = Vec::new();
    for f in &manifest.files {
        let file = join_relative(root, &f.path);
        if !file.is_file() || sha256_file(&file)? != f.sha256 {
            damaged.push(f.path.clone());
        }
    }
    Ok(damaged)
}

// ------------------------------------------------------------------ installing

/// What an install changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub from: Option<String>,
    pub to: String,
    /// Files written (new or changed).
    pub files: usize,
    pub removed: usize,
    /// The game runtime's source (`native/paradox-runtime`, `native/runtime-loader`) changed: the
    /// host rebuilds MysticParadox.dll and winmm.dll and publishes them again. `None`: not known
    /// (the server had no release before).
    pub runtime_changed: Option<bool>,
}

fn runtime_file(path: &str) -> bool {
    path.starts_with("native/paradox-runtime/") || path.starts_with("native/runtime-loader/")
}

/// Whether the game runtime's source differs between two releases.
pub fn runtime_changed(previous: &Manifest, next: &Manifest) -> bool {
    let side = |m: &Manifest| -> Vec<(String, String)> {
        let mut v: Vec<_> = m
            .files
            .iter()
            .filter(|f| runtime_file(&f.path))
            .map(|f| (f.path.clone(), f.sha256.clone()))
            .collect();
        v.sort();
        v
    };
    side(previous) != side(next)
}

/// Files of the release the host changed on this PC: the update would overwrite them. With an
/// installed release, a file whose content is neither the old release's nor the new one's. On a git
/// checkout with no release (a Node-kit host), what `git status` lists as changed and the release
/// writes differently; a `package-lock.json` npm rewrote does not count, as in the Node kit.
pub fn local_changes(root: &Path, previous: Option<&Manifest>, next: &Manifest) -> Result<Vec<String>, HostError> {
    let mut changed = Vec::new();
    match previous {
        Some(previous) => {
            for old in &previous.files {
                let file = join_relative(root, &old.path);
                if !file.is_file() {
                    continue;
                }
                let hash = sha256_file(&file)?;
                if hash != old.sha256 && next.file(&old.path).is_none_or(|n| n.sha256 != hash) {
                    changed.push(old.path.clone());
                }
            }
        }
        None if root.join(".git").exists() => {
            let Some(status) =
                crate::run::capture("git", &["status", "--porcelain", "--untracked-files=no"], Some(root))?
            else {
                return Ok(changed);
            };
            for line in status.lines() {
                let Some(path) = line
                    .get(3..)
                    .map(|p| p.rsplit(" -> ").next().unwrap_or(p).trim_matches('"'))
                else {
                    continue;
                };
                if path.ends_with("package-lock.json") {
                    continue;
                }
                let Some(new) = next.file(path) else { continue };
                let file = join_relative(root, path);
                if file.is_file() && sha256_file(&file)? != new.sha256 {
                    changed.push(path.to_owned());
                }
            }
        }
        None => {}
    }
    Ok(changed)
}

/// Writes `src` over `dst` through a temporary file in `dst`'s folder. A program that is running
/// (`reforged-host` itself, on Windows) cannot be replaced, but it can be renamed: it is moved aside
/// to `<name>.old-<n>` and removed by the next update.
fn replace_file(src: &Path, dst: &Path) -> Result<(), HostError> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|e| HostError::io(parent, e))?;
    }
    let tmp = PathBuf::from(format!("{}.reforged-new", dst.display()));
    fs::copy(src, &tmp).map_err(|e| HostError::io(&tmp, e))?;
    if fs::rename(&tmp, dst).is_ok() {
        return Ok(());
    }
    for n in 0..100 {
        let aside = PathBuf::from(format!("{}.old-{n}", dst.display()));
        if aside.exists() {
            continue;
        }
        if fs::rename(dst, &aside).is_ok() {
            break;
        }
    }
    let renamed = fs::rename(&tmp, dst);
    if let Err(e) = renamed {
        let _ = fs::remove_file(&tmp);
        return Err(HostError::message(format!(
            "could not replace {} ({e}): stop the server (and anything else using its files) and update again",
            dst.display()
        )));
    }
    Ok(())
}

/// Removes what an earlier update moved aside (`*.old-<n>` in `bin/`).
pub fn remove_moved_aside(root: &Path) {
    let Ok(entries) = fs::read_dir(root.join("bin")) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name
            .rsplit_once(".old-")
            .is_some_and(|(_, n)| n.bytes().all(|b| b.is_ascii_digit()))
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn remove_empty_parents(root: &Path, file: &Path) {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) || fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

/// The marker an install leaves until it is complete.
pub fn installing_marker(root: &Path) -> PathBuf {
    root.join(".workspace").join("update").join("installing")
}

/// Copies the release folder `staged` (unpacked by [`extract`]) over `root`. `previous` is the
/// installed release, if any. Every staged file is checked against the staged `release.json`
/// first; `release.json` is written last.
pub fn install(staged: &Path, root: &Path, previous: Option<&Manifest>) -> UpdateResult<(Manifest, Installed)> {
    let text =
        fs::read_to_string(staged.join(RELEASE_FILE)).map_err(|e| HostError::io(staged.join(RELEASE_FILE), e))?;
    let next = Manifest::parse(&text).map_err(|e| Problem::BadArchive(format!("{RELEASE_FILE}: {e}")))?;
    let damaged = damaged_files(staged, &next)?;
    if !damaged.is_empty() {
        return Err(Problem::BadArchive(format!(
            "{} file(s) do not match {RELEASE_FILE}, the first {}",
            damaged.len(),
            damaged[0]
        ))
        .into());
    }
    let changed = local_changes(root, previous, &next)?;
    if !changed.is_empty() {
        return Err(Problem::LocalChanges(changed).into());
    }

    let marker = installing_marker(root);
    if let Some(dir) = marker.parent() {
        fs::create_dir_all(dir).map_err(|e| HostError::io(dir, e))?;
    }
    fs::write(&marker, &next.version).map_err(|e| HostError::io(&marker, e))?;
    remove_moved_aside(root);

    if previous.is_none() {
        for dir in RELEASE_OWNED_DIRS {
            let target = join_relative(root, dir);
            if join_relative(staged, dir).is_dir() && target.exists() {
                fs::remove_dir_all(&target).map_err(|e| HostError::io(&target, e))?;
            }
        }
    }
    let mut written = 0;
    for f in next.files.iter().filter(|f| f.path != RELEASE_FILE) {
        let dst = join_relative(root, &f.path);
        if dst.is_file() && sha256_file(&dst)? == f.sha256 {
            continue;
        }
        replace_file(&join_relative(staged, &f.path), &dst)?;
        written += 1;
    }
    let mut removed = 0;
    if let Some(previous) = previous {
        for old in previous.files.iter().filter(|o| next.file(&o.path).is_none()) {
            let file = join_relative(root, &old.path);
            if fs::remove_file(&file).is_ok() {
                removed += 1;
                remove_empty_parents(root, &file);
            }
        }
    }
    replace_file(&staged.join(RELEASE_FILE), &root.join(RELEASE_FILE))?;
    let _ = fs::remove_file(&marker);
    let report = Installed {
        from: previous.map(|p| p.version.clone()),
        to: next.version.clone(),
        files: written,
        removed,
        runtime_changed: previous.map(|p| runtime_changed(p, &next)),
    };
    Ok((next, report))
}

// ------------------------------------------------------------------ HTTP

/// GETs over HTTP(S), redirects followed (GitHub's release files are redirects to its storage).
pub mod http {
    use std::io::Write;
    use std::path::Path;
    use std::sync::OnceLock;
    use std::time::Duration;

    use bytes::Bytes;
    use http_body_util::{BodyExt, Empty};
    use hyper::Request;
    use hyper_util::client::legacy::Client;
    use hyper_util::client::legacy::connect::HttpConnector;
    use hyper_util::rt::TokioExecutor;

    type Https = hyper_rustls::HttpsConnector<HttpConnector>;

    const USER_AGENT: &str = "reforged-host (Dauntless Reforged Revival host kit)";
    /// Waiting for an answer, or for the next part of one.
    const IDLE: Duration = Duration::from_secs(60);
    const MAX_TEXT: usize = 8 << 20;

    fn client() -> &'static Client<Https, Empty<Bytes>> {
        static CLIENT: OnceLock<Client<Https, Empty<Bytes>>> = OnceLock::new();
        CLIENT.get_or_init(|| {
            let builder = hyper_rustls::HttpsConnectorBuilder::new();
            let builder = match builder.with_provider_and_webpki_roots(rustls::crypto::aws_lc_rs::default_provider()) {
                Ok(builder) => builder,
                Err(_) => hyper_rustls::HttpsConnectorBuilder::new().with_webpki_roots(),
            };
            let connector = builder.https_or_http().enable_http1().build();
            Client::builder(TokioExecutor::new()).build(connector)
        })
    }

    fn block_on<T>(work: impl std::future::Future<Output = Result<T, String>>) -> Result<T, String> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?
            .block_on(work)
    }

    async fn open(url: &str, api: bool) -> Result<hyper::Response<hyper::body::Incoming>, String> {
        let mut url = url.to_owned();
        for _ in 0..6 {
            let mut request = Request::get(&url).header("user-agent", USER_AGENT);
            if api {
                request = request
                    .header("accept", "application/vnd.github+json")
                    .header("x-github-api-version", "2022-11-28");
            }
            let request = request.body(Empty::<Bytes>::new()).map_err(|e| format!("{url}: {e}"))?;
            let response = tokio::time::timeout(IDLE, client().request(request))
                .await
                .map_err(|_| format!("{url}: no answer in {} s", IDLE.as_secs()))?
                .map_err(|e| format!("{url}: {e}"))?;
            let status = response.status();
            if status.is_redirection() {
                let next = response
                    .headers()
                    .get("location")
                    .and_then(|l| l.to_str().ok())
                    .ok_or_else(|| format!("{url}: a redirect without a location"))?;
                let joined = url::Url::parse(&url)
                    .and_then(|base| base.join(next))
                    .map_err(|e| format!("{url}: a bad redirect ({e})"))?;
                // Never from HTTPS down to HTTP.
                if url.starts_with("https://") && joined.scheme() != "https" {
                    return Err(format!("{url}: a redirect away from HTTPS"));
                }
                url = joined.to_string();
                continue;
            }
            if !status.is_success() {
                return Err(format!("{url}: HTTP {}", status.as_u16()));
            }
            return Ok(response);
        }
        Err(format!("{url}: too many redirects"))
    }

    /// A text file (or GitHub's API, with `api`).
    pub fn get_text(url: &str, api: bool) -> Result<String, String> {
        block_on(async {
            let response = open(url, api).await?;
            let mut body = response.into_body();
            let mut out = Vec::new();
            while let Some(frame) = tokio::time::timeout(IDLE, body.frame())
                .await
                .map_err(|_| format!("{url}: the download stalled"))?
            {
                let frame = frame.map_err(|e| format!("{url}: the download was interrupted ({e})"))?;
                if let Some(data) = frame.data_ref() {
                    out.extend_from_slice(data);
                    if out.len() > MAX_TEXT {
                        return Err(format!("{url}: too large"));
                    }
                }
            }
            String::from_utf8(out).map_err(|_| format!("{url}: not text"))
        })
    }

    /// Streams `url` into `file`. An answer shorter than its `Content-Length`, or one that stops, is
    /// an interrupted download.
    pub fn download(url: &str, file: &Path) -> Result<(), String> {
        block_on(async {
            let response = open(url, false).await?;
            let expected: Option<u64> = response
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok());
            let mut out = std::fs::File::create(file).map_err(|e| format!("{}: {e}", file.display()))?;
            let mut body = response.into_body();
            let mut got: u64 = 0;
            while let Some(frame) = tokio::time::timeout(IDLE, body.frame())
                .await
                .map_err(|_| format!("{url}: the download stalled after {got} bytes"))?
            {
                let frame =
                    frame.map_err(|e| format!("{url}: the download was interrupted after {got} bytes ({e})"))?;
                if let Some(data) = frame.data_ref() {
                    out.write_all(data).map_err(|e| format!("{}: {e}", file.display()))?;
                    got += data.len() as u64;
                }
            }
            out.flush().map_err(|e| format!("{}: {e}", file.display()))?;
            if let Some(expected) = expected
                && got != expected
            {
                return Err(format!(
                    "{url}: the download was interrupted ({got} of {expected} bytes)"
                ));
            }
            Ok(())
        })
    }
}
