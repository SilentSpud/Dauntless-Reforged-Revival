//! MongoDB for the backend (`legacy/ReforgedHost/src/mongo.ts`), until the backend cutover (RB14;
//! RB15 removes this module). The backend runs in production mode, which needs a replica-set primary
//! (its registration and economy paths use transactions); a single mongod started with `--replSet`
//! and initiated once is enough. The host kit can run that mongod itself (host.toml `mongo.manage`),
//! or the host points `mongo.uri` at one they run (a Windows service with `replication.replSetName`).
//!
//! The Node kit used the backend's MongoDB driver; this speaks the three commands it needs itself
//! ([`wire`]). It does not authenticate: `hello` answers without it, which covers a replica set that
//! is already initiated; initiating one needs a mongod without access control, as the managed one is.

pub mod wire;

use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::HostError;
use wire::{Bson, Connection, Document, Failure};

pub const REPLICA_SET: &str = "rs0";

/// WiredTiger's cache defaults to half of (RAM - 1 GB): 7.5 GB on a 16 GB server, which the game
/// servers need far more. A friends' server's database is a few megabytes, so 1 GB is plenty.
pub const MONGO_CACHE_GB: u32 = 1;

/// What [`ensure_replica_set`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MongoCheck {
    Ok {
        set_name: String,
        primary: bool,
        initiated: bool,
    },
    Failed {
        reason: MongoProblem,
        detail: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MongoProblem {
    /// The Node kit's "the backend's packages are not installed yet"; this kit has its own client,
    /// so it is never returned.
    NoDriver,
    Unreachable,
    Standalone,
    Error,
}

#[derive(Debug, Clone, Default)]
pub struct ReplicaSetOptions {
    pub initiate: bool,
    pub host: Option<String>,
    pub timeout_ms: Option<u64>,
}

/// mongod's arguments: the replica set, loopback only, the data folder, the log and the cache cap.
pub fn mongod_args(data_dir: &Path, log_file: &Path, port: u16) -> Result<Vec<String>, HostError> {
    Ok([
        "--replSet",
        REPLICA_SET,
        "--bind_ip",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--dbpath",
        &data_dir.to_string_lossy(),
        "--logpath",
        &log_file.to_string_lossy(),
        "--logappend",
        "--wiredTigerCacheSizeGB",
        &MONGO_CACHE_GB.to_string(),
    ]
    .map(str::to_owned)
    .to_vec())
}

/// The first `name` on `path` (a PATH value), as `where` and `which` find it. On Windows a name
/// without an extension is also tried with `.exe`.
pub(crate) fn search_path(name: &str, path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) && Path::new(name).extension().is_none() {
        vec![format!("{name}.exe"), name.to_owned()]
    } else {
        vec![name.to_owned()]
    };
    std::env::split_paths(path?)
        .filter(|dir| !dir.as_os_str().is_empty())
        .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
        .find(|candidate| candidate.is_file())
}

/// Folder names compared as `localeCompare(..., { numeric: true })` does: digit runs as numbers.
pub(crate) fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    fn chunks(s: &str) -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.chars() {
            let digit = c.is_ascii_digit();
            match out.last_mut() {
                Some((d, text)) if *d == digit => text.push(c),
                _ => out.push((digit, c.to_string())),
            }
        }
        out
    }
    let (x, y) = (chunks(a), chunks(b));
    for (p, q) in x.iter().zip(&y) {
        let order = match (p, q) {
            ((true, m), (true, n)) => {
                let (m, n) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
                m.len().cmp(&n.len()).then_with(|| m.cmp(n))
            }
            ((_, m), (_, n)) => m.to_lowercase().cmp(&n.to_lowercase()),
        };
        if order.is_ne() {
            return order;
        }
    }
    x.len().cmp(&y.len())
}

/// The newest `<base>/<version>/bin/<exe>`, as MongoDB's Windows installers lay their programs out.
pub(crate) fn newest_installed(base: &Path, exe: &str) -> Option<PathBuf> {
    let mut versions: Vec<String> = std::fs::read_dir(base)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect();
    versions.sort_by(|a, b| natural_cmp(b, a));
    versions
        .into_iter()
        .map(|v| base.join(v).join("bin").join(exe))
        .find(|candidate| candidate.is_file())
}

/// `C:\Program Files`, from the environment.
pub(crate) fn program_files() -> PathBuf {
    std::env::var_os("ProgramFiles").map_or_else(|| PathBuf::from("C:\\Program Files"), PathBuf::from)
}

/// mongod on PATH, or in the usual Windows install folder (the newest version).
pub fn find_mongod() -> Result<Option<String>, HostError> {
    Ok(find_mongod_in(
        std::env::var_os("PATH").as_deref(),
        cfg!(windows).then(program_files).as_deref(),
    ))
}

/// [`find_mongod`] with the PATH and, on Windows, the Program Files folder given.
pub fn find_mongod_in(path: Option<&std::ffi::OsStr>, program_files: Option<&Path>) -> Option<String> {
    if let Some(found) = search_path("mongod", path) {
        return Some(found.to_string_lossy().into_owned());
    }
    let base = program_files?.join("MongoDB").join("Server");
    newest_installed(&base, "mongod.exe").map(|p| p.to_string_lossy().into_owned())
}

/// The URI without `replicaSet`, straight to one host: used to talk to a mongod whose replica set
/// is not initiated yet (a `replicaSet` URI would wait for a primary that does not exist).
pub fn direct_uri(uri: &str) -> String {
    let Ok(mut url) = url::Url::parse(uri) else {
        return uri.to_owned();
    };
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| k != "replicaSet" && k != "directConnection")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.query_pairs_mut()
        .clear()
        .extend_pairs(pairs)
        .append_pair("directConnection", "true");
    url.to_string()
}

/// The first host of a `mongodb://` URI, with its port (27017 when none is given).
pub fn first_host(uri: &str) -> Result<(String, u16), String> {
    let rest = uri
        .strip_prefix("mongodb://")
        .ok_or_else(|| format!("{uri} is not a mongodb:// connection string (mongodb+srv:// is not supported here)"))?;
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    let hosts = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let first = hosts.split(',').next().unwrap_or_default();
    let (host, port) = if let Some(v6) = first.strip_prefix('[') {
        let (h, after) = v6
            .split_once(']')
            .ok_or("a bad IPv6 address in the connection string")?;
        (h, after.strip_prefix(':'))
    } else {
        match first.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (first, None),
        }
    };
    if host.is_empty() {
        return Err("no host in the connection string".into());
    }
    let port = match port {
        None => 27017,
        Some(p) => p
            .parse()
            .map_err(|_| format!("a bad port in the connection string: {p}"))?,
    };
    Ok((host.to_owned(), port))
}

fn connect(uri: &str, timeout: Duration) -> Result<Connection, String> {
    let (host, port) = first_host(&direct_uri(uri))?;
    let addresses: Vec<_> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .collect();
    let mut last = format!("no address for {host}");
    for address in addresses {
        match TcpStream::connect_timeout(&address, timeout) {
            Ok(stream) => {
                stream.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
                stream.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;
                return Ok(Connection::new(stream));
            }
            Err(e) => last = format!("{address}: {e}"),
        }
    }
    Err(last)
}

fn hello() -> Document {
    Document::new().with("hello", Bson::Int32(1))
}

/// Connects, and initiates the one-member replica set when mongod was started with `--replSet` but
/// never initiated. A standalone mongod is reported, never changed.
pub fn ensure_replica_set(uri: &str, options: &ReplicaSetOptions) -> Result<MongoCheck, HostError> {
    let timeout = Duration::from_millis(options.timeout_ms.unwrap_or(4000));
    let failed = |reason, detail: String| Ok(MongoCheck::Failed { reason, detail });
    let mut db = match connect(uri, timeout) {
        Ok(db) => db,
        Err(detail) => return failed(MongoProblem::Unreachable, detail),
    };
    let answer = match db.command("admin", hello()) {
        Ok(answer) => answer,
        Err(Failure::Io(e)) => return failed(MongoProblem::Unreachable, e.to_string()),
        Err(e) => return failed(MongoProblem::Error, e.to_string()),
    };
    if let Some(set_name) = answer.str("setName").filter(|s| !s.is_empty()) {
        return Ok(MongoCheck::Ok {
            set_name: set_name.to_owned(),
            primary: answer.bool("isWritablePrimary") == Some(true),
            initiated: false,
        });
    }
    // Started with --replSet but not initiated yet: "hello" answers without setName, and
    // replSetGetStatus fails with NotYetInitialized (94).
    match db.command("admin", Document::new().with("replSetGetStatus", Bson::Int32(1))) {
        Ok(_) => return failed(MongoProblem::Error, "unexpected replica set state".into()),
        Err(Failure::Command(e)) if e.code == Some(94) => {}
        Err(Failure::Command(e)) if e.code == Some(76) => {
            return failed(
                MongoProblem::Standalone,
                "this mongod runs without a replica set; start it with --replSet rs0 (or set replication.replSetName: rs0 in mongod.cfg) and restart it".into(),
            );
        }
        Err(e) => return failed(MongoProblem::Error, e.to_string()),
    }
    if !options.initiate {
        return failed(MongoProblem::Error, "the replica set is not initiated yet".into());
    }
    let member = Document::new().with("_id", Bson::Int32(0)).with(
        "host",
        Bson::String(options.host.clone().unwrap_or_else(|| "127.0.0.1:27017".into())),
    );
    let config = Document::new()
        .with("_id", Bson::String(REPLICA_SET.into()))
        .with("members", Bson::Array(vec![Bson::Document(member)]));
    if let Err(e) = db.command("admin", Document::new().with("replSetInitiate", Bson::Document(config))) {
        return failed(MongoProblem::Error, e.to_string());
    }
    // A new one-member set elects itself within a few seconds.
    for _ in 0..40 {
        match db.command("admin", hello()) {
            Ok(again) if again.bool("isWritablePrimary") == Some(true) => {
                return Ok(MongoCheck::Ok {
                    set_name: again.str("setName").unwrap_or(REPLICA_SET).to_owned(),
                    primary: true,
                    initiated: true,
                });
            }
            Ok(_) => {}
            Err(e) => return failed(MongoProblem::Error, e.to_string()),
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(MongoCheck::Ok {
        set_name: REPLICA_SET.into(),
        primary: false,
        initiated: true,
    })
}
