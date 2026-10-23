//! The frozen Drizzle migrations of a Node service, replayed the way Drizzle's SQLite migrator runs
//! them. Used to bring an old offline database or `.drsave` save up to the Drizzle state the Diesel
//! baseline adopts (D11, X1), and by tests that check the baseline against the Drizzle history.

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Nullable, Text};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{Connection, Error};

/// One migration from Drizzle's `meta/_journal.json` and its `.sql` file.
#[derive(Debug, Clone)]
pub struct Migration {
    pub tag: String,
    /// The journal's `when`: what Drizzle stores as `created_at` and compares to decide what to run.
    pub created_at: i64,
    pub sql: String,
}

#[derive(Deserialize)]
struct Journal {
    entries: Vec<JournalEntry>,
}

#[derive(Deserialize)]
struct JournalEntry {
    tag: String,
    when: i64,
}

/// Reads a Drizzle migrations folder (`meta/_journal.json` and one `<tag>.sql` per entry).
pub fn read_folder(dir: &std::path::Path) -> Result<Vec<Migration>, Error> {
    let read = |path: std::path::PathBuf| {
        std::fs::read_to_string(&path).map_err(|e| Error::Migration(format!("{}: {e}", path.display())))
    };
    let journal: Journal = serde_json::from_str(&read(dir.join("meta").join("_journal.json"))?)
        .map_err(|e| Error::Migration(format!("Drizzle journal: {e}")))?;
    journal
        .entries
        .into_iter()
        .map(|entry| {
            let sql = read(dir.join(format!("{}.sql", entry.tag)))?;
            Ok(Migration {
                tag: entry.tag,
                created_at: entry.when,
                sql,
            })
        })
        .collect()
}

/// SHA-256 of a migration file as Drizzle records it, in lower-case hex.
pub fn hash(sql: &str) -> String {
    Sha256::digest(sql.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether `recorded` is Drizzle's hash of `sql`. Drizzle hashes the file as it was checked out, so
/// a server whose checkout converted line endings recorded the hash of the CRLF text.
pub fn hash_matches(recorded: &str, sql: &str) -> bool {
    let lf = sql.replace("\r\n", "\n");
    recorded == hash(&lf) || recorded == hash(&lf.replace('\n', "\r\n"))
}

#[derive(QueryableByName)]
struct LastRow {
    #[diesel(sql_type = Nullable<BigInt>)]
    created_at: Option<i64>,
    #[diesel(sql_type = Text)]
    hash: String,
}

/// The newest row of `__drizzle_migrations`: its `created_at` and `hash`.
pub fn last_applied(conn: &mut Connection) -> Result<Option<(Option<i64>, String)>, Error> {
    let row = diesel::sql_query("SELECT created_at, hash FROM __drizzle_migrations ORDER BY created_at DESC LIMIT 1")
        .get_result::<LastRow>(conn)
        .optional()?;
    Ok(row.map(|row| (row.created_at, row.hash)))
}

/// Applies every migration newer than the database's last one, in one transaction, as Drizzle's
/// `migrate` does: each `--> statement-breakpoint` chunk is one statement, and each migration adds
/// a row of its hash and `created_at`.
pub fn replay(conn: &mut Connection, migrations: &[Migration]) -> Result<(), Error> {
    conn.batch_execute(
        "CREATE TABLE IF NOT EXISTS \"__drizzle_migrations\" (
            id SERIAL PRIMARY KEY,
            hash text NOT NULL,
            created_at numeric
        )",
    )?;
    let last = last_applied(conn)?.and_then(|(created_at, _)| created_at);
    conn.immediate_transaction(|conn| {
        for migration in migrations
            .iter()
            .filter(|m| last.is_none_or(|last| last < m.created_at))
        {
            for statement in migration.sql.split("--> statement-breakpoint") {
                if !statement.trim().is_empty() {
                    conn.batch_execute(statement)?;
                }
            }
            diesel::sql_query("INSERT INTO \"__drizzle_migrations\" (\"hash\", \"created_at\") VALUES (?, ?)")
                .bind::<Text, _>(hash(&migration.sql))
                .bind::<BigInt, _>(migration.created_at)
                .execute(conn)?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_match_lf_and_crlf_checkouts() {
        let sql = "CREATE TABLE a (x);\n--> statement-breakpoint\nCREATE TABLE b (y);\n";
        assert!(hash_matches(&hash(sql), sql));
        assert!(hash_matches(&hash(&sql.replace('\n', "\r\n")), sql));
        assert!(!hash_matches(&hash("something else"), sql));
    }
}
