//! Checking game files against the manifest (row L24): `src/main/verify.ts`. Size first (cheap), then
//! SHA-256, read in 1 MiB pieces on the blocking pool, two files at a time by default.

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::core::manifest::{ManifestFile, resolve_inside};

#[derive(Debug)]
pub enum VerifyError {
    Aborted,
    Io(std::io::Error),
}

impl From<std::io::Error> for VerifyError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Set to stop a running check (`AbortSignal`).
#[derive(Debug, Clone, Default)]
pub struct Abort(Arc<AtomicBool>);

impl Abort {
    pub fn abort(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn aborted(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// SHA-256 of a file, in lower-case hex; `on_bytes` hears every piece read.
pub async fn hash_file(
    file: PathBuf,
    abort: Abort,
    on_bytes: Arc<dyn Fn(u64) + Send + Sync>,
) -> Result<String, VerifyError> {
    tokio::task::spawn_blocking(move || {
        let mut reader = std::fs::File::open(&file)?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            if abort.aborted() {
                return Err(VerifyError::Aborted);
            }
            let n = reader.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
            on_bytes(n as u64);
        }
        Ok(hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
    })
    .await
    .unwrap_or(Err(VerifyError::Aborted))
}

/// A file's modification time in milliseconds since 1970, as Node's `mtimeMs`.
pub fn mtime_ms(metadata: &std::fs::Metadata) -> f64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0.0, |since| {
            since.as_secs() as f64 * 1000.0 + f64::from(since.subsec_nanos()) / 1_000_000.0
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileProblem {
    Missing,
    Size,
    Hash,
}

/// Files this launcher already hashed (same size and modification time) can skip the hash.
pub type Trusted = Arc<dyn Fn(&ManifestFile, u64, f64) -> bool + Send + Sync>;

pub async fn check_file(
    dir: &Path,
    file: &ManifestFile,
    full_hash: bool,
    abort: &Abort,
    on_bytes: Arc<dyn Fn(u64) + Send + Sync>,
    trusted: Option<&Trusted>,
) -> Result<Option<FileProblem>, VerifyError> {
    let Ok(full) = resolve_inside(dir, &file.path) else {
        return Ok(Some(FileProblem::Missing));
    };
    let metadata = match tokio::fs::metadata(&full).await {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return Ok(Some(FileProblem::Missing)),
    };
    if metadata.len() != file.size {
        return Ok(Some(FileProblem::Size));
    }
    if !full_hash || trusted.is_some_and(|trusted| trusted(file, metadata.len(), mtime_ms(&metadata))) {
        return Ok(None);
    }
    let digest = hash_file(full, abort.clone(), on_bytes).await?;
    Ok((digest != file.sha256).then_some(FileProblem::Hash))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyProgress {
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub files_total: usize,
    pub files_done: usize,
    pub current_file: Option<String>,
}

#[derive(Clone, Default)]
pub struct VerifyOptions {
    pub full_hash: bool,
    pub trusted: Option<Trusted>,
    pub abort: Abort,
    pub on_progress: Option<Arc<dyn Fn(VerifyProgress) + Send + Sync>>,
    /// 1 to 8, 2 when unset
    pub concurrency: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    /// Sorted by path
    pub bad: Vec<(ManifestFile, FileProblem)>,
    pub checked_files: usize,
}

struct Tally {
    done_bytes: u64,
    files_done: usize,
    last_report: Option<Instant>,
}

/// Checks every file. Without `full_hash` only presence and size are checked (a quick look before
/// playing); Repair hashes everything.
pub async fn verify_install(
    dir: &Path,
    files: &[ManifestFile],
    options: VerifyOptions,
) -> Result<VerifyReport, VerifyError> {
    let total_bytes = if options.full_hash {
        files.iter().map(|file| file.size).sum()
    } else {
        files.len() as u64
    };
    let files_total = files.len();
    let queue = Arc::new(Mutex::new(files.iter().cloned().collect::<VecDeque<_>>()));
    let bad = Arc::new(Mutex::new(Vec::new()));
    let tally = Arc::new(Mutex::new(Tally {
        done_bytes: 0,
        files_done: 0,
        last_report: None,
    }));
    let report = {
        let (tally, on_progress) = (tally.clone(), options.on_progress.clone());
        Arc::new(move |current: Option<&str>, force: bool| {
            let Some(on_progress) = &on_progress else { return };
            let progress = {
                let mut tally = tally.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let now = Instant::now();
                if !force
                    && tally
                        .last_report
                        .is_some_and(|last| now.duration_since(last) < Duration::from_millis(200))
                {
                    return;
                }
                tally.last_report = Some(now);
                VerifyProgress {
                    total_bytes,
                    done_bytes: tally.done_bytes,
                    files_total,
                    files_done: tally.files_done,
                    current_file: current.map(str::to_owned),
                }
            };
            on_progress(progress);
        })
    };

    let workers = options.concurrency.unwrap_or(2).clamp(1, 8);
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..workers {
        let (dir, queue, bad, tally, report, options) = (
            dir.to_path_buf(),
            queue.clone(),
            bad.clone(),
            tally.clone(),
            report.clone(),
            options.clone(),
        );
        tasks.spawn(async move {
            loop {
                if options.abort.aborted() {
                    return Err(VerifyError::Aborted);
                }
                let Some(file) = queue
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .pop_front()
                else {
                    return Ok(());
                };
                let counted = Arc::new(Mutex::new(0u64));
                let on_bytes: Arc<dyn Fn(u64) + Send + Sync> = {
                    let (counted, tally, report, path) =
                        (counted.clone(), tally.clone(), report.clone(), file.path.clone());
                    Arc::new(move |n| {
                        *counted.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) += n;
                        tally.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).done_bytes += n;
                        report(Some(&path), false);
                    })
                };
                let problem = check_file(
                    &dir,
                    &file,
                    options.full_hash,
                    &options.abort,
                    on_bytes,
                    options.trusted.as_ref(),
                )
                .await?;
                {
                    let mut tally = tally.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    // Files skipped by the size check, or trusted, count as done
                    let counted = *counted.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    tally.done_bytes += if options.full_hash {
                        file.size.saturating_sub(counted)
                    } else {
                        1
                    };
                    tally.files_done += 1;
                }
                if let Some(problem) = problem {
                    bad.lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push((file.clone(), problem));
                }
                report(Some(&file.path), false);
            }
        });
    }
    while let Some(result) = tasks.join_next().await {
        if let Ok(Err(error)) = result {
            options.abort.abort();
            tasks.shutdown().await;
            return Err(error);
        }
    }
    report(None, true);
    let mut bad = std::mem::take(&mut *bad.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
    bad.sort_by(|a, b| a.0.path.cmp(&b.0.path));
    let checked_files = tally.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).files_done;
    Ok(VerifyReport { bad, checked_files })
}

/// Removes leftover `.part` files for the given manifest files (after a finished install or repair).
pub async fn remove_part_files(dir: &Path, files: &[ManifestFile]) {
    for file in files {
        if let Ok(full) = resolve_inside(dir, &file.path) {
            let mut part = full.into_os_string();
            part.push(".part");
            let _ = tokio::fs::remove_file(part).await;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Seen {
    size: f64,
    mtime_ms: f64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheFile {
    dir: Option<String>,
    files: HashMap<String, Seen>,
}

/// Remembers which files were hashed (by size and modification time), so an interrupted install does
/// not re-hash gigabytes it already checked. Repair ignores it and hashes everything. The file is the
/// one the Electron launcher writes.
#[derive(Clone)]
pub struct VerifiedCache {
    file: PathBuf,
    dir: Option<String>,
    files: HashMap<String, Seen>,
    dirty: bool,
}

impl VerifiedCache {
    pub fn load(file: PathBuf) -> Self {
        let saved: Option<CacheFile> = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let saved = saved.filter(|saved| saved.dir.is_some()).unwrap_or_default();
        Self {
            file,
            dir: saved.dir,
            files: saved.files,
            dirty: false,
        }
    }

    fn use_dir(&mut self, dir: &str) {
        if self.dir.as_deref() != Some(dir) {
            self.dir = Some(dir.to_owned());
            self.files.clear();
            self.dirty = true;
        }
    }

    pub fn is_trusted(&self, dir: &str, file: &ManifestFile, size: u64, mtime_ms: f64) -> bool {
        self.dir.as_deref() == Some(dir)
            && self
                .files
                .get(&file.path)
                .is_some_and(|seen| seen.size == size as f64 && seen.mtime_ms == mtime_ms)
            && size == file.size
    }

    pub async fn record(&mut self, dir: &Path, file: &ManifestFile) {
        self.use_dir(&dir.to_string_lossy());
        let Ok(full) = resolve_inside(dir, &file.path) else {
            return;
        };
        if let Ok(metadata) = tokio::fs::metadata(full).await {
            self.files.insert(
                file.path.clone(),
                Seen {
                    size: metadata.len() as f64,
                    mtime_ms: mtime_ms(&metadata),
                },
            );
            self.dirty = true;
        }
    }

    pub fn forget(&mut self, dir: &str, file: &ManifestFile) {
        if self.dir.as_deref() == Some(dir) && self.files.remove(&file.path).is_some() {
            self.dirty = true;
        }
    }

    pub async fn save(&mut self) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.dirty = false;
        let data = serde_json::to_vec(&CacheFile {
            dir: self.dir.clone(),
            files: self.files.clone(),
        })
        .map_err(std::io::Error::other)?;
        if let Some(parent) = self.file.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut temporary = self.file.clone().into_os_string();
        temporary.push(".tmp");
        tokio::fs::write(&temporary, data).await?;
        tokio::fs::rename(&temporary, &self.file).await
    }
}
