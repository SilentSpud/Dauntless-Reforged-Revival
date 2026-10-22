//! The gateway (`UndauntedGateway`, MIGRATION_PLAN.md S2): the one public TLS port of a server. It
//! proxies the game's HTTP to the metagame and the content server and its chat WebSocket to the chat
//! listener, refuses admin routes and odd requests, rate-limits per address, and tells the allowlist
//! helper which addresses belong to players who logged in. The `allowlist` binary is that helper
//! (S3).

pub mod allowlist;
pub mod cert;
pub mod config;
pub mod feed;
pub mod gateway;
pub mod guard;
pub mod ip;
pub mod policy;
pub mod ratelimit;
pub mod redact;
pub mod server;

/// Milliseconds since the epoch (`Date.now()`).
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
