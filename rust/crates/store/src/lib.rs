//! SQLite for every service, through synchronous Diesel (MIGRATION_PLAN.md D10, M13, M16).
//!
//! A request's database work is one closure: [`Store::read`] or [`Store::write`]. The closure runs
//! on tokio's blocking pool with a pooled connection, so a request makes one thread hop however many
//! queries it runs, and it can't `.await` while it holds a transaction. A write closure runs in
//! `BEGIN IMMEDIATE` on the only write connection; a read closure runs in a deferred transaction on
//! one of the read connections, so all its queries see the same snapshot. A dropped request future
//! doesn't stop a closure: its transaction commits or rolls back by itself, as `better-sqlite3`'s do.
//!
//! [`Store::open`] switches the file to WAL (D17), then brings the schema up to date: existing
//! Drizzle databases are adopted (D11, [`adopt`]), new ones get every migration.

pub mod adopt;
pub mod backup;
pub mod drizzle;
pub mod readonly;
mod snapshot;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use diesel::connection::{Connection as _, SimpleConnection};
use diesel::prelude::*;
use diesel::r2d2::{ConnectionManager, CustomizeConnection, Pool, PooledConnection};
use diesel::sql_types::Text;

pub use adopt::{DrizzleBaseline, Schema};
pub use readonly::{open_read_only, schema_version};
pub use snapshot::schema_snapshot;

/// The connection type every service queries through. A Postgres port would make this a
/// `MultiConnection` enum (D21); nothing outside this crate names `SqliteConnection`.
pub type Connection = SqliteConnection;

type Manager = ConnectionManager<Connection>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database: {0}")]
    Query(#[from] diesel::result::Error),
    #[error("database connection: {0}")]
    Connection(#[from] diesel::ConnectionError),
    #[error("database pool: {0}")]
    Pool(#[from] diesel::r2d2::PoolError),
    #[error("migration: {0}")]
    Migration(String),
    #[error("the database can't be adopted: {0}")]
    Adoption(String),
    #[error("backup: {0}")]
    Backup(String),
    #[error("the database task panicked")]
    Panicked,
}

#[derive(Debug, Clone)]
pub struct Options {
    /// Connections for read closures; reads run in parallel with each other and with the writer.
    pub read_connections: u32,
    /// How long a statement waits for a lock before failing; better-sqlite3's default is 5 s.
    pub busy_timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            read_connections: 4,
            busy_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Debug)]
struct Pragmas {
    busy_timeout: Duration,
    read_only: bool,
}

impl CustomizeConnection<Connection, diesel::r2d2::Error> for Pragmas {
    fn on_acquire(&self, conn: &mut Connection) -> Result<(), diesel::r2d2::Error> {
        // busy_timeout first, so the statements after it already wait for locks.
        let mut sql = format!("PRAGMA busy_timeout = {};", self.busy_timeout.as_millis());
        sql += "PRAGMA foreign_keys = ON;";
        if self.read_only {
            sql += "PRAGMA query_only = ON;";
        }
        conn.batch_execute(&sql).map_err(diesel::r2d2::Error::QueryError)
    }
}

#[derive(QueryableByName)]
struct JournalMode {
    #[diesel(sql_type = Text)]
    journal_mode: String,
}

/// The connection pools of one database file. Cloning is cheap and shares the pools.
#[derive(Clone)]
pub struct Store {
    write: Pool<Manager>,
    read: Pool<Manager>,
    path: Arc<PathBuf>,
    busy_timeout: Duration,
}

impl Store {
    /// Opens (or creates) the database, switches it to WAL and brings its schema up to date.
    /// Blocking: call it at start, before serving.
    pub fn open(path: &Path, options: &Options, schema: &Schema) -> Result<Self, Error> {
        let url = path.to_string_lossy().into_owned();
        let pool = |size, read_only| {
            Pool::builder()
                .max_size(size)
                .min_idle(Some(0))
                .test_on_check_out(false)
                .connection_customizer(Box::new(Pragmas {
                    busy_timeout: options.busy_timeout,
                    read_only,
                }))
                .build(Manager::new(url.clone()))
        };

        let write = pool(1, false)?;
        {
            let mut conn = write.get()?;
            let mode = diesel::sql_query("PRAGMA journal_mode = WAL").get_result::<JournalMode>(&mut conn)?;
            if !mode.journal_mode.eq_ignore_ascii_case("wal") {
                return Err(Error::Migration(format!(
                    "the database stayed in journal mode {}",
                    mode.journal_mode
                )));
            }
            adopt::prepare(&mut conn, schema)?;
        }
        let read = pool(options.read_connections.max(1), true)?;
        Ok(Self {
            write,
            read,
            path: Arc::new(path.to_owned()),
            busy_timeout: options.busy_timeout,
        })
    }

    /// Runs `f` in a deferred transaction on a read connection. Writes fail (`query_only`).
    pub async fn read<T, E, F>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce(&mut Connection) -> Result<T, E> + Send + 'static,
        T: Send + 'static,
        E: From<Error> + From<diesel::result::Error> + Send + 'static,
    {
        let pool = self.read.clone();
        run_blocking(move || {
            let mut conn = checkout::<E>(&pool)?;
            (*conn).transaction(f)
        })
        .await
    }

    /// Runs `f` in a `BEGIN IMMEDIATE` transaction on the write connection. Returning `Err` rolls
    /// it back.
    pub async fn write<T, E, F>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce(&mut Connection) -> Result<T, E> + Send + 'static,
        T: Send + 'static,
        E: From<Error> + From<diesel::result::Error> + Send + 'static,
    {
        let pool = self.write.clone();
        run_blocking(move || {
            let mut conn = checkout::<E>(&pool)?;
            (*conn).immediate_transaction(f)
        })
        .await
    }

    /// Moves everything in the WAL into the database file and empties the WAL; called on a clean
    /// shutdown (D17), after the last write.
    pub async fn checkpoint(&self) -> Result<(), Error> {
        let pool = self.write.clone();
        run_blocking(move || {
            let mut conn = checkout::<Error>(&pool)?;
            conn.batch_execute("PRAGMA wal_checkpoint(TRUNCATE);")?;
            Ok(())
        })
        .await
    }
}

/// Brings one connection's database to the current schema as [`Store::open`] does (adopting a Drizzle
/// database at the baseline), without switching it to WAL: for a private copy, such as an offline save
/// being imported (X1), that must stay a single file.
pub fn migrate(conn: &mut Connection, schema: &Schema) -> Result<(), Error> {
    adopt::prepare(conn, schema)
}

fn checkout<E: From<Error>>(pool: &Pool<Manager>) -> Result<PooledConnection<Manager>, E> {
    pool.get().map_err(|e| E::from(Error::Pool(e)))
}

pub(crate) async fn run_blocking<T, E, F>(f: F) -> Result<T, E>
where
    F: FnOnce() -> Result<T, E> + Send + 'static,
    T: Send + 'static,
    E: From<Error> + Send + 'static,
{
    // A panic inside the closure leaves its transaction open on the connection; r2d2 drops a
    // connection whose transaction manager is broken, and closing it rolls the transaction back.
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|_| Err(E::from(Error::Panicked)))
}
