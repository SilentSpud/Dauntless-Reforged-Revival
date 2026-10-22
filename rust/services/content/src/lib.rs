//! The content server (MIGRATION_PLAN.md RS2, from `legacy/ReforgedContent`, by way of Dauntless
//! Revived's Rust port of the same server): hands the host-built 1.14.7 game runtime to registered
//! players, plus the host's news and art pack. It listens on loopback only, behind the gateway, and is
//! a separate process from the backend, so downloads never slow the game.

pub mod app;
pub mod auth;
pub mod branding;
pub mod client;
pub mod config;
pub mod limits;
pub mod log;
pub mod news;
pub mod runtime;
pub mod server;

/// Milliseconds since 1970.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
