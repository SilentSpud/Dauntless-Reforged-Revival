//! The managed MongoDB (`legacy/ReforgedHost/src/mongo.ts`): a one-member replica set on loopback,
//! until the backend cutover (RB14). RH3 adds the managed Postgres beside it (RD15); RB15 removes
//! this module.
//!
//! Filled by RH3. `start` (RH2) uses the helpers below.

use std::path::Path;

use crate::error::{HostError, not_implemented};

pub const REPLICA_SET: &str = "rs0";

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

/// mongod(.exe): on PATH or in MongoDB's usual install folders.
pub fn find_mongod() -> Result<Option<String>, HostError> {
    not_implemented("finding mongod", "RH3")
}

/// mongod's arguments: the replica set, loopback only, the data folder, the log and the cache cap.
pub fn mongod_args(_data_dir: &Path, _log_file: &Path, _port: u16) -> Result<Vec<String>, HostError> {
    not_implemented("mongod's arguments", "RH3")
}

/// Connects, and initiates the one-member replica set when mongod was started with `--replSet` but
/// never initiated. A standalone mongod is reported, never changed.
pub fn ensure_replica_set(_uri: &str, _options: &ReplicaSetOptions) -> Result<MongoCheck, HostError> {
    not_implemented("checking MongoDB's replica set", "RH3")
}
