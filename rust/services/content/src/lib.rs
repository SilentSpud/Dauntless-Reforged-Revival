//! The content server (MIGRATION_PLAN.md S1, from `legacy/UndauntedContent`): serves the verified
//! 1.4.4 game files to registered accounts over the tailnet, plus the host's news and art pack. A
//! separate process from the metagame, so downloads never slow the game backend.

pub mod app;
pub mod auth;
pub mod branding;
pub mod client;
pub mod config;
pub mod gamedir;
pub mod limits;
pub mod log;
pub mod manifest;
pub mod news;
pub mod paths;
pub mod range;
pub mod server;
pub mod verify;

/// Milliseconds since 1970.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
