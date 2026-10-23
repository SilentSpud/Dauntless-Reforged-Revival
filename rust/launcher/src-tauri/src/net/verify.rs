//! SHA-256 of files on disk (row L24): `src/main/verify.ts`, read in 1 MiB pieces on the blocking pool.
//! DR's check of the 1.4.4 game against its manifest (`check_file`, `verify_install`,
//! `remove_part_files`, `VerifiedCache`) is not brought in (RD5). Reforged's `HashCache` (lines 39–93)
//! remembers the 1.14.7 executable's hash for the status display; PLAY hashes it afresh and checks it
//! against [`PINNED_EXE_SHA256`] (section 6).

pub mod cache_file;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

use sha2::{Digest, Sha256};

use crate::core::pins::PINNED_EXE_SHA256;
use cache_file::{CacheEntry, MAX_ENTRIES, parse_cache_file, write_cache_file};

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

/// Remembers a file's hash by its path, size and modification time, so the 120 MB game executable is
/// hashed once after it changes, not on every start. A file that changes (another build, a repair) has a
/// new size or time and is hashed again.
#[derive(Debug)]
pub struct HashCache {
    file: PathBuf,
    /// Oldest first
    entries: Vec<(String, CacheEntry)>,
    dirty: bool,
}

impl HashCache {
    /// The cache saved in `file` (`<userData>/file-hashes.json`), or an empty one.
    pub fn load(file: PathBuf) -> Self {
        let entries = std::fs::read_to_string(&file)
            .map(|text| parse_cache_file(&text))
            .unwrap_or_default();
        Self {
            file,
            entries,
            dirty: false,
        }
    }

    fn key(file: &Path) -> String {
        file.to_string_lossy().into_owned()
    }

    /// The remembered hash, or None when the file changed since (or was never hashed).
    pub fn cached(&self, file: &Path) -> Option<String> {
        let key = Self::key(file);
        let (_, entry) = self.entries.iter().find(|(path, _)| *path == key)?;
        let metadata = std::fs::metadata(file).ok()?;
        (metadata.len() == entry.size && mtime_ms(&metadata) == entry.mtime_ms).then(|| entry.sha256.clone())
    }

    /// Hashes the file and remembers the hash, unless the file changed while it was read.
    pub async fn hash(
        &mut self,
        file: &Path,
        abort: Abort,
        on_bytes: Arc<dyn Fn(u64) + Send + Sync>,
    ) -> Result<String, VerifyError> {
        let before = tokio::fs::metadata(file).await?;
        let digest = hash_file(file.to_path_buf(), abort, on_bytes).await?;
        let after = tokio::fs::metadata(file).await?;
        if after.len() == before.len() && mtime_ms(&after) == mtime_ms(&before) {
            let key = Self::key(file);
            self.entries.retain(|(path, _)| *path != key);
            self.entries.push((
                key,
                CacheEntry {
                    size: before.len(),
                    mtime_ms: mtime_ms(&before),
                    sha256: digest.clone(),
                },
            ));
            let extra = self.entries.len().saturating_sub(MAX_ENTRIES);
            self.entries.drain(..extra);
            self.dirty = true;
        }
        Ok(digest)
    }

    /// Writes the cache through a `.tmp` file and a rename, when anything changed.
    pub async fn save(&mut self) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.dirty = false;
        if let Some(parent) = self.file.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut tmp = self.file.clone().into_os_string();
        tmp.push(".tmp");
        tokio::fs::write(&tmp, write_cache_file(&self.entries)).await?;
        tokio::fs::rename(&tmp, &self.file).await
    }
}

/// The game executable against the 1.14.7 pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExeCheck {
    /// Dauntless 1.14.7
    Pinned,
    /// Another build
    OtherBuild,
    /// Not there, or not readable
    Missing,
}

impl ExeCheck {
    fn of(digest: &str, pin: &str) -> Self {
        if digest == pin { Self::Pinned } else { Self::OtherBuild }
    }
}

/// What the status display shows without reading the file: None when it has not been hashed since it
/// last changed. `pin` is [`PINNED_EXE_SHA256`] outside tests (Reforged's `exePin`).
pub fn cached_exe_check(cache: &HashCache, exe: &Path, pin: &str) -> Option<ExeCheck> {
    cache.cached(exe).map(|digest| ExeCheck::of(&digest, pin))
}

/// Hashes the executable afresh (PLAY and the "verify game" task) and checks it against the pin.
pub async fn check_game_exe(
    cache: &mut HashCache,
    exe: &Path,
    pin: &str,
    abort: Abort,
    on_bytes: Arc<dyn Fn(u64) + Send + Sync>,
) -> Result<ExeCheck, VerifyError> {
    match cache.hash(exe, abort, on_bytes).await {
        Ok(digest) => Ok(ExeCheck::of(&digest, pin)),
        Err(VerifyError::Io(_)) => Ok(ExeCheck::Missing),
        Err(error) => Err(error),
    }
}

/// The Dauntless 1.14.7 executable's hash.
pub const EXE_PIN: &str = PINNED_EXE_SHA256;

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> Arc<dyn Fn(u64) + Send + Sync> {
        Arc::new(|_| {})
    }

    /// SHA-256 of "abc"
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[tokio::test]
    async fn hash_file_is_sha256_hex_and_stops_when_aborted() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("abc");
        std::fs::write(&file, "abc").unwrap();
        assert_eq!(hash_file(file.clone(), Abort::default(), quiet()).await.unwrap(), ABC);
        let abort = Abort::default();
        abort.abort();
        assert!(matches!(
            hash_file(file, abort, quiet()).await,
            Err(VerifyError::Aborted)
        ));
    }

    #[tokio::test]
    async fn the_cache_serves_a_file_until_it_changes_and_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("Dauntless-Win64-Shipping.exe");
        let store = dir.path().join("data").join("file-hashes.json");
        std::fs::write(&exe, "abc").unwrap();
        let mut cache = HashCache::load(store.clone());
        assert_eq!(cache.cached(&exe), None, "never hashed");
        assert_eq!(cache.hash(&exe, Abort::default(), quiet()).await.unwrap(), ABC);
        assert_eq!(cache.cached(&exe).as_deref(), Some(ABC));
        cache.save().await.unwrap();
        assert!(!dir.path().join("data").join("file-hashes.json.tmp").exists());
        let reloaded = HashCache::load(store);
        assert_eq!(reloaded.cached(&exe).as_deref(), Some(ABC), "read back after a restart");
        std::fs::write(&exe, "abcd").unwrap();
        assert_eq!(reloaded.cached(&exe), None, "a changed file is hashed again");
    }

    #[tokio::test]
    async fn the_exe_is_checked_against_the_pin() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("Dauntless-Win64-Shipping.exe");
        let mut cache = HashCache::load(dir.path().join("file-hashes.json"));
        async fn fresh(cache: &mut HashCache, exe: &Path, pin: &str) -> ExeCheck {
            check_game_exe(cache, exe, pin, Abort::default(), quiet())
                .await
                .unwrap()
        }
        assert_eq!(fresh(&mut cache, &exe, EXE_PIN).await, ExeCheck::Missing);
        std::fs::write(&exe, "abc").unwrap();
        assert_eq!(fresh(&mut cache, &exe, EXE_PIN).await, ExeCheck::OtherBuild);
        assert_eq!(cached_exe_check(&cache, &exe, EXE_PIN), Some(ExeCheck::OtherBuild));
        assert_eq!(fresh(&mut cache, &exe, ABC).await, ExeCheck::Pinned);
        assert_eq!(
            EXE_PIN,
            "f3f95fbf22709ef077a11632ed95d675b7209b7e0172d9ecb9ba7d02a1fbb66b"
        );
    }
}
