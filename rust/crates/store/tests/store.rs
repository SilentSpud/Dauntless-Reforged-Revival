#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

use std::time::Duration;

use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Text};
use diesel_migrations::{EmbeddedMigrations, embed_migrations};
use store::{Error, Options, Schema, Store};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!("tests/migrations");
const SCHEMA: Schema = Schema {
    migrations: || MIGRATIONS,
    drizzle: None,
};

#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type = BigInt)]
    n: i64,
}

#[derive(QueryableByName)]
struct Mode {
    #[diesel(sql_type = Text)]
    journal_mode: String,
}

fn open() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("test.db"), &Options::default(), &SCHEMA).unwrap();
    (dir, store)
}

fn count(conn: &mut store::Connection) -> Result<i64, Error> {
    Ok(diesel::sql_query("SELECT count(*) AS n FROM items")
        .get_result::<Count>(conn)?
        .n)
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_commit_and_errors_roll_back() {
    let (_dir, store) = open();
    store
        .write(|c| {
            c.batch_execute("INSERT INTO items (name) VALUES ('a')")
                .map_err(Error::from)
        })
        .await
        .unwrap();
    let failed: Result<(), Error> = store
        .write(|c| {
            c.batch_execute("INSERT INTO items (name) VALUES ('b')")?;
            Err(Error::Migration("refused".into()))
        })
        .await;
    assert!(failed.is_err());
    assert_eq!(store.read(count).await.unwrap(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_cannot_write() {
    let (_dir, store) = open();
    let refused = store
        .read(|c| {
            c.batch_execute("INSERT INTO items (name) VALUES ('a')")
                .map_err(Error::from)
        })
        .await;
    assert!(refused.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_database_is_in_wal_mode() {
    let (_dir, store) = open();
    let mode = store
        .read(|c| {
            Ok::<_, Error>(
                diesel::sql_query("PRAGMA journal_mode")
                    .get_result::<Mode>(c)?
                    .journal_mode,
            )
        })
        .await
        .unwrap();
    assert_eq!(mode, "wal");
    store.checkpoint().await.unwrap();
}

// SQLite is built like better-sqlite3's (.cargo/config.toml): these differ from SQLite's defaults.
#[tokio::test(flavor = "multi_thread")]
async fn sqlite_behaves_like_better_sqlite3() {
    let (_dir, store) = open();
    let double_quoted_string = store
        .read(|c| c.batch_execute("SELECT \"not a column\"").map_err(Error::from))
        .await;
    assert!(
        double_quoted_string.is_err(),
        "a double-quoted string is an identifier only"
    );
    let orphan = store
        .write(|c| {
            c.batch_execute("INSERT INTO tags (item, tag) VALUES (42, 'x')")
                .map_err(Error::from)
        })
        .await;
    assert!(orphan.is_err(), "foreign keys are enforced");
    store
        .read(|c| c.batch_execute("SELECT sqrt(4), ln(1)").map_err(Error::from))
        .await
        .unwrap();
}

// M16: a request dropped mid-transaction doesn't leave its transaction open.
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_request_still_finishes_its_transaction() {
    let (_dir, store) = open();
    let slow = store.write(|c| {
        c.batch_execute("INSERT INTO items (name) VALUES ('slow')")?;
        std::thread::sleep(Duration::from_millis(300));
        Ok::<_, Error>(())
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), slow).await.is_err(),
        "the request was dropped"
    );
    store
        .write(|c| {
            c.batch_execute("INSERT INTO items (name) VALUES ('next')")
                .map_err(Error::from)
        })
        .await
        .unwrap();
    assert_eq!(store.read(count).await.unwrap(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_panicking_closure_rolls_back_and_the_store_keeps_working() {
    let (_dir, store) = open();
    let panicked: Result<(), Error> = store
        .write(|c| {
            c.batch_execute("INSERT INTO items (name) VALUES ('lost')")?;
            panic!("handler bug");
        })
        .await;
    assert!(matches!(panicked, Err(Error::Panicked)));
    store
        .write(|c| {
            c.batch_execute("INSERT INTO items (name) VALUES ('kept')")
                .map_err(Error::from)
        })
        .await
        .unwrap();
    assert_eq!(store.read(count).await.unwrap(), 1);
}

#[test]
fn a_database_with_unknown_tables_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("other.db");
    SqliteConnection::establish(path.to_str().unwrap())
        .unwrap()
        .batch_execute("CREATE TABLE other (x)")
        .unwrap();
    assert!(matches!(
        Store::open(&path, &Options::default(), &SCHEMA),
        Err(Error::Adoption(_))
    ));
}

#[derive(QueryableByName)]
struct Text1 {
    #[diesel(sql_type = Text)]
    integrity_check: String,
}

// M17: a backup taken while saves keep coming finishes, is consistent, and the saves aren't blocked.
#[tokio::test(flavor = "multi_thread")]
async fn a_backup_under_constant_writes_is_one_consistent_snapshot() {
    let (dir, store) = open();
    // About 40 MB, so the copy takes several steps.
    store
        .write(|c| {
            c.batch_execute(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 10000)
                 INSERT INTO items (name) SELECT hex(randomblob(2000)) FROM n",
            )
            .map_err(Error::from)
        })
        .await
        .unwrap();

    let writer = store.clone();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_writer = stop.clone();
    let saves = tokio::spawn(async move {
        let mut saves = 0u32;
        while !stop_writer.load(std::sync::atomic::Ordering::Relaxed) {
            writer
                .write(|c| {
                    c.batch_execute("INSERT INTO items (name) VALUES ('save')")
                        .map_err(Error::from)
                })
                .await
                .unwrap();
            saves += 1;
        }
        saves
    });

    let destination = dir.path().join("backup.db");
    store.backup(&destination, Duration::from_secs(60)).await.unwrap();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let saves = saves.await.unwrap();
    assert!(saves > 0, "saves went on during the backup");

    let mut copy = SqliteConnection::establish(destination.to_str().unwrap()).unwrap();
    let check = diesel::sql_query("PRAGMA integrity_check")
        .get_result::<Text1>(&mut copy)
        .unwrap();
    assert_eq!(check.integrity_check, "ok");
    let copied = diesel::sql_query("SELECT count(*) AS n FROM items")
        .get_result::<Count>(&mut copy)
        .unwrap()
        .n;
    let live = store.read(count).await.unwrap();
    assert!((10_000..=live).contains(&copied), "copied {copied} of {live}");
    assert!(
        store.backup(&destination, Duration::from_secs(60)).await.is_err(),
        "never overwrites"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_connection_reads_but_refuses_writes() {
    let (dir, store) = open();
    store
        .write(|c| {
            c.batch_execute("INSERT INTO items (name) VALUES ('a')")
                .map_err(Error::from)
        })
        .await
        .unwrap();
    let mut conn = store::open_read_only(&dir.path().join("test.db"), Duration::from_secs(1)).unwrap();
    assert_eq!(count(&mut conn).unwrap(), 1);
    let refused = conn.batch_execute("PRAGMA query_only = OFF; INSERT INTO items (name) VALUES ('b')");
    assert!(refused.is_err(), "mode=ro holds even with query_only turned off");
    assert_eq!(store.read(count).await.unwrap(), 1);
    assert!(store::open_read_only(&dir.path().join("missing.db"), Duration::from_secs(1)).is_err());
    assert!(
        !dir.path().join("missing.db").exists(),
        "a read-only open never creates the file"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_schema_version_is_readable_while_a_write_is_held() {
    let (dir, store) = open();
    let path = dir.path().join("test.db");
    let version = store
        .write(move |_| {
            // Inside BEGIN IMMEDIATE: the write lock is taken, and the panel still reads its version.
            let mut conn = store::open_read_only(&path, Duration::from_millis(100))?;
            store::schema_version(&mut conn)
        })
        .await
        .unwrap();
    assert_eq!(version.as_deref(), Some("0001"));

    let blank = dir.path().join("blank.db");
    diesel::SqliteConnection::establish(&blank.to_string_lossy())
        .unwrap()
        .batch_execute("CREATE TABLE t (x)")
        .unwrap();
    let mut conn = store::open_read_only(&blank, Duration::from_secs(1)).unwrap();
    assert_eq!(store::schema_version(&mut conn).unwrap(), None);
}
