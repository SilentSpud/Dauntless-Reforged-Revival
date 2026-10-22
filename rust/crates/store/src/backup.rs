//! A live copy of the database while it is in use (MIGRATION_PLAN.md M17, D17).
//!
//! SQLite's online backup API, through a separate read-only `rusqlite` connection: Diesel has no
//! backup API. The Node metagame runs its backup on the writer's connection because a backup from
//! another connection starts over whenever someone else writes. Here the database is in WAL mode
//! and the backup connection holds one read transaction for the whole copy, so it copies one
//! consistent snapshot, saves go on meanwhile, and nothing starts over. The copy includes what is
//! still only in the WAL, which a copy of the `.db` file alone would miss.

use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::backup::{Backup, StepResult};
use rusqlite::{Connection, OpenFlags};

use crate::{Error, Store, run_blocking};

/// Pages copied per step; between steps the deadline is checked.
const PAGES_PER_STEP: std::os::raw::c_int = 4096;

impl Store {
    /// Copies the database to `destination`, which must not exist yet. Gives up (and removes the
    /// partial copy) when it takes longer than `deadline`; the Node kit allows 15 minutes.
    pub async fn backup(&self, destination: &Path, deadline: Duration) -> Result<(), Error> {
        let source = self.path.as_ref().clone();
        let destination = destination.to_owned();
        let busy_timeout = self.busy_timeout;
        run_blocking(move || backup_file(&source, &destination, busy_timeout, deadline)).await
    }
}

/// [`Store::backup`] for a database file this process hasn't opened (the `db backup` tool): blocking,
/// and safe while a service has the database open.
pub fn backup_file(source: &Path, destination: &Path, busy_timeout: Duration, deadline: Duration) -> Result<(), Error> {
    if !source.is_file() {
        return Err(Error::Backup(format!("no database at {}", source.display())));
    }
    if destination.exists() {
        return Err(Error::Backup(format!("{} exists already", destination.display())));
    }
    let result = copy(source, destination, busy_timeout, Instant::now() + deadline);
    if result.is_err() {
        // Best effort: a partial copy must not pass for a backup.
        let _ = std::fs::remove_file(destination);
    }
    result
}

fn copy(source: &Path, destination: &Path, busy_timeout: Duration, deadline: Instant) -> Result<(), Error> {
    let backup_error = |e: rusqlite::Error| Error::Backup(e.to_string());
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let src = Connection::open_with_flags(source, flags).map_err(backup_error)?;
    src.busy_timeout(busy_timeout).map_err(backup_error)?;
    // One snapshot for the whole copy: writers carry on in the WAL without restarting the backup. A
    // deferred BEGIN takes its snapshot at the first read, so read once now; otherwise every step
    // opens and closes its own read and the backup starts over after each save.
    src.execute_batch("BEGIN; SELECT count(*) FROM sqlite_schema;")
        .map_err(backup_error)?;
    let mut dst = Connection::open(destination).map_err(backup_error)?;
    {
        let backup = Backup::new(&src, &mut dst).map_err(backup_error)?;
        loop {
            match backup.step(PAGES_PER_STEP).map_err(backup_error)? {
                StepResult::Done => break,
                StepResult::More => {}
                // Busy or locked: wait a little before the next step.
                _ => std::thread::sleep(Duration::from_millis(5)),
            }
            if Instant::now() >= deadline {
                return Err(Error::Backup("the backup deadline passed".into()));
            }
        }
    }
    src.execute_batch("COMMIT").map_err(backup_error)?;
    // The copy is a plain single file: no WAL to carry along.
    dst.pragma_update(None, "journal_mode", "DELETE")
        .map_err(backup_error)?;
    Ok(())
}
