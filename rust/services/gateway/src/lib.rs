//! The gateway (`ReforgedGateway`, MIGRATION_PLAN.md RS3): the one public TLS port of a server (or,
//! in private mode, plain HTTP on its Tailscale address). It proxies the game's HTTP to the Mystic
//! Paradox backend and the content server and its chat WebSocket to the backend's chat listener,
//! refuses admin and host-only routes and odd requests, rate-limits per address, can record the
//! traffic for the shadow replay, and tells the allowlist helper which addresses belong to players
//! who logged in. The `allowlist` binary is that helper (RS4).

pub mod allowlist;
pub mod capture;
pub mod cert;
pub mod cli;
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
