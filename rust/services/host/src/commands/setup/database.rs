//! What setup checks of the backend's database. Until the backend cutover (RB14) that is MongoDB
//! as the Node kit runs it: a one-member replica set that `start` runs (`crate::mongo`), so setup
//! only has to find `mongod`.
//!
//! Seam for RB14: the managed Postgres (RD15: install it, `initdb`, a role and a database for the
//! backend on loopback only) or the host's own with `--postgres-url` goes here as another
//! [`Database`]. Not written here.

use crate::hostconfig::{HostConfig, Platform};

/// The backend's database, as setup prepares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Database {
    /// MongoDB with a replica set (host.toml `[mongo]`), until RB14.
    Mongo,
}

/// The database setup prepares for this config.
pub fn database(_config: &HostConfig) -> Database {
    Database::Mongo
}

/// What setup says about the database at its end: nothing when it is ready to start.
pub fn database_notes(config: &HostConfig, platform: Platform) -> Vec<String> {
    match database(config) {
        Database::Mongo => {
            if !config.mongo.manage || config.mongo.mongod.is_some() {
                return Vec::new();
            }
            let linux = platform == Platform::Linux;
            vec![format!(
                "\n! MongoDB was not found. Install MongoDB Community Server ({}), then run setup again, or pass --mongod <path to {}>.",
                if linux {
                    "docs/hosting-linux.md"
                } else {
                    "https://www.mongodb.com/try/download/community"
                },
                if linux { "mongod" } else { "mongod.exe" }
            )]
        }
    }
}
