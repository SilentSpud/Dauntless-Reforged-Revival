//! Read-only access from outside the services (MIGRATION_PLAN.md X8, D22): the private admin panel browses
//! the live database through [`open_read_only`] and checks [`schema_version`] before it starts. It never
//! takes the write lock, so it can't hold up a game save; every write goes through the metagame's admin API.

use std::path::Path;
use std::time::Duration;

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Text};

use crate::{Connection, Error};

/// Opens `path` with SQLite's `mode=ro`: writes fail with "attempt to write a readonly database", and a
/// missing file is refused instead of created. Safe while the service runs (WAL readers don't block it).
pub fn open_read_only(path: &Path, busy_timeout: Duration) -> Result<Connection, Error> {
    let uri =
        read_only_uri(path).map_err(|e| diesel::ConnectionError::BadConnection(format!("{}: {e}", path.display())))?;
    let mut conn = <Connection as diesel::Connection>::establish(&uri)?;
    conn.batch_execute(&format!(
        "PRAGMA busy_timeout = {}; PRAGMA query_only = ON;",
        busy_timeout.as_millis()
    ))?;
    Ok(conn)
}

#[derive(QueryableByName)]
struct Tables {
    #[diesel(sql_type = BigInt)]
    n: i64,
}

#[derive(QueryableByName)]
struct Version {
    #[diesel(sql_type = Text)]
    version: String,
}

/// The newest applied Diesel migration: the schema version the admin panel checks before it starts, and
/// regenerates its entities for when it changes. `None` for a database no Rust service has opened yet.
pub fn schema_version(conn: &mut Connection) -> Result<Option<String>, Error> {
    let recorded = diesel::sql_query(
        "SELECT count(*) AS n FROM sqlite_master WHERE type = 'table' AND name = '__diesel_schema_migrations'",
    )
    .get_result::<Tables>(conn)?;
    if recorded.n == 0 {
        return Ok(None);
    }
    Ok(
        diesel::sql_query("SELECT version FROM __diesel_schema_migrations ORDER BY version DESC LIMIT 1")
            .get_result::<Version>(conn)
            .optional()?
            .map(|row| row.version),
    )
}

/// `file:` URI for `path` with `mode=ro`; characters with a meaning in URIs are percent-encoded.
fn read_only_uri(path: &Path) -> std::io::Result<String> {
    let absolute = std::path::absolute(path)?;
    let text = absolute.to_string_lossy().replace('\\', "/");
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    // An absolute Unix path starts with '/'; a Windows one with its drive letter, which needs the extra '/'.
    let slashes = if encoded.starts_with('/') { "//" } else { "///" };
    Ok(format!("file:{slashes}{encoded}?mode=ro"))
}

#[cfg(test)]
mod tests {
    use super::read_only_uri;
    use std::path::Path;

    #[test]
    fn uri_characters_in_the_path_are_encoded() {
        let uri = read_only_uri(Path::new("/srv/a b/db?#%.db")).unwrap_or_default();
        assert!(uri.ends_with("/a%20b/db%3F%23%25.db?mode=ro"), "{uri}");
        assert!(uri.starts_with("file:///"), "{uri}");
    }
}
