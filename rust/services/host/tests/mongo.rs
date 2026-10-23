//! MongoDB and `backup` (MIGRATION_PLAN.md RH3): the Node kit's tests (`kit.test.ts`,
//! `performance.test.ts`), the replica-set check against a fake mongod that speaks `OP_MSG` (no
//! MongoDB is installed where the tests run), and `backup` with `mongodump` mocked.

#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

use std::cell::RefCell;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::json;

use host::commands::backup::{BackupEnv, find_mongodump_in, run_with};
use host::error::HostError;
use host::hostconfig::{DEFAULT_MONGO_URI, HostConfig, write_host_config};
use host::mongo::wire::{Bson, Document, read_request, write_reply};
use host::mongo::{
    MONGO_CACHE_GB, MongoCheck, MongoProblem, ReplicaSetOptions, direct_uri, ensure_replica_set, find_mongod_in,
    first_host, mongod_args,
};
use host::paths::Paths;

#[test]
fn runs_as_a_one_member_replica_set_on_loopback_and_is_first_reached_directly() {
    let args = mongod_args(
        Path::new("D:\\rr\\.workspace\\mongodb"),
        Path::new("D:\\rr\\.workspace\\logs\\mongod.log"),
        27017,
    )
    .unwrap();
    assert_eq!(
        args[..6],
        ["--replSet", "rs0", "--bind_ip", "127.0.0.1", "--port", "27017"]
    );
    assert_eq!(
        direct_uri("mongodb://127.0.0.1:27017/?replicaSet=rs0"),
        "mongodb://127.0.0.1:27017/?directConnection=true"
    );
}

#[test]
fn caps_wiredtigers_cache_instead_of_letting_it_take_half_of_the_ram() {
    let args = mongod_args(
        Path::new("/home/reforged/Reforged/.workspace/mongodb"),
        Path::new("/x/mongod.log"),
        27017,
    )
    .unwrap();
    let at = args.iter().position(|a| a == "--wiredTigerCacheSizeGB").unwrap();
    assert_eq!(args[at + 1], MONGO_CACHE_GB.to_string());
}

#[test]
fn reads_the_first_host_of_a_connection_string() {
    assert_eq!(first_host(DEFAULT_MONGO_URI), Ok(("127.0.0.1".into(), 27017)));
    assert_eq!(
        first_host("mongodb://u:p%40ss@db.local:27018,db2:1/app?x=1"),
        Ok(("db.local".into(), 27018))
    );
    assert_eq!(first_host("mongodb://[::1]:27019/"), Ok(("::1".into(), 27019)));
    assert_eq!(first_host("mongodb://db"), Ok(("db".into(), 27017)));
    assert!(first_host("mongodb+srv://cluster.example.net/").is_err());
}

#[test]
fn finds_mongod_on_path_or_the_newest_in_program_files() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("MongoDB").join("Server");
    for v in ["7.0", "8.0", "10.0"] {
        std::fs::create_dir_all(base.join(v).join("bin")).unwrap();
        std::fs::write(base.join(v).join("bin").join("mongod.exe"), "").unwrap();
    }
    let empty = std::ffi::OsString::new();
    let found = find_mongod_in(Some(&empty), Some(dir.path())).unwrap();
    assert!(
        found.ends_with(&format!(
            "10.0{}bin{}mongod.exe",
            std::path::MAIN_SEPARATOR,
            std::path::MAIN_SEPARATOR
        )),
        "{found}"
    );
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let exe = bin.join(if cfg!(windows) { "mongod.exe" } else { "mongod" });
    std::fs::write(&exe, "").unwrap();
    assert_eq!(
        find_mongod_in(Some(bin.as_os_str()), Some(dir.path())),
        Some(exe.to_string_lossy().into_owned())
    );
    assert_eq!(find_mongod_in(Some(&empty), None), None);
}

/// A mongod for the check: answers `hello`, `replSetGetStatus` and `replSetInitiate` as the state says.
#[derive(Clone, Copy, PartialEq)]
enum Fake {
    Standalone,
    NotInitiated,
    Primary,
}

fn fake_mongod(state: Fake) -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let commands = Arc::new(Mutex::new(Vec::new()));
    let seen = commands.clone();
    std::thread::spawn(move || {
        let mut state = state;
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            while let Some((id, doc)) = read_request(&mut stream) {
                let name = doc.0[0].0.clone();
                assert_eq!(doc.str("$db"), Some("admin"));
                seen.lock().unwrap().push(name.clone());
                let ok = |d: Document| d.with("ok", Bson::Double(1.0));
                let error = |code: i32, msg: &str| {
                    Document::new()
                        .with("ok", Bson::Double(0.0))
                        .with("errmsg", Bson::String(msg.into()))
                        .with("code", Bson::Int32(code))
                };
                let reply = match (name.as_str(), state) {
                    ("hello", Fake::Primary) => ok(Document::new()
                        .with("isWritablePrimary", Bson::Bool(true))
                        .with("setName", Bson::String("rs0".into()))
                        .with(
                            "topologyVersion",
                            Bson::Document(Document::new().with("counter", Bson::Int64(0))),
                        )),
                    ("hello", _) => ok(Document::new().with("isWritablePrimary", Bson::Bool(false))),
                    ("replSetGetStatus", Fake::Standalone) => error(76, "not running with --replSet"),
                    ("replSetGetStatus", _) => error(94, "no replset config has been received"),
                    ("replSetInitiate", _) => {
                        let Some(Bson::Document(config)) = doc.get("replSetInitiate") else {
                            panic!("no config")
                        };
                        assert_eq!(config.str("_id"), Some("rs0"));
                        state = Fake::Primary;
                        ok(Document::new())
                    }
                    _ => error(59, "no such command"),
                };
                write_reply(&mut stream, id, &reply).unwrap();
            }
        }
    });
    (port, commands)
}

fn check(port: u16, initiate: bool) -> MongoCheck {
    let uri = format!("mongodb://127.0.0.1:{port}/?replicaSet=rs0");
    let options = ReplicaSetOptions {
        initiate,
        host: Some(format!("127.0.0.1:{port}")),
        timeout_ms: Some(2000),
    };
    ensure_replica_set(&uri, &options).unwrap()
}

#[test]
fn finds_a_primary_initiates_a_new_replica_set_and_never_changes_a_standalone_mongod() {
    let (port, _) = fake_mongod(Fake::Primary);
    assert_eq!(
        check(port, false),
        MongoCheck::Ok {
            set_name: "rs0".into(),
            primary: true,
            initiated: false
        }
    );

    let (port, seen) = fake_mongod(Fake::NotInitiated);
    assert_eq!(
        check(port, false),
        MongoCheck::Failed {
            reason: MongoProblem::Error,
            detail: "the replica set is not initiated yet".into()
        }
    );
    assert_eq!(
        check(port, true),
        MongoCheck::Ok {
            set_name: "rs0".into(),
            primary: true,
            initiated: true
        }
    );
    assert!(seen.lock().unwrap().contains(&"replSetInitiate".to_owned()));

    let (port, seen) = fake_mongod(Fake::Standalone);
    let MongoCheck::Failed { reason, detail } = check(port, true) else {
        panic!("a standalone mongod")
    };
    assert_eq!(reason, MongoProblem::Standalone);
    assert!(detail.contains("--replSet rs0"), "{detail}");
    assert!(
        !seen.lock().unwrap().contains(&"replSetInitiate".to_owned()),
        "never changed"
    );

    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let nobody = closed.local_addr().unwrap().port();
    drop(closed);
    assert!(matches!(
        check(nobody, false),
        MongoCheck::Failed {
            reason: MongoProblem::Unreachable,
            ..
        }
    ));
}

#[test]
fn doctor_words_what_the_check_found() {
    use host::doctor::{Level, database_check};
    let primary = MongoCheck::Ok {
        set_name: "rs0".into(),
        primary: true,
        initiated: false,
    };
    assert_eq!(
        database_check(&primary, DEFAULT_MONGO_URI, true).text,
        "MongoDB replica set rs0, primary"
    );
    let down = MongoCheck::Failed {
        reason: MongoProblem::Unreachable,
        detail: "refused".into(),
    };
    assert_eq!(
        database_check(&down, DEFAULT_MONGO_URI, true).level,
        Level::Info,
        "start starts it"
    );
    assert_eq!(database_check(&down, DEFAULT_MONGO_URI, false).level, Level::Fail);
}

// ------------------------------------------------------------------ backup

struct MockDump {
    found: Option<PathBuf>,
    runs: RefCell<Vec<Vec<String>>>,
    code: i32,
}

impl BackupEnv for MockDump {
    fn find_mongodump(&self, _config: &HostConfig) -> Result<Option<PathBuf>, HostError> {
        Ok(self.found.clone())
    }
    fn run(&self, _program: &Path, args: &[String], _log_file: &Path) -> Result<(i32, Option<String>), HostError> {
        self.runs.borrow_mut().push(args.to_vec());
        if self.code == 0 {
            let archive = args.iter().find_map(|a| a.strip_prefix("--archive=")).unwrap();
            std::fs::write(archive, b"\x1f\x8b fake archive").unwrap();
            Ok((0, None))
        } else {
            Ok((
                self.code,
                Some("Failed: can't create session: server selection error".into()),
            ))
        }
    }
    fn stamp(&self) -> String {
        "20261022-153000".into()
    }
}

#[test]
fn backup_dumps_the_backends_database_into_the_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let p = Paths::new(dir.path());
    let dump = |found: Option<&str>, code| MockDump {
        found: found.map(PathBuf::from),
        runs: RefCell::new(Vec::new()),
        code,
    };
    let mut lines = Vec::new();
    let env = dump(Some("mongodump"), 0);
    assert_eq!(
        run_with(&p, &env, &mut |l| lines.push(l.to_owned()))
            .unwrap_err()
            .to_string(),
        "not set up yet: run reforged-host setup first"
    );
    let config = HostConfig::from_json(&json!({
        "version": 1, "mode": "private", "host": "100.101.102.103", "gameAddress": "100.101.102.103", "name": "x",
        "gameDir": null, "registration": "INVITECODE", "shareUrl": null,
        "mongo": { "uri": DEFAULT_MONGO_URI, "manage": true, "mongod": null }, "maxHuntServers": 3
    }))
    .unwrap();
    write_host_config(&p.host_json, &config).unwrap();
    std::fs::write(
        p.config.join("backend.env"),
        "MONGODB_URI=mongodb://127.0.0.1:27017/?replicaSet=rs0&w=majority\n",
    )
    .unwrap();

    run_with(&p, &env, &mut |l| lines.push(l.to_owned())).unwrap();
    let archive = p.workspace.join("backups").join("mongodb-20261022-153000.archive.gz");
    assert_eq!(
        env.runs.borrow()[0],
        [
            "--uri=mongodb://127.0.0.1:27017/?replicaSet=rs0&w=majority".to_owned(),
            format!("--archive={}", archive.display()),
            "--gzip".into()
        ],
        "the backend's own URI"
    );
    assert!(
        lines
            .iter()
            .any(|l| l == &format!("Backup written: {} (15 bytes)", archive.display())),
        "{lines:?}"
    );

    let failed = run_with(&p, &dump(Some("mongodump"), 1), &mut |_| {})
        .unwrap_err()
        .to_string();
    assert!(
        failed.starts_with("mongodump failed: Failed: can't create session"),
        "{failed}"
    );
    let missing = run_with(&p, &dump(None, 0), &mut |_| {}).unwrap_err().to_string();
    assert!(
        missing.starts_with("mongodump was not found: install MongoDB Database Tools"),
        "{missing}"
    );
}

#[test]
fn finds_mongodump_on_path_in_the_database_tools_or_next_to_mongod() {
    let dir = tempfile::tempdir().unwrap();
    let exe = if cfg!(windows) { "mongodump.exe" } else { "mongodump" };
    let empty = std::ffi::OsString::new();
    assert_eq!(find_mongodump_in(Some(&empty), Some(dir.path()), None), None);
    let server_bin = dir.path().join("server").join("bin");
    std::fs::create_dir_all(&server_bin).unwrap();
    std::fs::write(server_bin.join(exe), "").unwrap();
    let mongod = server_bin.join("mongod");
    assert_eq!(
        find_mongodump_in(Some(&empty), None, Some(&mongod)),
        Some(server_bin.join(exe))
    );
    let tools = dir.path().join("MongoDB").join("Tools").join("100").join("bin");
    std::fs::create_dir_all(&tools).unwrap();
    std::fs::write(tools.join(exe), "").unwrap();
    assert_eq!(
        find_mongodump_in(Some(&empty), Some(dir.path()), Some(&mongod)),
        Some(tools.join(exe))
    );
}
