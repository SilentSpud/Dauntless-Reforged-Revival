//! Bringing a database to the current schema (MIGRATION_PLAN.md D11).
//!
//! - A database Diesel already manages gets its pending migrations.
//! - A new, empty database gets every migration, starting with the baseline.
//! - A database the Node service made is adopted, not migrated: its last Drizzle migration must be
//!   the one the baseline was cut from (same `created_at`, same file hash) and its schema must
//!   equal the baseline's. Then the baseline is recorded as applied without running it, and
//!   `__drizzle_migrations` stays as it is, so the Node service could still open the file until
//!   the first Diesel migration after the baseline.
//!
//! Anything else is refused: a database at an older Drizzle state is brought forward by running
//! the Node service once (or, for offline saves, by [`crate::drizzle::replay`] first).

use diesel::migration::MigrationSource;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Text};
use diesel::sqlite::Sqlite;
use diesel_migrations::{EmbeddedMigrations, MigrationHarness};

use crate::drizzle;
use crate::snapshot::schema_snapshot;
use crate::{Connection, Error};

/// The schema a service's database must have.
pub struct Schema {
    /// The service's Diesel migrations, the first one being the baseline: a function returning the
    /// `embed_migrations!` constant, because `EmbeddedMigrations` can't be copied.
    pub migrations: fn() -> EmbeddedMigrations,
    /// Present for services that ran on Drizzle before: what an adopted database must look like.
    pub drizzle: Option<DrizzleBaseline>,
}

/// The Drizzle state the baseline migration was cut from.
pub struct DrizzleBaseline {
    /// The `when` of the last Drizzle migration in the journal.
    pub created_at: i64,
    /// That migration's SQL file, to check the hash Drizzle recorded.
    pub sql: &'static str,
    /// [`schema_snapshot`] of a database at that state: the baseline's `up.sql`.
    pub snapshot: &'static str,
}

#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type = BigInt)]
    n: i64,
}

fn table_exists(conn: &mut Connection, name: &str) -> Result<bool, Error> {
    let count = diesel::sql_query("SELECT count(*) AS n FROM sqlite_schema WHERE type = 'table' AND name = ?")
        .bind::<Text, _>(name)
        .get_result::<Count>(conn)?;
    Ok(count.n > 0)
}

fn user_objects(conn: &mut Connection) -> Result<i64, Error> {
    let count = diesel::sql_query("SELECT count(*) AS n FROM sqlite_schema WHERE name NOT LIKE 'sqlite!_%' ESCAPE '!'")
        .get_result::<Count>(conn)?;
    Ok(count.n)
}

fn run_pending(conn: &mut Connection, schema: &Schema) -> Result<(), Error> {
    conn.run_pending_migrations((schema.migrations)())
        .map_err(|e| Error::Migration(e.to_string()))?;
    Ok(())
}

pub(crate) fn prepare(conn: &mut Connection, schema: &Schema) -> Result<(), Error> {
    if table_exists(conn, "__diesel_schema_migrations")? {
        return run_pending(conn, schema);
    }
    if table_exists(conn, "__drizzle_migrations")? {
        let baseline = schema.drizzle.as_ref().ok_or_else(|| {
            Error::Adoption("it has Drizzle migrations, and this service has no Drizzle baseline".into())
        })?;
        conn.immediate_transaction(|conn| adopt(conn, schema, baseline))?;
        return run_pending(conn, schema);
    }
    if user_objects(conn)? == 0 {
        return run_pending(conn, schema);
    }
    Err(Error::Adoption("it has tables but no migration history".into()))
}

fn adopt(conn: &mut Connection, schema: &Schema, baseline: &DrizzleBaseline) -> Result<(), Error> {
    let (created_at, hash) =
        drizzle::last_applied(conn)?.ok_or_else(|| Error::Adoption("__drizzle_migrations is empty".into()))?;
    if created_at != Some(baseline.created_at) {
        return Err(Error::Adoption(format!(
            "its last Drizzle migration is from {created_at:?}, not {}: start the Node service of the \
             latest release once to bring it up to date, or restore a newer backup",
            baseline.created_at
        )));
    }
    if !drizzle::hash_matches(&hash, baseline.sql) {
        return Err(Error::Adoption(
            "its last Drizzle migration has a different hash than the baseline's".into(),
        ));
    }
    // Compared with LF line endings: either side may come from a checkout that converted them.
    if schema_snapshot(conn)? != baseline.snapshot.replace("\r\n", "\n") {
        return Err(Error::Adoption("its schema differs from the baseline's".into()));
    }

    let first = MigrationSource::<Sqlite>::migrations(&(schema.migrations)())
        .map_err(|e| Error::Migration(e.to_string()))?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Migration("there is no baseline migration".into()))?;
    let version = first.name().version().to_string();
    // applied_migrations creates __diesel_schema_migrations the way Diesel does.
    conn.applied_migrations().map_err(|e| Error::Migration(e.to_string()))?;
    diesel::sql_query("INSERT INTO __diesel_schema_migrations (version) VALUES (?)")
        .bind::<Text, _>(version)
        .execute(conn)?;
    Ok(())
}
