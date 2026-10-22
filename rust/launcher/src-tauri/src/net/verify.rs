//! SHA-256 of files on disk (row L24): `src/main/verify.ts`, read in 1 MiB pieces on the blocking pool.
//! DR's check of the 1.4.4 game against its manifest (`check_file`, `verify_install`,
//! `remove_part_files`, `VerifiedCache`) is not brought in (RD5); RA7 adds Reforged's `HashCache`.

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

use sha2::{Digest, Sha256};

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
